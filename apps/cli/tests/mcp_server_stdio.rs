//! Launch the actual stdio CLI against a fake HTTP daemon. The poisoned owner
//! token file makes any accidental owner-client construction fail the test.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use daemon_api::mcp_server::{McpScope, McpTokenGrant};
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use secret_store::{EncryptedFileStore, KdfParams, SecretStore};
use serde_json::json;
use tokio::io::AsyncReadExt;
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PASSPHRASE: &str = "test-only-store-passphrase";

fn seed_credential(data: &Path, bot: Uuid, id: Uuid, token: &str) {
    let store = EncryptedFileStore::new(data.join("secrets.enc"), PASSPHRASE).with_kdf(KdfParams {
        m_cost: 8,
        t_cost: 1,
        p_cost: 1,
    });
    store.set(&format!("mcp-server/token/{id}"), token).unwrap();
    store
        .set(
            &format!("mcp-server/meta/{id}"),
            &serde_json::to_string(&McpTokenGrant {
                id,
                bots: vec![bot],
                scopes: BTreeSet::from([McpScope::Tasks]),
                expires_at_ms: storage::now_ms() + 60_000,
            })
            .unwrap(),
        )
        .unwrap();
}

async fn fake_daemon(bot: Uuid, token: &str) -> MockServer {
    let daemon = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/mcp-server/tools"))
        .and(header("authorization", format!("Bearer {token}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "name":"task_start","description":"Fake enabled task", "read_only":false,
            "untrusted_output":true,"input_schema":{"type":"object"}
        }])))
        .expect(1)
        .mount(&daemon)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/mcp-server/call"))
        .and(header("authorization", format!("Bearer {token}")))
        .and(body_json(
            json!({"bot":bot,"name":"task_start","arguments":{"text":"test task"}}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"run":"fake-run","echo":token})),
        )
        .expect(1)
        .mount(&daemon)
        .await;
    daemon
}

#[tokio::test]
async fn actual_mcp_stdio_process_uses_scoped_store_credential_without_owner_token() {
    let data = tempfile::tempdir().unwrap();
    let bot = Uuid::now_v7();
    let id = Uuid::now_v7();
    let token = format!("praxi-mcp.{id}.{}", "a".repeat(64));
    seed_credential(data.path(), bot, id, &token);
    let poison = "OWNER_TOKEN_MUST_NOT_BE_READ";
    std::fs::write(data.path().join("api-token"), poison).unwrap();
    let daemon = fake_daemon(bot, &token).await;

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_bot"))
        .arg("--data-dir")
        .arg(data.path())
        .args([
            "--secrets-backend",
            "file",
            "--url",
            &daemon.uri(),
            "mcp",
            "serve",
            "--bot",
            &bot.to_string(),
            "--token-id",
            &id.to_string(),
        ])
        .env_remove("BOT_API_TOKEN")
        .env("BOT_SECRETS_PASSPHRASE", PASSPHRASE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = tokio::time::timeout(
        Duration::from_secs(10),
        ().serve((child.stdout.take().unwrap(), child.stdin.take().unwrap())),
    )
    .await
    .expect("actual stdio handshake timed out")
    .expect("MCP CLI must initialize without reading the invalid owner token");
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "task_start");
    let result = client
        .call_tool(
            CallToolRequestParams::new("task_start")
                .with_arguments(serde_json::from_value(json!({"text":"test task"})).unwrap()),
        )
        .await
        .unwrap();
    let result = serde_json::to_string(&result).unwrap();
    assert!(result.contains("fake-run"));
    assert!(result.contains("UNTRUSTED"));
    assert!(!result.contains(&token));
    assert!(!result.contains(poison));
    client.cancel().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .await
        .unwrap();
    assert!(!stderr.contains(&token));
    assert!(!stderr.contains(poison));
    assert_eq!(
        std::fs::read_to_string(data.path().join("api-token")).unwrap(),
        poison
    );
    daemon.verify().await;
}
