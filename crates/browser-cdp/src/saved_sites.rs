//! Owner-facing profile housekeeping. Never read passwords or cookie values.

use std::collections::BTreeSet;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde_json::json;

use crate::{Browser, Error, Result};

/// Refuse links/reparse points before reading or deleting profile data.
pub fn size(path: &Path) -> Result<u64> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    reject_link(&meta)?;
    if !meta.is_dir() {
        return Ok(meta.len());
    }
    let mut total = 0u64;
    for item in std::fs::read_dir(path)? {
        total = total.saturating_add(size(&item?.path())?);
    }
    Ok(total)
}

fn reject_link(meta: &std::fs::Metadata) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(Error::Io("browser profile contains a reparse point".into()));
        }
    }
    if meta.file_type().is_symlink() {
        return Err(Error::Io("browser profile contains a symbolic link".into()));
    }
    Ok(())
}

/// Cookie domains only. Cookies are evidence of saved site data, not proof
/// that the site still accepts a login. Read-only SQLite also respects WAL.
pub fn domains(path: &Path) -> Result<Vec<String>> {
    let mut domains = BTreeSet::new();
    for relative in ["Default/Network/Cookies", "Default/Cookies"] {
        let db = path.join(relative);
        if !db.exists() {
            continue;
        }
        // Check ancestors too; never follow a substituted Default/Network.
        for part in [
            path.to_path_buf(),
            path.join("Default"),
            db.parent().unwrap_or(path).into(),
            db.clone(),
        ] {
            reject_link(&std::fs::symlink_metadata(part)?)?;
        }
        let conn =
            Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_| {
                Error::Io("saved sites are unavailable; close the browser and refresh".into())
            })?;
        let mut stmt = conn
            .prepare("SELECT DISTINCT host_key FROM cookies")
            .map_err(|_| Error::Io("could not read saved site names".into()))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| Error::Io("could not read saved site names".into()))?;
        for row in rows {
            let host = row.map_err(|_| Error::Io("could not read saved site names".into()))?;
            let host = host.trim_start_matches('.');
            if valid_domain(host) {
                domains.insert(host.to_ascii_lowercase());
            }
        }
    }
    Ok(domains.into_iter().collect())
}

