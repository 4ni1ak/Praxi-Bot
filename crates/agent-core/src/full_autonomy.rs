//! The one protection that stays in a "Full" (Tam) owner run after a page was read: a browser
//! action whose arguments look like they carry DATA out (a long encoded blob in a URL, a token
//! typed into a form) still asks. Ordinary browsing (a public site, a deep page, a normal query
//! string, normal form text) does not.
//!
//! The daemon does not keep the text a page showed, only the fact that something untrusted was
//! read. So this cannot ask "was this copied from the page?"; it asks the question it can answer
//! from the call alone: "does this argument have the SHAPE of copied data?" (see
//! [`blob_like`]) and "is it bigger than a navigation or a form field needs?". Both limits are
//! deliberately small: a short sentence can still leave this way, a key, a token, an encoded
//! file or a card number cannot.

use serde_json::Value;

/// How much a URL of this run may carry, by how well the site is known.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Reach {
    /// The site the browser is on, one the owner named or one they approved: normal URLs.
    KnownSite,
    /// Any other public site: path and query stay short, no fragment.
    NewSite,
}

impl Reach {
    /// Longest decoded path + query + fragment text.
    const fn total_limit(self) -> usize {
        match self {
            Self::KnownSite => 300,
            Self::NewSite => 120,
        }
    }

    /// Longest single path segment, query key or query value (decoded).
    const fn part_limit(self) -> usize {
        match self {
            Self::KnownSite => 64,
            Self::NewSite => 40,
        }
    }
}

/// Longest text one form field, key press or dialog answer may carry silently.
const TEXT_LIMIT: usize = 400;
/// More query pairs than this is a data channel, not a page address.
const MAX_QUERY_PAIRS: usize = 12;
/// More percent escapes than this in one URL is an encoded payload.
const MAX_PERCENT_ESCAPES: usize = 24;

/// A token of letters and digits that looks like a key, hash, id or encoded blob rather than a
/// word or a slug (`referanslarimiz-hakkimizda`, `2024-yilin-en-iyi-10-urunu`).
pub(crate) fn blob_like(token: &str) -> bool {
    let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let len = token.chars().count();
    if len < 16 {
        return false;
    }
    let digits = token.chars().filter(char::is_ascii_digit).count();
    let letters = token.chars().filter(char::is_ascii_alphabetic).count();
    let upper = token.chars().any(|c| c.is_ascii_uppercase());
    let lower = token.chars().any(|c| c.is_ascii_lowercase());
    // Hex digests and long numbers.
    if token.chars().all(|c| c.is_ascii_hexdigit()) {
        return digits > 0 || len >= 32;
    }
    let separators = token
        .chars()
        .filter(|c| matches!(c, '-' | '_' | '.' | '~'))
        .count();
    // Words and slugs are cut by separators often; a key or an encoded blob is not.
    let sparse = separators * 10 < len;
    if token.ends_with('=') && sparse {
        return true;
    }
    (len >= 20 && digits > 0 && letters > 0 && sparse)
        || (len >= 24 && upper && lower && sparse)
        || (len >= 40 && sparse)
}

/// Well-known secret prefixes (an API key, a bearer token, a private key block).
fn secret_prefixed(token: &str) -> bool {
    let t = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
    let lower = t.to_ascii_lowercase();
    (t.len() >= 12
        && [
            "sk-", "ghp_", "gho_", "xoxb-", "xoxp-", "akia", "eyj", "bearer",
        ]
        .iter()
        .any(|p| lower.starts_with(p)))
        || token.contains("-----BEGIN")
}

/// Digits with spaces or dashes between them that add up to a card or account number.
fn long_number(text: &str) -> bool {
    let mut run = 0usize;
    for c in text.chars() {
        if c.is_ascii_digit() {
            run += 1;
            if run >= 13 {
                return true;
            }
        } else if !matches!(c, ' ' | '-') {
            run = 0;
        }
    }
    false
}

/// Free text (a form field, a typed key, a dialog answer) that looks like copied data.
pub(crate) fn text_carries_data(text: &str) -> bool {
    text.chars().count() > TEXT_LIMIT
        || long_number(text)
        || text
            .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '|' | '"' | '\''))
            .any(|w| blob_like(w) || secret_prefixed(w))
}

/// One decoded URL part (path segment, query key or value, fragment).
fn part_carries_data(part: &str, reach: Reach) -> bool {
    part.chars().count() > reach.part_limit()
        || part
            .split(|c: char| c.is_whitespace() || matches!(c, '/' | ',' | ';' | ':' | '|' | '+'))
            .any(|w| blob_like(w) || secret_prefixed(w))
}

