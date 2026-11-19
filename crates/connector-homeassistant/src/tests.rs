//! Offline tests against an in-process fake hub.

use std::sync::{Arc, Mutex};

use secret_store::{MemoryStore, SecretRef, SecretStore};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tool_registry::{ContentPart, ToolContext, ToolError, ToolRegistry};

use crate::{HomeAssistantConfig, register};

/// `(request line, authorization header, body)` of each request seen.
type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

async fn fake_hub() -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, body) = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if let Some((h, b)) = text.split_once("\r\n\r\n") {
                        let len = h
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if b.len() >= len {
                            break (h.to_owned(), b.to_owned());
                        }
                    }
                };
                let request = head.lines().next().unwrap_or("").to_owned();
                let auth = head
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("")
                    .to_owned();
                log.lock().unwrap().push((request.clone(), auth, body));
                let payload = if request.starts_with("GET /api/states/light.kitchen") {
                    json!({"entity_id":"light.kitchen","state":"off","attributes":{"friendly_name":"Kitchen"}})
                } else if request.starts_with("GET /api/states ") {
                    json!([
                        {"entity_id":"light.kitchen","state":"off","attributes":{"friendly_name":"Kitchen"}},
                        {"entity_id":"sensor.temp","state":"21","attributes":{}}
                    ])
                } else if request.starts_with("POST /api/services/") {
                    json!([{"entity_id":"light.kitchen","state":"on"}])
                } else {
                    json!({})
                }
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (port, seen)
}

fn registry_for(port: u16, allowed: &[&str]) -> ToolRegistry {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    store.set("homeassistant/token", "ha-secret-token").unwrap();
    let cfg = HomeAssistantConfig {
        url: format!("http://127.0.0.1:{port}"),
        token: SecretRef::new("homeassistant/token").unwrap(),
        allowed_domains: allowed.iter().map(|s| (*s).to_owned()).collect(),
        timeout_secs: 5,
    };
    let reg = ToolRegistry::new();
    let (added, problem) = register(&reg, Some(cfg), &store);
    assert_eq!((added, problem), (3, None));
    reg
}

async fn call(reg: &ToolRegistry, name: &str, input: Value) -> Result<Value, ToolError> {
    let out = reg
        .get(name)
        .unwrap()
        .call(&ToolContext::default(), input)
        .await?;
    match out.content.into_iter().next() {
        Some(ContentPart::Json { data }) => Ok(data),
        other => panic!("unexpected output {other:?}"),
    }
}

#[tokio::test]
async fn reads_and_service_call_use_the_stored_token() {
    let (port, seen) = fake_hub().await;
    let reg = registry_for(port, &[]);

    let list = call(&reg, "ha_list_entities", json!({"domain":"light"}))
        .await
        .unwrap();
    assert_eq!(list["untrusted_external_content"], true);
    assert_eq!(list["returned"], 1);
    assert_eq!(list["entities"][0]["name"], "Kitchen");

    let st = call(&reg, "ha_get_state", json!({"entity_id":"light.kitchen"}))
        .await
        .unwrap();
    assert_eq!(st["state"]["state"], "off");

    call(
        &reg,
        "ha_call_service",
        json!({"domain":"light","service":"turn_on","entity_id":"light.kitchen","data":{"brightness_pct":40}}),
    )
    .await
    .unwrap();
    let log = seen.lock().unwrap();
    let (line, auth, body) = log.last().unwrap();
    assert!(
        line.starts_with("POST /api/services/light/turn_on"),
        "{line}"
    );
    assert!(auth.ends_with("Bearer ha-secret-token"), "{auth}");
    let body: Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["entity_id"], "light.kitchen");
    assert_eq!(body["brightness_pct"], 40);
}

#[tokio::test]
async fn code_running_and_unlisted_services_never_reach_the_hub() {
    let (port, seen) = fake_hub().await;
    let reg = registry_for(port, &["light"]);
    for (domain, service) in [
        ("shell_command", "x"),
        ("homeassistant", "restart"),
        ("lock", "unlock"),
    ] {
        let err = call(
            &reg,
            "ha_call_service",
            json!({"domain":domain,"service":service}),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidInput(_)),
            "{domain}.{service}: {err}"
        );
    }
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn public_plain_http_is_refused_and_tools_are_gated() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let cfg = HomeAssistantConfig {
        url: "http://ha.example.com:8123".into(),
        token: SecretRef::new("homeassistant/token").unwrap(),
        allowed_domains: vec![],
        timeout_secs: 5,
    };
    let reg = ToolRegistry::new();
    let (added, problem) = register(&reg, Some(cfg), &store);
    assert_eq!(added, 3);
    assert!(problem.unwrap().contains("must be https"));
    let specs = reg.list();
    let effect = |n: &str| specs.iter().find(|s| s.name == n).unwrap().effect;
    assert_eq!(
        effect("ha_call_service"),
        tool_registry::Effect::Destructive
    );
    assert_eq!(effect("ha_get_state"), tool_registry::Effect::Read);
}
