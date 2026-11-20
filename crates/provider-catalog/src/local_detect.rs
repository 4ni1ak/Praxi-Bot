//! Detects a local model server (Ollama, LM Studio, llama.cpp) listening on this machine.
//!
//! Only the loopback address `127.0.0.1` is ever contacted, on a short fixed list of default
//! ports, with a short timeout, no proxy, and no redirects. Nothing is sent but `GET /v1/models`.

use std::time::Duration;

use reqwest::redirect::Policy;
use serde::Serialize;
use serde_json::Value;

/// Per-probe timeout. A local server answers in milliseconds; this only bounds a dead port.
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// Largest `/v1/models` body read (1 MiB); a bigger answer is treated as "not detected".
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Most model ids kept per server.
const MAX_MODELS: usize = 500;
/// Longest model id kept, in characters.
const MAX_ID_CHARS: usize = 256;

/// A local runtime and the default port it listens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalRuntime {
    /// Provider id (a catalog id when the catalog has one).
    pub id: &'static str,
    /// Default TCP port.
    pub port: u16,
}

/// Default ports probed, in order. 8080 is the llama.cpp `llama-server` default.
pub const DEFAULT_RUNTIMES: [LocalRuntime; 3] = [
    LocalRuntime {
        id: "ollama",
        port: 11434,
    },
    LocalRuntime {
        id: "lmstudio",
        port: 1234,
    },
    LocalRuntime {
        id: "llamacpp",
        port: 8080,
    },
];

/// A local server that answered with a model list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalServer {
    /// Provider id (`ollama`, `lmstudio`, `llamacpp`).
    pub id: String,
    /// OpenAI-compatible base URL, e.g. `http://127.0.0.1:11434/v1`.
    pub base_url: String,
    /// Installed model ids (empty when the server runs with nothing installed).
    pub models: Vec<String>,
}

/// Probes the default ports on loopback and returns every server that answered.
pub async fn detect_local_servers() -> Vec<LocalServer> {
    detect_on(&DEFAULT_RUNTIMES).await
}

/// Probes the given runtimes on `127.0.0.1` (ports are injectable for tests).
pub async fn detect_on(runtimes: &[LocalRuntime]) -> Vec<LocalServer> {
    let Ok(client) = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .no_proxy()
        .redirect(Policy::none())
        .build()
    else {
        return Vec::new();
    };
    let probes = runtimes.iter().map(|r| probe(&client, *r));
    futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
        .collect()
}

async fn probe(client: &reqwest::Client, rt: LocalRuntime) -> Option<LocalServer> {
    let base_url = format!("http://127.0.0.1:{}/v1", rt.port);
    let mut resp = client.get(format!("{base_url}/models")).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_BODY_BYTES as u64)
    {
        return None;
    }
    // Read through a bounded loop: a chunked body has no content-length to check up front.
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if bytes.len() + chunk.len() > MAX_BODY_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&bytes).ok()?;
    if rt.id == "llamacpp" && !looks_like_llamacpp(&body) {
        return None;
    }
    let models = parse_models(&body)?;
    Some(LocalServer {
        id: rt.id.to_owned(),
        base_url,
        models,
    })
}

/// Port 8080 is a generic port, so a `llamacpp` answer must look like llama.cpp: `llama-server`
/// tags every entry of `/v1/models` with `"owned_by": "llamacpp"`. Conservative on purpose: an
/// empty or untagged list is not accepted.
fn looks_like_llamacpp(body: &Value) -> bool {
    body.get("data")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|m| m.get("owned_by").and_then(Value::as_str) == Some("llamacpp"))
        })
}

