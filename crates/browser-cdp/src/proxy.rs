//! The network guard of a served browser (card 272, review blocker 2).
//!
//! A browser another computer drives must not become a way into this
//! machine's own network. Checking a URL inside the browser is not enough:
//! the browser resolves the name again afterwards (DNS rebinding), and pages
//! can open popups, workers and `WebSocket`s that a per-page hook never sees.
//!
//! So EVERY request of a served browser goes through [`FilterProxy`], a small
//! HTTP proxy on loopback that:
//! - resolves the name itself (Chrome is told it cannot resolve anything, see
//!   [`FilterProxy::chrome_args`]),
//! - classifies every address (loopback, link-local/metadata, unspecified and
//!   multicast are never reached; a private range only with the owner's
//!   allowance for that host),
//! - and connects to exactly the addresses it classified, so the check and
//!   the connection cannot use two different answers.
//!
//! `CONNECT` (https, wss) is tunnelled; plain `http://` requests are forwarded
//! one per connection (`Connection: close`). Nothing else is served.

use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::destination::{AddrClass, DestinationCheck, classify_ip};

type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A connection to the destination.
pub trait Upstream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Upstream for T {}

type ResolveFn = dyn Fn(String, u16) -> BoxFut<io::Result<Vec<IpAddr>>> + Send + Sync;
type ConnectFn = dyn Fn(SocketAddr) -> BoxFut<io::Result<Box<dyn Upstream>>> + Send + Sync;

/// How the proxy resolves names and opens connections (the system's by
/// default; tests put fakes here).
#[derive(Clone)]
pub struct Network {
    resolve: Arc<ResolveFn>,
    connect: Arc<ConnectFn>,
}

impl Network {
    /// The system resolver and plain TCP.
    #[must_use]
    pub fn system() -> Self {
        Self {
            resolve: Arc::new(|host, port| {
                Box::pin(async move {
                    Ok(tokio::net::lookup_host((host.as_str(), port))
                        .await?
                        .map(|sa| sa.ip())
                        .collect())
                })
            }),
            connect: Arc::new(|addr| {
                Box::pin(async move {
                    let s = TcpStream::connect(addr).await?;
                    Ok(Box::new(s) as Box<dyn Upstream>)
                })
            }),
        }
    }

    /// A custom resolver and connector.
    pub fn new<R, RF, C, CF>(resolve: R, connect: C) -> Self
    where
        R: Fn(String, u16) -> RF + Send + Sync + 'static,
        RF: Future<Output = io::Result<Vec<IpAddr>>> + Send + 'static,
        C: Fn(SocketAddr) -> CF + Send + Sync + 'static,
        CF: Future<Output = io::Result<Box<dyn Upstream>>> + Send + 'static,
    {
        Self {
            resolve: Arc::new(move |h, p| Box::pin(resolve(h, p))),
            connect: Arc::new(move |a| Box::pin(connect(a))),
        }
    }
}

/// Longest request head the proxy reads.
const MAX_HEAD: usize = 32 * 1024;
/// How long a client may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(20);
/// How long one connection attempt may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A running filtering proxy; stops when dropped.
pub struct FilterProxy {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FilterProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl std::fmt::Debug for FilterProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FilterProxy({})", self.addr)
    }
}

impl FilterProxy {
    /// Start on an ephemeral loopback port. `private_host` decides a host
    /// whose addresses are in a private range (the owner's allowance);
    /// everything else is decided here.
    ///
    /// # Errors
    /// The loopback port could not be opened.
    pub async fn start(private_host: DestinationCheck) -> io::Result<Self> {
        Self::start_with(private_host, Network::system()).await
    }

    /// [`Self::start`] over a given network (tests).
    ///
    /// # Errors
    /// The loopback port could not be opened.
    pub async fn start_with(private_host: DestinationCheck, net: Network) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let task = tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let (policy, net) = (private_host.clone(), net.clone());
                tokio::spawn(async move {
                    let _ = serve(sock, &policy, &net).await;
                });
            }
        });
        Ok(Self { addr, task })
    }

    /// Where the proxy listens.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Chrome flags that send every request of the browser through this
    /// proxy: loopback is NOT bypassed (Chrome bypasses it by default), Chrome
    /// itself resolves no name, QUIC is off and WebRTC uses no direct UDP.
    #[must_use]
    pub fn chrome_args(addr: SocketAddr) -> Vec<String> {
        vec![
            format!("--proxy-server=http://{addr}"),
            "--proxy-bypass-list=<-loopback>".into(),
            format!(
                "--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE {}",
                addr.ip()
            ),
            "--disable-quic".into(),
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp".into(),
        ]
    }
}

