//! Review finding 4: OAuth traffic never goes through a configured proxy
//! (a proxy resolves targets itself, past any address check).
//!
//! The test re-runs this binary as a child with the proxy variables set, so
//! the process environment of other tests is never changed.  The parent owns
//! the fake proxy and counts what reaches it; the child owns the target.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CHILD: &str = "PRAXI_OAUTH_PROXY_CHILD";

/// Accepts connections, counts them and answers every request with `body`.
async fn listener(body: &'static str) -> (u16, Arc<AtomicUsize>) {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = socket.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (port, hits)
}

/// Child: one ordinary request (must use the proxy), then OAuth discovery
/// through the policy client (must not).
async fn child() {
    let (target_port, direct) = listener("{}").await;
    let base = format!("http://127.0.0.1:{target_port}");
    // Control for the parent: an ordinary client honours the variables.
    let _ = reqwest::Client::new()
        .get(format!("{base}/control"))
        .send()
        .await;
    let before = direct.load(Ordering::SeqCst);
    let client = Arc::new(mcp_bridge::oauth_net::PolicyHttpClient::new(true));
    let manager = rmcp::transport::auth::AuthorizationManager::new_with_oauth_http_client(
        format!("{base}/mcp").as_str(),
        client,
    )
    .await
    .unwrap();
    let _ = manager.resolve_metadata().await;
    assert!(
        direct.load(Ordering::SeqCst) > before,
        "the OAuth target was not contacted directly"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn oauth_requests_ignore_configured_proxies() {
    if std::env::var_os(CHILD).is_some() {
        child().await;
        return;
    }
    let (proxy_port, proxied) = listener("{}").await;
    let proxy = format!("http://127.0.0.1:{proxy_port}");
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "oauth_requests_ignore_configured_proxies",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy");
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env(key, &proxy);
    }
    let status = command.status().await.unwrap();
    assert!(status.success(), "child failed: {status}");
    // Exactly the control request reached the proxy: the variables were in
    // effect, and no OAuth request used them.
    assert_eq!(proxied.load(Ordering::SeqCst), 1);
}
