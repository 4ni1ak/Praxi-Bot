//! Real rmcp framing over pipes and an HTTP fake daemon; no desktop or account.

use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

const TOKEN: &str = "praxi-mcp.00000000-0000-0000-0000-000000000001.scoped-test-credential";

fn catalog(name: &str) -> Value {
    json!([{"name":name,"description":"Fake enabled task", "read_only":false,
        "untrusted_output":true,"input_schema":{"type":"object","properties":{"text":{"type":"string"}}}}])
}

#[tokio::test]
async fn stdio_protocol_initializes_lists_calls_and_refuses_disabled_tools() {
    let daemon = MockServer::start().await;
    let bot = Uuid::now_v7();
    let reflected = json!({"run":"fake-run","untrusted":true,"echo_credential":TOKEN})
        .to_string()
        .replace("praxi-mcp.", r"\u0070raxi-mcp.");
    Mock::given(method("GET"))
        .and(path("/v1/mcp-server/tools"))
        .and(query_param("bot", bot.to_string()))
        .and(query_param("computer", "false"))
        .and(header("authorization", format!("Bearer {TOKEN}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(catalog("task_start")))
        .expect(1)
        .mount(&daemon)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/mcp-server/call"))
        .and(header("authorization", format!("Bearer {TOKEN}")))
        .and(body_json(
            json!({"bot":bot,"name":"task_start","arguments":{"text":"external task"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(reflected))
        .expect(1)
        .mount(&daemon)
        .await;
    let server = McpServer::connect(
        Client::scoped(&daemon.uri(), TOKEN.into()).unwrap(),
        bot,
        &BTreeSet::from([McpScope::Tasks]),
    )
    .await
    .unwrap();
    let (server_pipe, client_pipe) = tokio::io::duplex(16 * 1024);
    let (read, write) = tokio::io::split(server_pipe);
    let (client, server) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(
            ().serve(client_pipe),
            server.serve((FrameLimit::new(read), write))
        )
    })
    .await
    .unwrap();
    let client = client.unwrap();
    let server = server.unwrap();
    let listed = client.list_all_tools().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "task_start");
    let reply = client
        .call_tool(
            CallToolRequestParams::new("task_start")
                .with_arguments(serde_json::from_value(json!({"text":"external task"})).unwrap()),
        )
        .await
        .unwrap();
    let encoded = serde_json::to_string(&reply).unwrap();
    assert!(encoded.contains("fake-run"));
    assert!(encoded.contains("UNTRUSTED"));
    assert!(!encoded.contains(TOKEN));
    let blocked = client
        .call_tool(CallToolRequestParams::new("browser_eval"))
        .await
        .unwrap();
    assert_eq!(blocked.is_error, Some(true));
    client.cancel().await.unwrap();
    server.cancel().await.unwrap();
    daemon.verify().await;
}

#[tokio::test]
async fn computer_layer_is_explicit_and_remote_error_content_never_reaches_mcp() {
    let daemon = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/mcp-server/tools"))
        .and(query_param("computer", "true"))
        .and(query_param("tasks", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(catalog("desktop_snapshot")))
        .expect(1)
        .mount(&daemon)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/mcp-server/call"))
        .respond_with(ResponseTemplate::new(403).set_body_string(format!("unsafe error {TOKEN}")))
        .expect(1)
        .mount(&daemon)
        .await;
    let server = McpServer::connect(
        Client::scoped(&daemon.uri(), TOKEN.into()).unwrap(),
        Uuid::now_v7(),
        &BTreeSet::from([McpScope::Computer]),
    )
    .await
    .unwrap();
    let (a, b) = tokio::io::duplex(16 * 1024);
    let (client, server) = tokio::join!(().serve(a), server.serve(b));
    let client = client.unwrap();
    let server = server.unwrap();
    assert_eq!(
        client.list_all_tools().await.unwrap()[0].name,
        "desktop_snapshot"
    );
    let reply = client
        .call_tool(CallToolRequestParams::new("desktop_snapshot"))
        .await
        .unwrap();
    assert_eq!(reply.is_error, Some(true));
    let encoded = serde_json::to_string(&reply).unwrap();
    assert!(encoded.contains("403"));
    assert!(!encoded.contains(TOKEN));
    assert!(!encoded.contains("unsafe error"));
    client.cancel().await.unwrap();
    server.cancel().await.unwrap();
}

#[tokio::test]
async fn frame_and_http_byte_budgets_are_enforced() {
    let (mut writer, reader) = tokio::io::duplex(128 * 1024);
    writer.write_all(&vec![b'x'; 65 * 1024]).await.unwrap();
    drop(writer);
    let mut reader = FrameLimit::new(reader);
    assert_eq!(
        reader
            .read_to_end(&mut Vec::new())
            .await
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    let (mut writer, reader) = tokio::io::duplex(128 * 1024);
    writer.write_all(&vec![b'\n'; 65 * 1024]).await.unwrap();
    drop(writer);
    let mut reader = FrameLimit::new(reader);
    assert_eq!(
        reader.read_to_end(&mut Vec::new()).await.unwrap(),
        65 * 1024
    );
    let daemon = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(129 * 1024)))
        .mount(&daemon)
        .await;
    let client = Client::scoped(&daemon.uri(), TOKEN.into()).unwrap();
    let error = client
        .mcp_request(Method::GET, "/v1/mcp-server/tools", None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("byte budget"));
    assert!(
        client
            .mcp_request(Method::GET, "/v1/secrets", None)
            .await
            .is_err()
    );
}

#[test]
fn clap_defaults_to_tasks_and_only_accepts_explicit_layers_and_safe_ttl() {
    let bot = Uuid::now_v7().to_string();
    let cli = Cli::try_parse_from(["bot", "mcp", "serve", "--bot", &bot]).unwrap();
    let Some(crate::Command::Mcp {
        action: McpCmd::Serve(args),
    }) = cli.command
    else {
        panic!("mcp command")
    };
    assert_eq!(args.allow, vec![Layer::Tasks]);
    assert!(
        Cli::try_parse_from(["bot", "mcp", "serve", "--bot", &bot, "--allow", "shell"]).is_err()
    );
    assert_eq!(parse_ttl("30d").unwrap(), 30 * 86_400);
    for ttl in ["0", "59s", "31d", "99999999999999999999d", "-1", "garbage"] {
        assert!(parse_ttl(ttl).is_err());
    }
    for origin in [
        "https://example.com",
        "http://127.0.0.1@example.com",
        "http://example.com",
        "http://127.0.0.1/path",
        "http://127.0.0.1?token=secret",
    ] {
        assert!(Client::scoped(origin, TOKEN.into()).is_err());
    }
    assert!(Client::scoped("http://127.0.0.1:47821", "owner-token".into()).is_err());
}

#[test]
fn general_secret_cli_cannot_read_or_replace_scoped_credentials() {
    use secret_store::SecretStore;
    let store = secret_store::MemoryStore::new();
    let name = "mcp-server/token/test";
    store.set(name, TOKEN).unwrap();
    let mut names = Vec::new();
    crate::commands::secrets_list(&store, &mut |v| names.push(v.to_owned())).unwrap();
    assert!(!names.join(" ").contains("mcp-server"));
    assert!(
        crate::commands::secrets_set(&store, name, &secret_store::SecretValue::new("replacement"))
            .is_err()
    );
    assert!(crate::commands::secrets_delete(&store, name).is_err());
    assert_eq!(store.get(name).unwrap().unwrap().expose(), TOKEN);
}
