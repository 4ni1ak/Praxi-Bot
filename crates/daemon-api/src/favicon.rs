//! Site favicons: fetched directly from the target public domain via the
//! daemon's SSRF-guarded HTTP fetcher (never via any third-party favicon
//! service), validated by magic bytes / safe static SVG, cached under
//! `<data_dir>/cache/favicons/`, and served with strict `nosniff` + CSP headers.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::Response;
use url::Url;
use web_tools::{DefaultUrlPolicy, GetOptions, HttpConfig, HttpFetcher, metadata};

use crate::AppState;
use crate::error::{ApiError, ErrorBody};

/// Largest favicon accepted and cached (96 KiB).
pub const MAX_FAVICON_BYTES: usize = 96 * 1024;
/// Maximum HTML document bytes scanned for `<link rel="icon">`.
const MAX_HTML_BYTES: usize = 256 * 1024;
/// How long cached favicons stay fresh on disk.
const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// How long a failed lookup is negatively cached.
const MISS_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Favicon {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaviconError {
    TooLarge,
    NotImage,
    Unsafe,
}

/// Validate a public DNS domain name for favicon lookup.
/// Rejects IPs, localhost, `.local`/`.internal`/`.arpa`/`.lan`, empty labels,
/// and anything outside `[a-z0-9.-]`.
pub(crate) fn check_domain(raw: &str) -> Result<String, ApiError> {
    let domain = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() || domain.len() > 253 || !domain.contains('.') {
        return Err(ApiError::bad_request("invalid domain"));
    }
    if domain.parse::<IpAddr>().is_ok() {
        return Err(ApiError::bad_request("ip address is not a domain"));
    }
    if matches!(
        domain.rsplit('.').next(),
        Some("localhost" | "local" | "internal" | "arpa" | "lan" | "corp" | "intranet" | "home")
    ) {
        return Err(ApiError::bad_request("non-public domain"));
    }
    for label in domain.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(ApiError::bad_request("invalid domain label"));
        }
    }
    Ok(domain)
}

/// Active/external SVG constructs that must never be served.
const FORBIDDEN_SVG: &[&str] = &[
    "<script",
    "<foreignobject",
    "<iframe",
    "<object",
    "<embed",
    "<!entity",
    "<!doctype",
    "<image",
    "<use",
    "<a ",
    "<a>",
    "<animate",
    "<set ",
    "<style",
    "javascript:",
    "data:",
    "http://",
    "https://",
    "xlink:href",
    "href=",
    "@import",
    "url(",
];

fn check_svg(bytes: &[u8]) -> Result<(), FaviconError> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Err(FaviconError::NotImage);
    };
    let lower = text.to_ascii_lowercase();
    if !lower.contains("<svg") {
        return Err(FaviconError::NotImage);
    }
    let stripped = lower.replace("http://www.w3.org/2000/svg", "");
    if FORBIDDEN_SVG.iter().any(|f| stripped.contains(f)) {
        return Err(FaviconError::Unsafe);
    }
    let raw = lower.as_bytes();
    for (i, w) in raw.windows(3).enumerate() {
        if w == b" on" || w == b"\non" || w == b"\ton" {
            let rest = &lower[i + 3..];
            let name: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
            let after = rest[name.len()..].trim_start();
            if !name.is_empty() && after.starts_with('=') {
                return Err(FaviconError::Unsafe);
            }
        }
    }
    Ok(())
}

/// Detect and validate favicon image bytes by their magic signature.
pub fn classify(bytes: &[u8]) -> Result<&'static str, FaviconError> {
    if bytes.len() > MAX_FAVICON_BYTES {
        return Err(FaviconError::TooLarge);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok("image/png");
    }
    if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok("image/webp");
    }
    if bytes.len() > 6 && (&bytes[..4] == b"\x00\x00\x01\x00" || &bytes[..4] == b"\x00\x00\x02\x00")
    {
        return Ok("image/x-icon");
    }
    let head = &bytes[..bytes.len().min(512)];
    let trimmed = head
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map_or(head, |i| &head[i..]);
    if trimmed.starts_with(b"<svg") || trimmed.starts_with(b"<?xml") {
        check_svg(bytes)?;
        return Ok("image/svg+xml");
    }
    Err(FaviconError::NotImage)
}

fn extension(content_type: &str) -> &'static str {
    match content_type {
        "image/png" => "png",
        "image/webp" => "webp",
        "image/x-icon" => "ico",
        _ => "svg",
    }
}

fn cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("favicons")
}

fn fresh(meta: &std::fs::Metadata, ttl: Duration) -> bool {
    meta.modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age < ttl)
}