/// A plain DNS host, never a path or an arbitrary CDP origin.
pub fn valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Disable password saving before opening a manual-login window. Chromium
/// still keeps session cookies, protected by its normal OS keyring when available.
pub fn prepare(path: &Path) -> Result<()> {
    size(path)?;
    std::fs::create_dir_all(path.join("Default"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let file = path.join("Default/Preferences");
    let mut prefs = match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|_| Error::Io("could not read browser preferences".into()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e.into()),
    };
    if !prefs.is_object() || prefs.get("profile").is_some_and(|p| !p.is_object()) {
        return Err(Error::Io("invalid browser preferences".into()));
    }
    prefs["credentials_enable_service"] = json!(false);
    if prefs.get("profile").is_none() {
        prefs["profile"] = json!({});
    }
    prefs["profile"]["password_manager_enabled"] = json!(false);
    std::fs::write(
        file,
        serde_json::to_vec(&prefs).map_err(|e| Error::Json(e.to_string()))?,
    )?;
    Ok(())
}

/// Try Windows filesystem encryption. Unsupported filesystems/Windows editions
/// may refuse; report that honestly instead of claiming the directory is encrypted.
pub fn encrypt_directory(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return false;
        };
        if reject_link(&meta).is_err() {
            return false;
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: NUL-terminated path, valid for the duration of the call.
        #[allow(unsafe_code)]
        let encrypted = unsafe {
            windows::Win32::Storage::FileSystem::EncryptFileW(windows::core::PCWSTR(wide.as_ptr()))
                .is_ok()
        };
        if !encrypted {
            return false;
        }
        // Existing files need encryption too; new files inherit it from the directory.
        if meta.is_dir() {
            let Ok(entries) = std::fs::read_dir(path) else {
                return false;
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    return false;
                };
                if entry.metadata().is_err() || !encrypt_directory(&entry.path()) {
                    return false;
                }
            }
        }
        true
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// Whether Windows reports directory encryption (other OS/filesystem protection
/// cannot be established here and is never promised).
pub fn directory_encrypted(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return false;
        };
        if reject_link(&meta).is_err() || meta.file_attributes() & 0x4000 == 0 {
            return false;
        }
        !meta.is_dir()
            || std::fs::read_dir(path).is_ok_and(|mut entries| {
                entries.all(|e| e.is_ok_and(|e| directory_encrypted(&e.path())))
            })
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

impl Browser {
    /// Clear cookies and other browser-managed storage for a site and its saved
    /// subdomains. The caller closes old pages first so they cannot recreate it.
    pub async fn forget_site(&self, domain: &str, saved: &[String]) -> Result<()> {
        if !valid_domain(domain) {
            return Err(Error::Io("invalid site name".into()));
        }
        let mut hosts = BTreeSet::from([domain.to_owned()]);
        hosts.extend(
            saved
                .iter()
                .filter(|h| *h == domain || h.ends_with(&format!(".{domain}")))
                .cloned(),
        );
        for host in hosts {
            for scheme in ["https", "http"] {
                self.client()
                    .send(
                        "Storage.clearDataForOrigin",
                        json!({
                            "origin": format!("{scheme}://{host}"), "storageTypes": "all"
                        }),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_names_only_and_password_saving_off() {
        let path = std::env::temp_dir().join(format!("profile-test-{}", uuid::Uuid::new_v4()));
        prepare(&path).unwrap();
        let conn = Connection::open(path.join("Default/Cookies")).unwrap();
        conn.execute_batch("CREATE TABLE cookies(host_key TEXT, encrypted_value BLOB); INSERT INTO cookies VALUES('.example.org', X'736563726574'), ('example.org', X'00'), ('../escape', X'00');").unwrap();
        assert_eq!(domains(&path).unwrap(), ["example.org"]);
        let prefs: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path.join("Default/Preferences")).unwrap())
                .unwrap();
        assert_eq!(prefs["profile"]["password_manager_enabled"], false);
        assert!(size(&path).unwrap() > 0);
        drop(conn);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn housekeeping_refuses_links_outside_the_profile() {
        let path = std::env::temp_dir().join(format!("profile-link-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        std::os::unix::fs::symlink(std::env::temp_dir(), path.join("outside")).unwrap();
        assert!(size(&path).is_err());
        assert!(prepare(&path).is_err());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn forget_site_clears_both_schemes_and_saved_subdomains_only() {
        use futures_util::{SinkExt, StreamExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut origins = Vec::new();
            for _ in 0..4 {
                let request = ws.next().await.unwrap().unwrap().into_text().unwrap();
                let request: serde_json::Value = serde_json::from_str(&request).unwrap();
                assert_eq!(request["method"], "Storage.clearDataForOrigin");
                assert_eq!(request["params"]["storageTypes"], "all");
                origins.push(request["params"]["origin"].as_str().unwrap().to_owned());
                ws.send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"id":request["id"], "result":{}}).to_string().into(),
                ))
                .await
                .unwrap();
            }
            origins
        });
        let browser = Browser::connect(&format!("ws://{addr}")).await.unwrap();
        browser
            .forget_site(
                "example.org",
                &[
                    "example.org".into(),
                    "mail.example.org".into(),
                    "other.org".into(),
                ],
            )
            .await
            .unwrap();
        assert_eq!(
            peer.await.unwrap(),
            [
                "https://example.org",
                "http://example.org",
                "https://mail.example.org",
                "http://mail.example.org"
            ]
        );
        assert!(browser.forget_site("../escape", &[]).await.is_err());
        browser.close().await.unwrap();
    }
}
