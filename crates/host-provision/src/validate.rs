//! Validation of everything the user types before it reaches an argv.
//!
//! Nothing here builds a shell string: the values are later passed as
//! individual argv elements (`-l user -p port -- host`), and these checks make
//! sure none of them can be read as an option or carry control characters.
use std::{
    net::{Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
};

use crate::Error;

fn bad(what: &str, why: &str) -> Error {
    Error::Invalid(format!("{what}: {why}"))
}

/// Hostname, IPv4 or IPv6 literal (no brackets, no zone id).
pub fn host(s: &str) -> Result<String, Error> {
    if s.is_empty() || s.len() > 253 {
        return Err(bad("server address", "empty or too long"));
    }
    if s.starts_with('-') {
        return Err(bad("server address", "must not start with '-'"));
    }
    if s.contains(':') {
        s.parse::<Ipv6Addr>()
            .map_err(|_| bad("server address", "not a valid IPv6 address"))?;
        return Ok(s.to_ascii_lowercase());
    }
    if s.parse::<Ipv4Addr>().is_ok() {
        return Ok(s.to_owned());
    }
    let labels_ok = s.split('.').all(|l| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    if labels_ok {
        Ok(s.to_ascii_lowercase())
    } else {
        Err(bad(
            "server address",
            "use a host name or IP address (letters, digits, '-' and '.')",
        ))
    }
}

/// `[a-z_][a-z0-9_-]{0,31}`.
pub fn user(s: &str) -> Result<String, Error> {
    let mut chars = s.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_');
    if first_ok
        && s.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        Ok(s.to_owned())
    } else {
        Err(bad("user name", "expected a Unix user name like ubuntu, opc or root"))
    }
}

pub fn port(p: u32) -> Result<u16, Error> {
    u16::try_from(p)
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| bad("port", "must be 1-65535"))
}

/// `user@host` or `user@[v6]`; also returns the pieces validated.
pub fn target(s: &str) -> Result<(String, String), Error> {
    let (user_part, host_part) = s
        .rsplit_once('@')
        .ok_or_else(|| bad("server", "expected user@server, for example ubuntu@203.0.113.7"))?;
    let host_part = host_part
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host_part);
    Ok((user(user_part)?, host(host_part)?))
}

/// A short name for the local record: `[a-z0-9][a-z0-9._-]{0,62}`.
pub fn name(s: &str) -> Result<String, Error> {
    let ok = !s.is_empty()
        && s.len() <= 63
        && s.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(s.to_owned())
    } else {
        Err(bad("name", "use lowercase letters, digits, '.', '_' or '-'"))
    }
}

/// Default record name for a host: dots and colons become '-'.
pub fn default_name(host: &str) -> String {
    let n: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    let n = n.trim_matches('-').to_ascii_lowercase();
    if n.is_empty() { "server".into() } else { n.chars().take(63).collect() }
}

/// An existing regular file, absolute, with no quote or control characters
/// (the path ends up inside one `-i` argv element).
pub fn key_path(p: &Path) -> Result<PathBuf, Error> {
    let text = p.to_string_lossy();
    if text.chars().any(|c| c.is_control() || c == '"') {
        return Err(bad("key file", "unusual characters in the path"));
    }
    let abs = std::path::absolute(p).map_err(|e| bad("key file", &e.to_string()))?;
    if !abs.is_file() {
        return Err(bad("key file", "not an existing file"));
    }
    Ok(abs)
}

/// Release tag: `v1.2.3` or `1.2.3` (+ optional `-rc.1` style suffix) -> `v1.2.3...`.
pub fn version_tag(s: &str) -> Result<String, Error> {
    let bare = s.strip_prefix('v').unwrap_or(s);
    let mut parts = bare.splitn(3, '.');
    let numeric = |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let (major, minor, rest) = (parts.next(), parts.next(), parts.next());
    let patch = rest.map(|r| r.split(['-', '+']).next().unwrap_or(""));
    let suffix_ok = rest.is_some_and(|r| {
        r.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
    });
    if numeric(major) && numeric(minor) && numeric(patch) && suffix_ok && bare.len() <= 40 {
        Ok(format!("v{bare}"))
    } else {
        Err(bad("version", "expected a release tag like v0.1.0"))
    }
}

/// A device key as the host CLI prints it: alphanumeric, bounded.
pub fn device_key(s: &str) -> Result<String, Error> {
    if !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(s.to_owned())
    } else {
        Err(bad("device key", "unexpected characters"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_strings_are_rejected_and_normal_ones_pass() {
        for evil in [
            "-oProxyCommand=calc",
            "host;reboot",
            "a b",
            "host\n",
            "$(id)",
            "[::1",
            "evil.com -o x",
            "",
        ] {
            assert!(host(evil).is_err(), "{evil:?}");
        }
        for evil in ["-oProxyCommand=x", "root;id", "Root", "a b", ""] {
            assert!(user(evil).is_err(), "{evil:?}");
        }
        assert!(target("no-at-sign").is_err());
        assert!(target("-oFoo@host").is_err());
        assert_eq!(target("ubuntu@Example.COM").unwrap(), ("ubuntu".into(), "example.com".into()));
        assert_eq!(target("opc@[::1]").unwrap().1, "::1");
        assert!(host("203.0.113.7").is_ok());
        assert!(port(0).is_err() && port(70000).is_err() && port(22).is_ok());
        assert_eq!(version_tag("0.1.0").unwrap(), "v0.1.0");
        assert!(version_tag("latest").is_err() && version_tag("v1.2").is_err());
        assert!(version_tag("v1.2.3;rm").is_err());
    }
}