enum CacheHit {
    Icon(Favicon),
    Miss,
    Empty,
}

fn read_cache(data_dir: &Path, domain: &str) -> CacheHit {
    let dir = cache_dir(data_dir);
    for ext in ["png", "webp", "ico", "svg"] {
        let p = dir.join(format!("{domain}.{ext}"));
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if !meta.is_file() || meta.file_type().is_symlink() || !fresh(&meta, CACHE_TTL) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&p)
            && let Ok(content_type) = classify(&bytes)
            && extension(content_type) == ext
        {
            return CacheHit::Icon(Favicon {
                bytes,
                content_type,
            });
        }
    }
    let miss = dir.join(format!("{domain}.miss"));
    if let Ok(meta) = std::fs::symlink_metadata(&miss)
        && meta.is_file()
        && !meta.file_type().is_symlink()
        && fresh(&meta, MISS_TTL)
    {
        return CacheHit::Miss;
    }
    CacheHit::Empty
}

fn store_cache(data_dir: &Path, domain: &str, icon: &Favicon) {
    let dir = cache_dir(data_dir);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::remove_file(dir.join(format!("{domain}.miss")));
        let _ = std::fs::write(
            dir.join(format!("{domain}.{}", extension(icon.content_type))),
            &icon.bytes,
        );
    }
}

fn store_miss(data_dir: &Path, domain: &str) {
    let dir = cache_dir(data_dir);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join(format!("{domain}.miss")), b"");
    }
}

fn same_site_https(candidate: &str, domain: &str) -> Option<Url> {
    let url = Url::parse(candidate).ok()?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host == domain || host.ends_with(&format!(".{domain}")) {
        Some(url)
    } else {
        None
    }
}

async fn fetch_from_origin(
    origin_base: &Url,
    domain: &str,
    fetcher: &HttpFetcher,
) -> Option<Favicon> {
    let icon_url = origin_base.join("/favicon.ico").ok()?;
    let icon_opts = GetOptions {
        max_bytes: Some(MAX_FAVICON_BYTES),
        timeout: Some(Duration::from_secs(3)),
        ..GetOptions::default()
    };
    if let Ok(resp) = fetcher.get(icon_url.as_str(), &icon_opts).await
        && (200..300).contains(&resp.status)
        && let Ok(content_type) = classify(&resp.body)
    {
        return Some(Favicon {
            bytes: resp.body,
            content_type,
        });
    }

    let html_opts = GetOptions {
        accept_types: vec!["text/html".to_owned(), "application/xhtml+xml".to_owned()],
        max_bytes: Some(MAX_HTML_BYTES),
        timeout: Some(Duration::from_secs(3)),
        ..GetOptions::default()
    };
    let page = fetcher.get(origin_base.as_str(), &html_opts).await.ok()?;
    if !(200..300).contains(&page.status) {
        return None;
    }
    let meta = metadata::extract_metadata(&page.text(), Some(&page.url));
    for candidate in meta.icons {
        let Some(url) = same_site_https(&candidate, domain) else {
            continue;
        };
        if let Ok(resp) = fetcher.get(url.as_str(), &icon_opts).await
            && (200..300).contains(&resp.status)
            && let Ok(content_type) = classify(&resp.body)
        {
            return Some(Favicon {
                bytes: resp.body,
                content_type,
            });
        }
    }
    None
}

async fn resolve_favicon_with(
    data_dir: &Path,
    domain: &str,
    origin_base: &Url,
    fetcher: &HttpFetcher,
) -> Option<Favicon> {
    match read_cache(data_dir, domain) {
        CacheHit::Icon(icon) => return Some(icon),
        CacheHit::Miss => return None,
        CacheHit::Empty => {}
    }
    if let Some(icon) = fetch_from_origin(origin_base, domain, fetcher).await {
        store_cache(data_dir, domain, &icon);
        Some(icon)
    } else {
        store_miss(data_dir, domain);
        None
    }
}