/// Decide a destination: the addresses to connect to, all of them allowed.
async fn decide(
    host: &str,
    port: u16,
    policy: &DestinationCheck,
    net: &Network,
) -> Result<Vec<SocketAddr>, String> {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return Err("no host".into());
    }
    let ips: Vec<IpAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![ip]
    } else if host == "localhost" || host.ends_with(".localhost") {
        return Err("loopback address (services on this computer)".into());
    } else {
        (net.resolve)(host.clone(), port)
            .await
            .map_err(|_| "the name does not resolve".to_owned())?
    };
    if ips.is_empty() {
        return Err("the name does not resolve".into());
    }
    let mut private = false;
    for ip in &ips {
        match classify_ip(*ip) {
            AddrClass::Blocked(why) => return Err(why.into()),
            AddrClass::Private => private = true,
            AddrClass::Public => {}
        }
    }
    if private {
        policy.check(host).await?;
    }
    Ok(ips
        .into_iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect())
}

async fn connect_any(net: &Network, addrs: &[SocketAddr]) -> io::Result<Box<dyn Upstream>> {
    let mut last = io::Error::other("no address");
    for a in addrs {
        match tokio::time::timeout(CONNECT_TIMEOUT, (net.connect)(*a)).await {
            Ok(Ok(s)) => return Ok(s),
            Ok(Err(e)) => last = e,
            Err(_) => last = io::Error::new(io::ErrorKind::TimedOut, "connect timed out"),
        }
    }
    Err(last)
}

async fn read_head(sock: &mut TcpStream) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "head too long"));
        }
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn answer(sock: &mut TcpStream, status: &str, why: &str) -> io::Result<()> {
    let body = format!("Praxi did not open this address: {why}\n");
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body.as_bytes()).await?;
    sock.shutdown().await
}

/// `host:port` of a CONNECT target (IPv6 in brackets).
fn host_port(target: &str) -> Option<(String, u16)> {
    let (host, port) = target.rsplit_once(':')?;
    let port = port.parse().ok()?;
    (!host.is_empty()).then(|| (host.to_owned(), port))
}