/// Model ids of an OpenAI-style `{"data":[{"id":..}]}` list; `None` when the body is not one
/// (so an unrelated web server on the same port is not mistaken for a model server). At most
/// [`MAX_MODELS`] ids of at most [`MAX_ID_CHARS`] characters are kept; the rest are dropped.
fn parse_models(body: &Value) -> Option<Vec<String>> {
    let items = body.get("data")?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .filter(|id| id.chars().count() <= MAX_ID_CHARS)
            .take(MAX_MODELS)
            .map(str::to_owned)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serves one canned HTTP response per connection on an ephemeral loopback port.
    async fn fake_server(status: &'static str, body: &'static str) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        port
    }

    async fn dead_port() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn finds_running_servers_with_their_models_and_skips_the_rest() {
        let up = fake_server("200 OK", r#"{"data":[{"id":"llama3.2:3b"},{"id":"qwen"}]}"#).await;
        let empty = fake_server("200 OK", r#"{"data":[]}"#).await;
        let not_models = fake_server("200 OK", r#"{"hello":"world"}"#).await;
        let broken = fake_server("500 Internal Server Error", "{}").await;
        let down = dead_port().await;
        let found = detect_on(&[
            LocalRuntime {
                id: "ollama",
                port: up,
            },
            LocalRuntime {
                id: "lmstudio",
                port: empty,
            },
            LocalRuntime {
                id: "llamacpp",
                port: not_models,
            },
            LocalRuntime {
                id: "x",
                port: broken,
            },
            LocalRuntime {
                id: "y",
                port: down,
            },
        ])
        .await;
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id, "ollama");
        assert_eq!(found[0].base_url, format!("http://127.0.0.1:{up}/v1"));
        assert_eq!(found[0].models, ["llama3.2:3b", "qwen"]);
        assert_eq!(found[1].id, "lmstudio");
        assert_eq!(found[1].models, Vec::<String>::new());
    }

    /// Like `fake_server` but with an owned body and extra header lines.
    async fn raw_server(headers: String, body: String) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(headers.as_bytes()).await;
                let _ = s.write_all(body.as_bytes()).await;
            }
        });
        port
    }

    fn rt(id: &'static str, port: u16) -> [LocalRuntime; 1] {
        [LocalRuntime { id, port }]
    }

    #[tokio::test]
    async fn oversized_body_is_not_detected() {
        let big = format!(
            r#"{{"data":[],"pad":"{}"}}"#,
            "a".repeat(MAX_BODY_BYTES + 10)
        );
        let hdr = format!(
            "HTTP/1.1 200 OK
content-length: {}
connection: close

",
            big.len()
        );
        let declared = raw_server(hdr, big.clone()).await;
        // Chunked: no content-length to check up front.
        let chunked = raw_server(
            "HTTP/1.1 200 OK
transfer-encoding: chunked
connection: close

"
            .into(),
            format!(
                "{:x}
{big}
0

",
                big.len()
            ),
        )
        .await;
        assert_eq!(detect_on(&rt("ollama", declared)).await.len(), 0);
        assert_eq!(detect_on(&rt("ollama", chunked)).await.len(), 0);
    }

    #[tokio::test]
    async fn redirect_is_not_followed() {
        let target = fake_server("200 OK", r#"{"data":[{"id":"x"}]}"#).await;
        let hdr = format!(
            "HTTP/1.1 302 Found
location: http://127.0.0.1:{target}/v1/models
content-length: 0
connection: close

"
        );
        let redirecting = raw_server(hdr, String::new()).await;
        assert_eq!(detect_on(&rt("ollama", redirecting)).await.len(), 0);
    }

    #[tokio::test]
    async fn port_8080_must_look_like_llamacpp() {
        let generic = fake_server("200 OK", r#"{"data":[]}"#).await;
        let other = fake_server("200 OK", r#"{"data":[{"id":"x","owned_by":"someone"}]}"#).await;
        let real = fake_server(
            "200 OK",
            r#"{"data":[{"id":"m.gguf","owned_by":"llamacpp"}]}"#,
        )
        .await;
        assert_eq!(detect_on(&rt("llamacpp", generic)).await.len(), 0);
        assert_eq!(detect_on(&rt("llamacpp", other)).await.len(), 0);
        let found = detect_on(&rt("llamacpp", real)).await;
        assert_eq!(found[0].models, ["m.gguf"]);
    }

    #[test]
    fn model_list_is_capped() {
        let items: Vec<Value> = (0..600)
            .map(|i| serde_json::json!({"id": format!("m{i}")}))
            .collect();
        let mut with_long = items.clone();
        with_long.insert(0, serde_json::json!({"id": "x".repeat(MAX_ID_CHARS + 1)}));
        let ids = parse_models(&serde_json::json!({ "data": with_long })).unwrap();
        assert_eq!(ids.len(), MAX_MODELS);
        assert_eq!(ids[0], "m0");
    }

    #[test]
    fn default_probes_are_the_documented_loopback_ports() {
        let ports: Vec<u16> = DEFAULT_RUNTIMES.iter().map(|r| r.port).collect();
        assert_eq!(ports, [11434, 1234, 8080]);
    }
}
