//! Where a served browser session may connect (card 272, review blocker 4).
//!
//! A browser that another computer drives must not become a way into the
//! machine's own network: its loopback services (the Praxi daemon API, admin
//! UIs), link-local and metadata endpoints (169.254.169.254) and, unless the
//! owner allows a host, private ranges. The policy is enforced twice:
//! - at the network: every request of a served browser goes through
//!   [`crate::FilterProxy`], which resolves the name itself and connects to
//!   exactly the address it judged (so DNS rebinding cannot split the check
//!   from the connection), and covers popups, workers and `WebSocket`s;
//! - inside the browser (CDP `Fetch` interception, see
//!   [`crate::CdpClient::set_destination_check`]), with every new target
//!   paused until the check is on it.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::pin::Pin;
use std::sync::Arc;

type CheckFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Decides one request URL; `Err` carries the reason it was blocked.
#[derive(Clone)]
pub struct DestinationCheck(Arc<dyn Fn(String) -> CheckFuture + Send + Sync>);

impl DestinationCheck {
    /// Wrap a policy function.
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        Self(Arc::new(move |url| Box::pin(f(url))))
    }

    /// Judge one URL.
    ///
    /// # Errors
    /// The reason the destination is not allowed.
    pub async fn check(&self, url: String) -> Result<(), String> {
        (self.0)(url).await
    }
}

impl std::fmt::Debug for DestinationCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DestinationCheck")
    }
}

/// The network guard of a browser another computer drives: every request
/// goes through a [`crate::FilterProxy`] (which asks `private_host` about a
/// host in a private range), and every page, popup, frame and worker is
/// paused at start until `check` judges its requests (CDP `Fetch`).
#[derive(Clone, Debug)]
pub struct ServedNetwork {
    /// Judges one request URL (defence in depth inside the browser).
    pub check: DestinationCheck,
    /// Judges a private-range HOST the proxy resolved (the owner's allowance).
    pub private_host: DestinationCheck,
}

/// What the policy says about a destination before any owner decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A public destination.
    Allow,
    /// Never reachable: loopback, link-local/metadata, unspecified, multicast,
    /// a scheme that is not http(s)/ws(s), a name that does not resolve.
    Block(String),
    /// A private-range host (RFC 1918, CGNAT, ULA): only with a local allowance
    /// for exactly this host. The string is the host as written.
    Private(String),
}

/// How an address is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrClass {
    Public,
    Private,
    Blocked(&'static str),
}

/// Classify one address (IPv4-mapped IPv6 is judged as the IPv4 it wraps).
#[must_use]
pub fn classify_ip(ip: IpAddr) -> AddrClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return classify_v4(v4);
            }
            classify_v6(v6)
        }
    }
}

fn classify_v4(ip: Ipv4Addr) -> AddrClass {
    let o = ip.octets();
    if ip.is_loopback() {
        AddrClass::Blocked("loopback address (services on this computer)")
    } else if ip.is_link_local() {
        AddrClass::Blocked("link-local / cloud metadata address")
    } else if ip.is_unspecified() || o[0] == 0 {
        AddrClass::Blocked("unspecified address")
    } else if ip.is_multicast() || ip.is_broadcast() {
        AddrClass::Blocked("multicast or broadcast address")
    } else if ip.is_private() || (o[0] == 100 && (o[1] & 0xc0) == 64) {
        AddrClass::Private
    } else {
        AddrClass::Public
    }
}

fn classify_v6(ip: Ipv6Addr) -> AddrClass {
    let s = ip.segments();
    if ip.is_loopback() {
        AddrClass::Blocked("loopback address (services on this computer)")
    } else if ip.is_unspecified() {
        AddrClass::Blocked("unspecified address")
    } else if (s[0] & 0xffc0) == 0xfe80 {
        AddrClass::Blocked("link-local address")
    } else if ip.is_multicast() {
        AddrClass::Blocked("multicast address")
    } else if (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfec0 {
        // Unique-local and (deprecated) site-local.
        AddrClass::Private
    } else if let Some(v4) = embedded_v4(&s) {
        // Forms that carry an IPv4 address are judged as that address.
        classify_v4(v4)
    } else {
        AddrClass::Public
    }
}

/// The IPv4 address inside an IPv4-compatible (`::a.b.c.d`), NAT64
/// (`64:ff9b::/96`) or 6to4 (`2002::/16`) IPv6 address.
fn embedded_v4(s: &[u16; 8]) -> Option<Ipv4Addr> {
    let from =
        |hi: u16, lo: u16| Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
    if s[..6] == [0; 6] || (s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0; 4]) {
        Some(from(s[6], s[7]))
    } else if s[0] == 0x2002 {
        Some(from(s[1], s[2]))
    } else {
        None
    }
}