async fn serve(mut client: TcpStream, policy: &DestinationCheck, net: &Network) -> io::Result<()> {
    let (head, rest) = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut client))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "head"))??;
    let Ok(text) = std::str::from_utf8(&head) else {
        return answer(&mut client, "400 Bad Request", "unreadable request").await;
    };
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let parts: Vec<&str> = first.split(' ').collect();
    let [method, target, version] = parts.as_slice() else {
        return answer(&mut client, "400 Bad Request", "unreadable request").await;
    };
    if method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = host_port(target) else {
            return answer(&mut client, "400 Bad Request", "bad tunnel target").await;
        };
        let addrs = match decide(&host, port, policy, net).await {
            Ok(a) => a,
            Err(why) => return answer(&mut client, "403 Forbidden", &why).await,
        };
        let Ok(mut up) = connect_any(net, &addrs).await else {
            return answer(&mut client, "502 Bad Gateway", "the address did not answer").await;
        };
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        if !rest.is_empty() {
            up.write_all(&rest).await?;
        }
        let _ = tokio::io::copy_bidirectional(&mut client, &mut up).await;
        return Ok(());
    }
    // A plain request in absolute form: http only.
    let Ok(url) = url::Url::parse(target) else {
        return answer(&mut client, "400 Bad Request", "not a proxy request").await;
    };
    if url.scheme() != "http" {
        return answer(&mut client, "403 Forbidden", "only http is forwarded").await;
    }
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return answer(&mut client, "400 Bad Request", "no host").await;
    };
    let addrs = match decide(host, port, policy, net).await {
        Ok(a) => a,
        Err(why) => return answer(&mut client, "403 Forbidden", &why).await,
    };
    let Ok(mut up) = connect_any(net, &addrs).await else {
        return answer(&mut client, "502 Bad Gateway", "the address did not answer").await;
    };
    let path = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_owned(),
    };
    let headers: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
    let upgrade = headers
        .iter()
        .any(|h| h.to_ascii_lowercase().starts_with("upgrade:"));
    let mut out = format!("{method} {path} {version}\r\n");
    for h in headers {
        let lower = h.to_ascii_lowercase();
        let hop = lower.starts_with("proxy-connection:")
            || lower.starts_with("proxy-authorization:")
            || lower.starts_with("keep-alive:")
            || (!upgrade && lower.starts_with("connection:"));
        if !hop {
            out.push_str(h);
            out.push_str("\r\n");
        }
    }
    if !upgrade {
        // One request per connection: the next one is decided again.
        out.push_str("Connection: close\r\n");
    }
    out.push_str("\r\n");
    up.write_all(out.as_bytes()).await?;
    if !rest.is_empty() {
        up.write_all(&rest).await?;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut up).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A destination that answers every request with "ok".
    fn fake_upstream() -> Box<dyn Upstream> {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            let mut theirs = theirs;
            let mut buf = [0u8; 4096];
            let _ = theirs.read(&mut buf).await;
            let _ = theirs
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await;
            let _ = theirs.shutdown().await;
        });
        Box::new(ours)
    }

    async fn send(proxy: SocketAddr, raw: &str) -> String {
        let mut s = TcpStream::connect(proxy).await.unwrap();
        s.write_all(raw.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
        String::from_utf8_lossy(&out).into_owned()
    }

    fn deny_private() -> DestinationCheck {
        DestinationCheck::new(|host| async move { Err(format!("{host} not allowed")) })
    }

    /// DNS rebinding: the name answers a public address once and loopback
    /// the next time. The proxy resolves once per request and connects to
    /// exactly what it classified, so the loopback answer is refused and no
    /// connection to loopback is ever made.
    #[tokio::test]
    async fn a_rebinding_name_never_reaches_loopback() {
        let lookups = Arc::new(AtomicUsize::new(0));
        let connected = Arc::new(parking_lot_free::Log::default());
        let (l, c) = (lookups.clone(), connected.clone());
        let net = Network::new(
            move |_host, _port| {
                let n = l.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(vec![if n == 0 {
                        IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))
                    } else {
                        IpAddr::V4(Ipv4Addr::LOCALHOST)
                    }])
                }
            },
            move |addr| {
                c.push(addr);
                async move { Ok(fake_upstream()) }
            },
        );
        let proxy = FilterProxy::start_with(deny_private(), net).await.unwrap();
        let req = "GET http://rebind.example/secret HTTP/1.1\r\nHost: rebind.example\r\nProxy-Connection: keep-alive\r\n\r\n";
        let first = send(proxy.addr(), req).await;
        assert!(first.starts_with("HTTP/1.1 200"), "{first}");
        let second = send(proxy.addr(), req).await;
        assert!(second.starts_with("HTTP/1.1 403"), "{second}");
        assert!(second.contains("loopback"), "{second}");
        let tunnel = send(proxy.addr(), "CONNECT rebind.example:443 HTTP/1.1\r\n\r\n").await;
        assert!(tunnel.starts_with("HTTP/1.1 403"), "{tunnel}");
        assert_eq!(
            connected.all(),
            vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
                80
            )],
            "only the address that was checked was ever connected"
        );
    }

    /// Loopback, metadata and odd spellings of them are refused before any
    /// lookup or connection; a private host needs the owner's allowance.
    #[tokio::test]
    async fn internal_addresses_are_refused_and_private_hosts_ask() {
        let connected = Arc::new(parking_lot_free::Log::default());
        let c = connected.clone();
        let net = Network::new(
            |host, _| async move {
                Ok(match host.as_str() {
                    "nas.home" => vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))],
                    _ => vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
                })
            },
            move |addr| {
                c.push(addr);
                async move { Ok(fake_upstream()) }
            },
        );
        let asked = Arc::new(AtomicUsize::new(0));
        let a = asked.clone();
        let policy = DestinationCheck::new(move |host| {
            a.fetch_add(1, Ordering::SeqCst);
            async move {
                if host == "nas.home" {
                    Ok(())
                } else {
                    Err("no".into())
                }
            }
        });
        let proxy = FilterProxy::start_with(policy, net).await.unwrap();
        for target in [
            "CONNECT 127.0.0.1:8123 HTTP/1.1\r\n\r\n",
            "CONNECT [::1]:443 HTTP/1.1\r\n\r\n",
            "CONNECT localhost:443 HTTP/1.1\r\n\r\n",
            "GET http://169.254.169.254/latest/meta-data/ HTTP/1.1\r\n\r\n",
            "GET http://[::ffff:127.0.0.1]:9/ HTTP/1.1\r\n\r\n",
            "GET ftp://example.com/ HTTP/1.1\r\n\r\n",
        ] {
            let out = send(proxy.addr(), target).await;
            assert!(out.starts_with("HTTP/1.1 4"), "{target}: {out}");
        }
        assert_eq!(connected.all(), Vec::<SocketAddr>::new());
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        let ok = send(proxy.addr(), "CONNECT nas.home:443 HTTP/1.1\r\n\r\n").await;
        assert!(
            ok.starts_with("HTTP/1.1 200 Connection Established"),
            "{ok}"
        );
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        let public = send(
            proxy.addr(),
            "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
        )
        .await;
        assert!(public.contains("ok"), "{public}");
    }

    #[test]
    fn chrome_cannot_bypass_or_resolve_on_its_own() {
        let args = FilterProxy::chrome_args(SocketAddr::from(([127, 0, 0, 1], 4567)));
        assert!(args.contains(&"--proxy-server=http://127.0.0.1:4567".to_owned()));
        assert!(args.contains(&"--proxy-bypass-list=<-loopback>".to_owned()));
        assert!(
            args.iter()
                .any(|a| a.starts_with("--host-resolver-rules=MAP * ~NOTFOUND"))
        );
    }

    /// A tiny thread-safe log for the tests (no extra dependency).
    mod parking_lot_free {
        use std::net::SocketAddr;
        use std::sync::Mutex;

        #[derive(Default)]
        pub struct Log(Mutex<Vec<SocketAddr>>);

        impl Log {
            pub fn push(&self, a: SocketAddr) {
                self.0.lock().unwrap().push(a);
            }
            pub fn all(&self) -> Vec<SocketAddr> {
                self.0.lock().unwrap().clone()
            }
        }
    }
}
