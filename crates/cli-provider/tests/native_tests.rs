//! Native-tools sessions against the hermetic fake CLI: one run per dialect family.
use async_trait::async_trait;
use cli_provider::{
    ApprovalLevel, Driver, PathEnv,
    runtime::{NativePermission, NativeTurn, ToolReply, Turn, TurnHost, run_native_turn},
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use tool_registry::{Effect, PermissionCategory, ToolSpec};

mod common;

struct Host {
    allow: bool,
    permissions: Mutex<Vec<NativePermission>>,
    text: Mutex<String>,
}
#[async_trait]
impl TurnHost for Host {
    fn text(&self, t: &str) {
        self.text.lock().push_str(t);
    }
    async fn call(&self, _: &str, _: Value) -> ToolReply {
        panic!("no Praxi tool is called in this test")
    }
    async fn approve(&self, _: &str, _: Value) -> bool {
        panic!("no Praxi tool approval in this test")
    }
    async fn permission(&self, request: NativePermission) -> bool {
        self.permissions.lock().push(request);
        self.allow
    }
}

fn turn() -> Turn {
    Turn {
        prompt: "private prompt".into(),
        system: "private system".into(),
        model: None,
        tools: vec![ToolSpec {
            name: "known.tool".into(),
            description: "known".into(),
            input_schema: json!({"type":"object"}),
            effect: Effect::Read,
            category: PermissionCategory::Fs,
            platforms: vec![],
        }],
    }
}

/// A `fixture` executable on a sealed PATH, a log folder and a workspace folder.
fn setup() -> (tempfile::TempDir, tempfile::TempDir, PathEnv) {
    let binary = Path::new(env!("CARGO_BIN_EXE_native-cli-fixture"));
    let dir = common::fixture_dir(binary);
    common::install_binary(
        binary,
        &dir.path()
            .join(format!("fixture{}", std::env::consts::EXE_SUFFIX)),
    );
    let env = PathEnv::for_test(dir.path().into())
        .with_env_var("FAKE_LOG", dir.path().display().to_string())
        .with_env_var("API_KEY", "SENTINEL_ENV_KEY")
        .with_env_var("MY_TOKEN", "SENTINEL_ENV_TOKEN");
    (dir, tempfile::tempdir().unwrap(), env)
}

async fn run(
    driver: Driver,
    level: ApprovalLevel,
    allow: bool,
) -> (tempfile::TempDir, tempfile::TempDir, Arc<Host>) {
    let (dir, workspace, env) = setup();
    let host = Arc::new(Host {
        allow,
        permissions: Mutex::new(Vec::new()),
        text: Mutex::new(String::new()),
    });
    let nt = NativeTurn {
        turn: turn(),
        driver,
        level,
        workspace: workspace.path().to_owned(),
    };
    tokio::time::timeout(
        Duration::from_secs(30),
        run_native_turn("fixture", &env, nt, host.clone(), CancellationToken::new()),
    )
    .await
    .expect("native turn timed out")
    .expect("native turn failed");
    (dir, workspace, host)
}

fn log(dir: &tempfile::TempDir, name: &str) -> String {
    std::fs::read_to_string(dir.path().join(name)).unwrap_or_else(|_| panic!("{name} not logged"))
}

fn same_dir(logged: &str, expected: &Path) -> bool {
    std::fs::canonicalize(logged).unwrap() == std::fs::canonicalize(expected).unwrap()
}

#[tokio::test]
async fn claude_stream_json_runs_in_workspace_with_its_own_tools() {
    for allow in [true, false] {
        let (dir, workspace, host) = run(Driver::Claude, ApprovalLevel::AcceptEdits, allow).await;
        let argv = log(&dir, "argv");
        let args: Vec<&str> = argv.lines().collect();
        let at = |flag: &str| args.iter().position(|a| *a == flag).map(|i| args[i + 1]);
        // The CLI's own tools stay on: no `--tools ""`, mode from the bot's level.
        assert!(!args.contains(&"--tools"));
        assert_eq!(at("--permission-mode"), Some("acceptEdits"));
        assert_eq!(at("--permission-prompt-tool"), Some("mcp__praxi__approve"));
        assert!(!argv.contains("private prompt"));
        assert!(same_dir(&log(&dir, "cwd"), workspace.path()));
        // Environment allowlist: no provider keys or tokens reach the CLI.
        let env = log(&dir, "env");
        assert!(
            !env.contains("API_KEY") && !env.contains("MY_TOKEN"),
            "{env}"
        );
        // The CLI's permission prompt became one Praxi request and was answered.
        let permissions = host.permissions.lock();
        assert_eq!(permissions.len(), 1);
        assert_eq!(permissions[0].tool, "Bash");
        assert!(permissions[0].summary.contains("touch note.txt"));
        assert!(log(&dir, "verdict").contains(if allow { "\"allow\"" } else { "\"deny\"" }));
        assert_eq!(host.text.lock().as_str(), "native done");
    }
}

#[tokio::test]
async fn acp_agent_permissions_follow_the_level_and_fs_is_refused() {
    // Ask: the request goes to Praxi and a deny selects the agent's reject option.
    let (dir, workspace, host) = run(Driver::OpenCode, ApprovalLevel::Ask, false).await;
    assert_eq!(host.permissions.lock().len(), 1);
    assert!(log(&dir, "permission_answer").contains("\"optionId\":\"no\""));
    assert!(log(&dir, "fs_answer").contains("\"error\""));
    assert_eq!(log(&dir, "argv"), "acp");
    let init: Value = serde_json::from_str(&log(&dir, "init")).unwrap();
    assert_eq!(init["clientCapabilities"]["fs"]["writeTextFile"], false);
    assert_eq!(init["clientCapabilities"]["terminal"], false);
    let new: Value = serde_json::from_str(&log(&dir, "session_new")).unwrap();
    assert!(same_dir(new["cwd"].as_str().unwrap(), workspace.path()));
    assert_eq!(new["mcpServers"][0]["name"], "praxi");
    assert_eq!(host.text.lock().as_str(), "native acp");
    // Full access: accepted without a card.
    let (dir, _w, host) = run(Driver::OpenCode, ApprovalLevel::Full, false).await;
    assert!(host.permissions.lock().is_empty());
    assert!(log(&dir, "permission_answer").contains("\"optionId\":\"yes\""));
}