/// `%XX` escapes and `+` decoded, so the judged text is what the site would read.
fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        let escaped = (bytes[i] == b'%')
            .then(|| {
                (
                    bytes.get(i + 1).and_then(|b| hex(*b)),
                    bytes.get(i + 2).and_then(|b| hex(*b)),
                )
            })
            .and_then(|pair| match pair {
                (Some(h), Some(l)) => u8::try_from(h * 16 + l).ok(),
                _ => None,
            });
        if let Some(byte) = escaped {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A public `http(s)` address whose host, path, query or fragment look like they carry data.
///
/// Fails closed: anything that does not parse as a plain web address carries.
pub(crate) fn url_carries_data(raw: &str, reach: Reach) -> bool {
    let Ok(url) = url::Url::parse(raw.trim()) else {
        return true;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return true;
    }
    let Some(host) = url.host_str() else {
        return true;
    };
    // A name made of data (`<encoded-text>.example.com`) is a channel too.
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() > 8 || labels.iter().any(|l| l.len() > 40 || blob_like(l)) {
        return true;
    }
    if raw.matches('%').count() > MAX_PERCENT_ESCAPES {
        return true;
    }
    let mut total = 0usize;
    if let Some(segments) = url.path_segments() {
        for segment in segments {
            let part = decode(segment);
            total += part.chars().count();
            if part_carries_data(&part, reach) {
                return true;
            }
        }
    }
    let mut pairs = 0usize;
    for (k, v) in url.query_pairs() {
        pairs += 1;
        // `query_pairs` already decodes, so only the shape is judged here.
        total += k.chars().count() + v.chars().count();
        if pairs > MAX_QUERY_PAIRS || part_carries_data(&k, reach) || part_carries_data(&v, reach) {
            return true;
        }
    }
    if let Some(fragment) = url.fragment().filter(|f| !f.is_empty()) {
        let part = decode(fragment);
        total += part.chars().count();
        if reach == Reach::NewSite || part_carries_data(&part, reach) {
            return true;
        }
    }
    total > reach.total_limit()
}

/// Whether any argument of a browser action carries data: a URL field is judged as an address
/// (`reach_of` says how well the site is known), every other string as free text. Keys of the
/// call are the tool's own schema and are not judged.
pub(crate) fn input_carries_data(input: &Value, reach_of: &dyn Fn(&str) -> Reach) -> bool {
    fn walk(value: &Value, key: Option<&str>, reach_of: &dyn Fn(&str) -> Reach) -> bool {
        match value {
            Value::String(s) => {
                let is_url = key.is_some_and(|k| {
                    matches!(k.to_ascii_lowercase().as_str(), "url" | "uri" | "href")
                });
                if is_url {
                    url_carries_data(s, reach_of(s))
                } else {
                    text_carries_data(s)
                }
            }
            Value::Array(items) => items.iter().any(|v| walk(v, key, reach_of)),
            Value::Object(map) => map.iter().any(|(k, v)| walk(v, Some(k), reach_of)),
            _ => false,
        }
    }
    walk(input, None, reach_of)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ordinary_addresses_do_not_carry_data() {
        for url in [
            "https://webioo.com.tr/",
            "https://webioo.com.tr/referanslarimiz",
            "https://webioo.com.tr/iletisim?page=2&sort=new",
            "https://webioo.com.tr/blog/2024-yilin-en-iyi-10-urunu",
            "https://webioo.com.tr/ara?q=web+tasarim+fiyatlari",
            "https://webioo.com.tr/hizmetler#fiyat",
            "https://www.google.com/search?q=webioo+referanslar",
            "https://cdn.example.com/a.js?v=1700000000000",
            "https://webioo.com.tr/%C3%BCr%C3%BCnler/%C5%9Fapka",
        ] {
            assert!(!url_carries_data(url, Reach::KnownSite), "{url}");
        }
        assert!(!url_carries_data(
            "https://other.example/products?page=3",
            Reach::NewSite
        ));
    }

    #[test]
    fn data_shaped_addresses_carry() {
        for url in [
            "https://webioo.com.tr/c2VjcmV0LXRva2VuLWZyb20tdGhlLXBhZ2UtYWJjZGVmZ2hpams",
            "https://webioo.com.tr/?d=0123456789abcdef0123456789abcdef",
            "https://webioo.com.tr/?q=sk-live-AbCdEfGhIjKlMnOp",
            "https://webioo.com.tr/x?token=ghp_abcdefghijklmnop1234",
            "https://webioo.com.tr/?a=1&b=2&c=3&d=4&e=5&f=6&g=7&h=8&i=9&j=10&k=11&l=12&m=13",
            "https://c2VjcmV0LXRva2VuLWZyb20tdGhlLXBhZ2U.evil.example/",
            "https://u:p@webioo.com.tr/",
            "ftp://webioo.com.tr/",
            "not a url",
        ] {
            assert!(url_carries_data(url, Reach::KnownSite), "{url}");
        }
        // an unknown site gets no fragment and a smaller budget
        assert!(url_carries_data(
            "https://other.example/#anything",
            Reach::NewSite
        ));
        let long = format!("https://other.example/?q={}", "kelime+".repeat(7));
        assert!(url_carries_data(&long, Reach::NewSite));
        assert!(!url_carries_data(&long, Reach::KnownSite));
    }

    #[test]
    fn form_text_is_judged_by_shape_not_by_topic() {
        for text in [
            "Merhaba, fiyat teklifi almak istiyorum.",
            "ali@example.com",
            "Enter",
            "Control+A",
            "+90 555 123 45 67",
        ] {
            assert!(!text_carries_data(text), "{text}");
        }
        for text in [
            "4111 1111 1111 1111",
            "sk-live-AbCdEfGhIjKlMnOp",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
            "0123456789abcdef0123456789abcdef",
            "-----BEGIN PRIVATE KEY-----",
        ] {
            assert!(text_carries_data(text), "{text}");
        }
        assert!(text_carries_data(&"a ".repeat(300)));
    }

    #[test]
    fn a_call_is_judged_per_field() {
        let known = |_: &str| Reach::KnownSite;
        assert!(!input_carries_data(
            &json!({"ref": "e12", "text": "merhaba"}),
            &known
        ));
        assert!(input_carries_data(
            &json!({"ref": "e12", "text": "sk-live-AbCdEfGhIjKlMnOp"}),
            &known
        ));
        assert!(!input_carries_data(
            &json!({"steps": [{"op": "click", "ref": "e1"}, {"op": "fill", "ref": "e2", "text": "ali"}]}),
            &known
        ));
        assert!(input_carries_data(
            &json!({"url": "https://a.example/?d=0123456789abcdef0123456789abcdef"}),
            &known
        ));
    }
}