/// Judge a URL: its scheme, and every address its host resolves to. The worst
/// address decides (one blocked address blocks the name).
pub async fn classify_url(raw: &str) -> Verdict {
    let Ok(url) = url::Url::parse(raw) else {
        return Verdict::Block("not a valid address".into());
    };
    match url.scheme() {
        "http" | "https" | "ws" | "wss" => {}
        // Inline content cannot reach a network by itself.
        "data" | "blob" => return Verdict::Allow,
        "about" if raw == "about:blank" => return Verdict::Allow,
        other => return Verdict::Block(format!("the {other} scheme is not allowed")),
    }
    let Some(host) = url.host() else {
        return Verdict::Block("no host".into());
    };
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs: Vec<IpAddr> = match host {
        url::Host::Ipv4(ip) => vec![IpAddr::V4(ip)],
        url::Host::Ipv6(ip) => vec![IpAddr::V6(ip)],
        url::Host::Domain(name) => {
            if name.eq_ignore_ascii_case("localhost")
                || name.to_ascii_lowercase().ends_with(".localhost")
            {
                return Verdict::Block("loopback address (services on this computer)".into());
            }
            match tokio::net::lookup_host((name, port)).await {
                Ok(it) => it.map(|sa| sa.ip()).collect(),
                Err(_) => return Verdict::Block("the name does not resolve".into()),
            }
        }
    };
    if addrs.is_empty() {
        return Verdict::Block("the name does not resolve".into());
    }
    let mut private = false;
    for ip in addrs {
        match classify_ip(ip) {
            AddrClass::Blocked(why) => return Verdict::Block(why.into()),
            AddrClass::Private => private = true,
            AddrClass::Public => {}
        }
    }
    if private {
        Verdict::Private(url.host_str().unwrap_or_default().to_ascii_lowercase())
    } else {
        Verdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn internal_destinations_are_blocked_whatever_the_spelling() {
        for url in [
            "http://127.0.0.1:8080/v1/status",
            "http://localhost/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://0.0.0.0/",
            "http://[fe80::1]/",
            "http://2130706433/",
            "file:///C:/Windows/win.ini",
            "chrome://settings",
            "javascript:alert(1)",
        ] {
            assert!(
                matches!(classify_url(url).await, Verdict::Block(_)),
                "{url}"
            );
        }
    }

    #[tokio::test]
    async fn private_ranges_need_an_allowance_and_public_addresses_pass() {
        for url in [
            "http://10.0.0.5/",
            "http://192.168.1.1/admin",
            "http://172.16.3.4/",
            "http://100.64.0.1/",
            "http://[fd00::1]/",
        ] {
            assert!(
                matches!(classify_url(url).await, Verdict::Private(_)),
                "{url}"
            );
        }
        for url in [
            "https://93.184.216.34/",
            "http://[2606:2800:220:1::1]/",
            "data:text/plain,hi",
            "about:blank",
        ] {
            assert_eq!(classify_url(url).await, Verdict::Allow, "{url}");
        }
    }
}

#[cfg(test)]
mod v6_embedded_tests {
    use super::*;

    #[test]
    fn ipv6_forms_that_carry_a_private_ipv4_are_not_public() {
        for (ip, private) in [
            ("::192.168.1.5", true),
            ("64:ff9b::10.0.0.1", true),
            ("2002:c0a8:0105::1", true),
            ("fec0::1", true),
            ("64:ff9b::8.8.8.8", false),
            ("2606:4700::1111", false),
        ] {
            let got = classify_ip(ip.parse().unwrap());
            assert_eq!(matches!(got, AddrClass::Private), private, "{ip}");
            assert!(!matches!(got, AddrClass::Blocked(_)) || private, "{ip}");
        }
        assert!(matches!(
            classify_ip("64:ff9b::127.0.0.1".parse().unwrap()),
            AddrClass::Blocked(_)
        ));
    }
}
