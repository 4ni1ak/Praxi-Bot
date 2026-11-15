//! Exercise cancellation through the actual stdio process, rmcp cancellation
//! notification and real loopback HTTP daemon. All computer work is a fake.

use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agent_core::{AgentLoop, ApprovalScope, LoopConfig, Providers, StreamItem};
use async_trait::async_trait;
use daemon_api::mcp_server::McpTokenGrant;
use daemon_api::{AppState, DaemonConfig};
use protocol::Event;
use protocol::event::ApprovalNeeded;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequest, CallToolRequestParams, ClientConfig, ProtocolVersion};
use rmcp::service::PeerRequestOptions;
use secret_store::{EncryptedFileStore, KdfParams};
use serde_json::{Value, json};
use tool_registry::{
    Effect, PermissionCategory, Platform, Tool, ToolContext, ToolError, ToolOutput, ToolRegistry,
    ToolSpec,
};
use uuid::Uuid;

const OWNER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const PASSPHRASE: &str = "test-only-cancellation-passphrase";
const PROFILE: &str = "[bot]\nid='cancel-test'\nname='Cancel test'\n[models]\nexecutor='fake/echo'\n[permissions]\npreset='full'\n";

struct FakeClick {
    spec: ToolSpec,
    hits: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for FakeClick {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn call(&self, _: &ToolContext, _: Value) -> Result<ToolOutput, ToolError> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::text("fake click completed"))
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    agent: AgentLoop,
    storage: Arc<storage::Storage>,
    user: protocol::UserId,
    url: String,
    bot: Uuid,
    grant: McpTokenGrant,
    hits: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<std::io::Result<Vec<protocol::RunId>>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(storage::Storage::open(dir.path()).unwrap());
        let registry = ToolRegistry::new();
        let hits = Arc::new(AtomicUsize::new(0));
        registry.register(Arc::new(FakeClick {
            spec: ToolSpec {
                name: "browser_click".into(),
                description: "In-memory click counter".into(),
                input_schema: json!({"type":"object"}),
                effect: Effect::Write,
                category: PermissionCategory::Browser,
                platforms: vec![Platform::All],
            },
            hits: Arc::clone(&hits),
        }));
        let agent = AgentLoop::new(
            Arc::clone(&storage),
            registry,
            Providers::single(Arc::new(daemon_api::fake::FakeProvider)),
            LoopConfig::default(),
        );
        let state = AppState::new(
            agent.clone(),
            Arc::clone(&storage),
            DaemonConfig::new(OWNER),
        )
        .unwrap();
        let store = EncryptedFileStore::new(dir.path().join("secrets.enc"), PASSPHRASE).with_kdf(
            KdfParams {
                m_cost: 8,
                t_cost: 1,
                p_cost: 1,
            },
        );
        state.set_secret_store(Arc::new(store));
        std::fs::write(dir.path().join("api-token"), "OWNER_FILE_MUST_NOT_BE_READ").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(daemon_api::serve(
            listener,
            state.clone(),
            std::future::pending(),
        ));
        let bot = owner_post(&url, "/v1/bots", json!({"profile_toml":PROFILE})).await;
        let bot: Uuid = serde_json::from_value(bot["id"].clone()).unwrap();
        let user = storage
            .get_bot(protocol::BotId(bot))
            .unwrap()
            .unwrap()
            .user_id;
        let grant = owner_post(
            &url,
            "/v1/mcp-server/tokens",
            json!({"bots":[bot],"scopes":["mcp:computer"],"ttl_seconds":3600}),
        )
        .await;
        Self {
            dir,
            agent,
            storage,
            user,
            url,
            bot,
            grant: serde_json::from_value(grant).unwrap(),
            hits,
            server,
        }
    }

    fn spawn_stdio(&self) -> tokio::process::Child {
        tokio::process::Command::new(env!("CARGO_BIN_EXE_bot"))
            .arg("--data-dir")
            .arg(self.dir.path())
            .args([
                "--secrets-backend",
                "file",
                "--url",
                &self.url,
                "mcp",
                "serve",
                "--bot",
                &self.bot.to_string(),
                "--allow",
                "computer",
                "--token-id",
                &self.grant.id.to_string(),
            ])
            .env_remove("BOT_API_TOKEN")
            .env("BOT_SECRETS_PASSPHRASE", PASSPHRASE)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }
}

async fn owner_post(base: &str, path: &str, body: Value) -> Value {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{base}{path}"))
        .bearer_auth(OWNER)
        .json(&body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn approval(rx: &mut tokio::sync::broadcast::Receiver<StreamItem>) -> ApprovalNeeded {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Event::ApprovalNeeded(approval) = rx.recv().await.unwrap().envelope.event {
                return approval;
            }
        }
    })
    .await
    .expect("fake computer call did not request approval")
}

async fn execute_approved(
    client: &rmcp::Peer<rmcp::RoleClient>,
    f: &Fixture,
    rx: &mut tokio::sync::broadcast::Receiver<StreamItem>,
    cancelled: protocol::RunId,
) {
    let (reply, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            client.call_tool(CallToolRequestParams::new("browser_click")),
            async {
                let allowed = approval(rx).await;
                assert_ne!(allowed.run_id, cancelled);
                f.agent
                    .approve(allowed.call_id, ApprovalScope::Once, f.user)
                    .unwrap();
            }
        )
    })
    .await
    .expect("the positive control must execute through the same MCP session");
    let reply = reply.unwrap();
    assert_ne!(reply.is_error, Some(true));
    assert!(
        serde_json::to_string(&reply)
            .unwrap()
            .contains("fake click completed")
    );
    assert_eq!(f.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_cancellation_cancels_live_http_approval_and_later_call_still_executes() {
    let f = Fixture::new().await;
    let mut rx = f.agent.subscribe();
    let mut child = f.spawn_stdio();
    let client = tokio::time::timeout(
        Duration::from_secs(10),
        ClientConfig::default()
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .serve((child.stdout.take().unwrap(), child.stdin.take().unwrap())),
    )
    .await
    .unwrap()
    .unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "browser_click");
    let request = client
        .send_cancellable_request(
            CallToolRequest::new(CallToolRequestParams::new("browser_click")).into(),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    let cancelled = approval(&mut rx).await;
    assert_eq!(f.hits.load(Ordering::SeqCst), 0);
    request
        .cancel(Some("test client cancelled this call".into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), f.agent.wait(cancelled.run_id))
        .await
        .expect("stdio cancellation left the HTTP approval alive")
        .unwrap();
    assert_eq!(
        f.storage.get_run(cancelled.run_id).unwrap().unwrap().state,
        "cancelled"
    );
    assert!(
        f.agent
            .approve(cancelled.call_id, ApprovalScope::Once, f.user)
            .is_err()
    );
    assert_eq!(f.hits.load(Ordering::SeqCst), 0);

    // The same live MCP session can still execute a different, approved call.
    execute_approved(&client, &f, &mut rx, cancelled.run_id).await;
    client.cancel().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}