/// Fetch and cache a site's favicon directly from `https://<domain>` (never a
/// third-party favicon service). Returns `404` when unavailable so the UI falls
/// back to a globe or letter tile.
#[utoipa::path(
    get,
    path = "/v1/favicons/{domain}",
    tag = "favicons",
    security(("bearer" = [])),
    params(("domain" = String, Path, description = "Public site domain")),
    responses(
        (status = 200, description = "Favicon image bytes"),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody)
    )
)]
pub(crate) async fn get_favicon(
    State(s): State<AppState>,
    AxumPath(domain): AxumPath<String>,
) -> Result<Response, ApiError> {
    let domain = check_domain(&domain)?;
    let origin = Url::parse(&format!("https://{domain}/"))
        .map_err(|_| ApiError::bad_request("invalid domain"))?;
    let fetcher = HttpFetcher::new(
        Arc::new(DefaultUrlPolicy::default()),
        HttpConfig {
            timeout: Duration::from_secs(4),
            connect_timeout: Duration::from_secs(3),
            max_bytes: MAX_HTML_BYTES,
            max_redirects: 3,
            ..HttpConfig::default()
        },
    )
    .map_err(|e| ApiError::internal(format!("favicon client: {e}")))?;

    let icon = resolve_favicon_with(&s.data_dir(), &domain, &origin, &fetcher)
        .await
        .ok_or_else(|| ApiError::not_found("favicon"))?;

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", icon.content_type)
        .header("x-content-type-options", "nosniff")
        .header(
            "content-security-policy",
            "default-src 'none'; style-src 'unsafe-inline'; sandbox",
        )
        .header("cache-control", "private, max-age=86400")
        .body(Body::from(icon.bytes))
        .map_err(ApiError::internal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use web_tools::UrlPolicy;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
    const ICO: &[u8] = b"\x00\x00\x01\x00\x01\x00\x10\x10\x00\x00\x01\x00\x20\x00";

    struct AllowLoopback;
    impl UrlPolicy for AllowLoopback {
        fn check_url(&self, _: &Url) -> Result<(), web_tools::PolicyViolation> {
            Ok(())
        }
        fn check_ip(&self, _: &str, _: IpAddr) -> Result<(), web_tools::PolicyViolation> {
            Ok(())
        }
    }

    #[test]
    fn rejects_non_public_ips_and_bad_domains() {
        assert_eq!(check_domain("Webioo.COM.TR.").unwrap(), "webioo.com.tr");
        for bad in [
            "",
            "localhost",
            "127.0.0.1",
            "::1",
            "printer.local",
            "service.internal",
            "a..b.com",
            "-bad.com",
            "bad-.com",
            "a/b.com",
            "user@example.com",
        ] {
            assert!(check_domain(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn classifies_valid_images_and_blocks_active_svg() {
        assert_eq!(classify(PNG), Ok("image/png"));
        assert_eq!(classify(ICO), Ok("image/x-icon"));
        assert_eq!(classify(b"RIFF\x10\x00\x00\x00WEBPVP8 "), Ok("image/webp"));
        assert_eq!(
            classify(br#"<svg xmlns="http://www.w3.org/2000/svg"><circle r="4"/></svg>"#),
            Ok("image/svg+xml")
        );
        assert_eq!(
            classify(br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#),
            Err(FaviconError::Unsafe)
        );
        assert_eq!(
            classify(b"<html><body>not an icon</body></html>"),
            Err(FaviconError::NotImage)
        );
    }

    #[tokio::test]
    async fn fetches_once_and_serves_from_local_cache_without_third_party_calls() {
        let srv = MockServer::start().await;
        Mock::given(path("/favicon.ico"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG))
            .expect(1)
            .mount(&srv)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let fetcher = HttpFetcher::new(Arc::new(AllowLoopback), HttpConfig::default()).unwrap();
        let origin = Url::parse(&srv.uri()).unwrap();

        let first = resolve_favicon_with(dir.path(), "webioo.com.tr", &origin, &fetcher)
            .await
            .unwrap();
        assert_eq!(first.content_type, "image/png");
        assert_eq!(first.bytes, PNG);

        // Second call must hit the local disk cache (mock expects exactly 1 HTTP hit).
        let second = resolve_favicon_with(dir.path(), "webioo.com.tr", &origin, &fetcher)
            .await
            .unwrap();
        assert_eq!(second, first);

        // Tampered cache file is ignored rather than served.
        std::fs::write(
            cache_dir(dir.path()).join("webioo.com.tr.png"),
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script/></svg>",
        )
        .unwrap();
        assert!(matches!(
            read_cache(dir.path(), "webioo.com.tr"),
            CacheHit::Empty
        ));
    }

    #[test]
    fn same_site_filter_blocks_third_party_icon_urls() {
        assert!(same_site_https("https://webioo.com.tr/icon.png", "webioo.com.tr").is_some());
        assert!(same_site_https("https://cdn.webioo.com.tr/icon.png", "webioo.com.tr").is_some());
        assert!(
            same_site_https(
                "https://www.google.com/s2/favicons?domain=webioo.com.tr",
                "webioo.com.tr"
            )
            .is_none()
        );
        assert!(same_site_https("http://webioo.com.tr/icon.png", "webioo.com.tr").is_none());
    }
}
