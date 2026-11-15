//! Native-tools sessions: the CLI keeps its OWN file and shell tools and its own
//! permission mode (chosen from the bot's approval level, see `levels.rs`).
//!
//! What stays Praxi's: the process launch (pinned binary, allowlisted
//! environment, kill-on-close process tree), the workspace folder, Praxi's own
//! tools served over the per-turn MCP bridge, and the approval cards for every
//! permission request the CLI forwards. Everything the CLI prints is untrusted.
use super::*;
use crate::levels::{ApprovalLevel, Driver, LevelPlan, Transport, acp_args, level_plan};
use std::path::PathBuf;

/// One native-tools turn.
pub struct NativeTurn {
    pub turn: Turn,
    pub driver: Driver,
    pub level: ApprovalLevel,
    /// Existing folder chosen by the owner; the CLI's working directory.
    pub workspace: PathBuf,
}

/// Size limits for a CLI that is not trusted (same scale as the provider limits).
const MAX_LINE: usize = 1024 * 1024;
const MAX_TOTAL: usize = 32 * 1024 * 1024;

/// One-line description of a permission request for the approval card.
pub(super) fn describe(tool: &str, input: &Value) -> String {
    let detail = ["command", "file_path", "path", "url", "pattern"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .unwrap_or("");
    let text = format!("{tool} {detail}");
    text.chars().take(300).collect::<String>().trim().to_owned()
}

/// Run one turn with the CLI's own tools. Kills the process tree even if the
/// future is dropped.
///
/// # Errors
/// Stable [`CliError`] codes; CLI quotas are never retried.
pub async fn run_native_turn(
    command: &str,
    env: &PathEnv,
    nt: NativeTurn,
    host: Arc<dyn TurnHost>,
    cancel: CancellationToken,
) -> Result<TurnUsage, CliError> {
    let stop = cancel.child_token();
    let _guard = CancelOnDrop(stop.clone());
    let command = command.to_owned();
    let env = env.clone();
    tokio::spawn(async move { session(&command, &env, nt, host, stop).await })
        .await
        .map_err(|_| CliError::Io)?
}

async fn native_version(
    driver: Driver,
    resolved: &crate::detect::ResolvedCommand,
    env: &PathEnv,
) -> Result<Option<(u32, u32, u32)>, CliError> {
    match driver {
        Driver::Claude => Ok(crate::detect::claude_version(resolved, env).await),
        Driver::Codex => crate::detect::codex_version(resolved, env).await,
        _ => Ok(None),
    }
}

/// `--permission-mode` value this Claude accepts for the level. Newer builds
/// renamed `default` to `manual`; a mode the CLI does not list is refused, never
/// replaced by a wider one.
fn claude_mode(plan: &LevelPlan, help: &str) -> Result<String, CliError> {
    let modes = permission_modes(help);
    let wanted = plan.claude_mode.ok_or(CliError::NativeToolsOpen)?;
    let name = if wanted == "default" && !modes.iter().any(|m| m == "default") {
        "manual"
    } else {
        wanted
    };
    if modes.iter().any(|m| m == name) {
        Ok(name.to_owned())
    } else {
        Err(CliError::NativeToolsOpen)
    }
}

fn claude_native_args(
    turn: &Turn,
    plan: &LevelPlan,
    mode: &str,
    config: &std::path::Path,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--input-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-mode",
        mode,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    // Praxi's tools are the only MCP servers. They gate themselves, so the
    // server is pre-allowed; the CLI's own tools follow `mode`.
    args.extend([
        "--mcp-config".into(),
        config.to_string_lossy().into_owned(),
        "--strict-mcp-config".into(),
        "--allowedTools".into(),
        "mcp__praxi".into(),
    ]);
    if plan.applied != ApprovalLevel::Full {
        // Prompts for the CLI's own tools come to Praxi as approval cards.
        args.extend([
            "--permission-prompt-tool".into(),
            "mcp__praxi__approve".into(),
        ]);
    }
    if let Some(model) = &turn.model {
        args.extend(["--model".into(), model.clone()]);
    }
    args
}

fn codex_native_args(turn: &Turn) -> Vec<String> {
    let mut args = vec!["app-server".to_owned()];
    if let Some(model) = &turn.model {
        args.extend([
            "-c".into(),
            format!("model={}", serde_json::to_string(model).expect("string")),
        ]);
    }
    args
}

#[allow(clippy::too_many_lines)] // Mirrors `session`: one linear, security-critical launch.
async fn session(
    command: &str,
    env: &PathEnv,
    nt: NativeTurn,
    host: Arc<dyn TurnHost>,
    stop: CancellationToken,
) -> Result<TurnUsage, CliError> {
    let driver = nt.driver;
    let spec = crate::registry::engine_spec(driver.id()).ok_or(CliError::NotInstalled)?;
    let candidate = std::iter::once(command)
        .chain(spec.aliases.iter().copied())
        .find_map(|name| find_candidate(name, env))
        .ok_or(CliError::NotInstalled)?;
    let resolved = resolve_spawn(&candidate, env).map_err(|_| CliError::ShimRefused)?;
    if !nt.workspace.is_dir() {
        return Err(CliError::WorkspaceUnavailable);
    }
    #[cfg(windows)]
    let (resolved, executable_lock) = crate::detect::lock_executable(resolved)?;
    #[cfg(windows)]
    let job = crate::detect::SharedJob::new();
    let bridge = Bridge {
        capability: format!("Bearer {}{}", uuid::Uuid::now_v7(), uuid::Uuid::now_v7()),
        tools: Arc::new(nt.turn.tools.clone()),
        host: host.clone(),
        stop: stop.clone(),
        native: true,
        #[cfg(not(windows))]
        child_pid: Arc::new(AtomicU32::new(0)),
        #[cfg(windows)]
        job: job.clone(),
        listen: Arc::new(OnceLock::new()),
    };
    let server = BridgeServer::open(bridge.clone()).await?;
    if stop.is_cancelled() {
        return Err(CliError::Cancelled);
    }
    let pin = binary_pin(&resolved.program).ok_or(CliError::NotInstalled)?;
    let version = native_version(driver, &resolved, env).await?;
    let plan = level_plan(driver, nt.level);
    let mut claude_mode_arg = String::new();
    if driver == Driver::Claude {
        let help = crate::detect::claude_help(&resolved, env)
            .await
            .ok_or(CliError::NativeToolsOpen)?;
        claude_mode_arg = claude_mode(&plan, &help)?;
        if plan.applied != ApprovalLevel::Full && !help.contains("--permission-prompt-tool") {
            return Err(CliError::NativeToolsOpen);
        }
    }
    if binary_pin(&resolved.program).as_ref() != Some(&pin)
        || native_version(driver, &resolved, env).await? != version
    {
        return Err(CliError::BinaryChanged);
    }
    let argv = match driver.transport() {
        Transport::StreamJson => {
            claude_native_args(&nt.turn, &plan, &claude_mode_arg, server.file.path())
        }
        Transport::CodexAppServer => codex_native_args(&nt.turn),
        Transport::Acp => {
            acp_args(driver, &plan, nt.turn.model.as_deref()).ok_or(CliError::NativeToolsOpen)?
        }
    };
    if resolved.program.to_string_lossy().encode_utf16().count()
        + resolved
            .args
            .iter()
            .chain(&argv)
            .map(|a| a.encode_utf16().count() * 2 + 3)
            .sum::<usize>()
        > 30_000
    {
        return Err(CliError::Protocol);
    }

    let mut cmd = tokio::process::Command::new(&resolved.program);
    cmd.args(&resolved.args)
        .args(argv)
        .current_dir(&nt.workspace);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Allowlisted environment: no Praxi secrets and no provider keys.
    apply_sanitized_env(&mut cmd, env);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    #[cfg(windows)]
    let mut isolated = isolate(cmd, job);
    #[cfg(not(windows))]
    let mut isolated = isolate(cmd);
    let child = isolated.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CliError::NotInstalled
        } else {
            CliError::Io
        }
    })?;
    #[cfg(windows)]
    drop(executable_lock);
    let mut child = ChildTree::new(child);
    #[cfg(not(windows))]
    {
        let child_pid = child
            .id()
            .filter(|pid| *pid != 0)
            .ok_or(CliError::NativeToolsOpen)?;
        server.allow_child(child_pid);
    }

    let mut stdin = child.stdin().take().ok_or(CliError::Io)?;
    let stdout = child.stdout().take().ok_or(CliError::Io)?;
    let mut stderr = child.stderr().take().ok_or(CliError::Io)?;
    // Ring buffer only used to recognise a signed-out CLI; never surfaced.
    let stderr_task = tokio::spawn(async move {
        let mut ring = std::collections::VecDeque::with_capacity(4096);
        let mut buf = [0; 1024];
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 {
                break;
            }
            for b in &buf[..n] {
                if ring.len() == 4096 {
                    ring.pop_front();
                }
                ring.push_back(*b);
            }
        }
        ring
    });
    let run = Run {
        nt: &nt,
        plan: &plan,
        server: &server,
        host: host.as_ref(),
        bridge: &bridge,
    };
    let result = tokio::select! {
        () = stop.cancelled() => Err(CliError::Cancelled),
        r = run.protocol(&mut stdin, stdout) => r,
    };
    stop.cancel();
    child.terminate().await;
    if matches!(result, Err(CliError::Protocol)) {
        let abort_handle = stderr_task.abort_handle();
        if let Ok(Ok(ring)) =
            tokio::time::timeout(std::time::Duration::from_millis(50), stderr_task).await
        {
            let (head, tail) = ring.as_slices();
            let mut combined = Vec::with_capacity(ring.len());
            combined.extend_from_slice(head);
            combined.extend_from_slice(tail);
            if is_signed_out_text(&String::from_utf8_lossy(&combined)) {
                return Err(CliError::SignedOut);
            }
        } else {
            abort_handle.abort();
        }
        return result;
    }
    stderr_task.abort();
    result
}

struct Run<'a> {
    nt: &'a NativeTurn,
    plan: &'a LevelPlan,
    server: &'a BridgeServer,
    host: &'a dyn TurnHost,
    bridge: &'a Bridge,
}

/// Bounded line reader shared by the three dialects.
struct Frames {
    reader: BufReader<tokio::process::ChildStdout>,
    total: usize,
}
impl Frames {
    async fn next(&mut self) -> Result<Value, CliError> {
        loop {
            let mut bytes = Vec::new();
            let n = (&mut self.reader)
                .take(MAX_LINE as u64 + 1)
                .read_until(b'\n', &mut bytes)
                .await?;
            self.total += n;
            if n == 0 || n > MAX_LINE || self.total > MAX_TOTAL {
                return Err(CliError::Protocol);
            }
            if bytes.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
                if is_signed_out_text(&String::from_utf8_lossy(&bytes)) {
                    return Err(CliError::SignedOut);
                }
                return Err(CliError::Protocol);
            };
            return Ok(v);
        }
    }
}

impl Run<'_> {
    /// A request the CLI forwarded for one of its own tools. Full access
    /// answers without a card; every other level asks the owner.
    async fn allowed(&self, request: NativePermission) -> bool {
        self.plan.applied == ApprovalLevel::Full || self.host.permission(request).await
    }

    async fn protocol(
        &self,
        stdin: &mut tokio::process::ChildStdin,
        stdout: tokio::process::ChildStdout,
    ) -> Result<TurnUsage, CliError> {
        let frames = Frames {
            reader: BufReader::new(stdout),
            total: 0,
        };
        match self.nt.driver.transport() {
            Transport::StreamJson => self.claude(stdin, frames).await,
            Transport::CodexAppServer => self.codex(stdin, frames).await,
            Transport::Acp => self.acp(stdin, frames).await,
        }
    }

    fn prompt_text(&self) -> String {
        format!("{}\n\n{}", self.nt.turn.system, self.nt.turn.prompt)
    }

    async fn claude(
        &self,
        stdin: &mut tokio::process::ChildStdin,
        mut frames: Frames,
    ) -> Result<TurnUsage, CliError> {
        send(
            stdin,
            json!({"type":"user","message":{"role":"user","content":self.prompt_text()}}),
        )
        .await?;
        let mut usage = TurnUsage::default();
        let mut partial = false;
        loop {
            let v = frames.next().await?;
            if v.get("error").is_some() || v["is_error"] == true || v["method"] == "error" {
                return Err(child_error(&v));
            }
            match v["type"].as_str().unwrap_or("") {
                "stream_event" => {
                    let e = &v["event"];
                    if e["type"] == "message_start" {
                        partial = false;
                    }
                    if e["delta"]["type"] == "text_delta"
                        && let Some(t) = e["delta"]["text"].as_str()
                    {
                        partial = true;
                        self.host.text(t);
                    }
                }
                "assistant" => {
                    if !partial && let Some(blocks) = v["message"]["content"].as_array() {
                        for block in blocks {
                            if block["type"] == "text"
                                && let Some(text) = block["text"].as_str()
                            {
                                self.host.text(text);
                            }
                        }
                    }
                    partial = false;
                }
                "result" => {
                    if v["subtype"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("error"))
                    {
                        return Err(child_error(&v));
                    }
                    read_usage(&v["usage"], &mut usage);
                    return Ok(usage);
                }
                // `system`, `user` (tool results), rate-limit notices...: the CLI's own business.
                _ => {}
            }
        }
    }

    async fn codex(
        &self,
        stdin: &mut tokio::process::ChildStdin,
        mut frames: Frames,
    ) -> Result<TurnUsage, CliError> {
        let policy = self.plan.codex.ok_or(CliError::NativeToolsOpen)?;
        let cwd = self.nt.workspace.to_string_lossy().into_owned();
        send(
            stdin,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "clientInfo":{"name":"praxi","version":"0.1.0"},"capabilities":{}}}),
        )
        .await?;
        let mut usage = TurnUsage::default();
        let mut thread: Option<String> = None;
        loop {
            let v = frames.next().await?;
            if v.get("method").is_none() && (v.get("error").is_some() || v["is_error"] == true) {
                return Err(child_error(&v));
            }
            if v.get("method").is_none() {
                match v["id"].as_i64() {
                    Some(1) => {
                        send(
                            stdin,
                            json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
                        )
                        .await?;
                        send(
                            stdin,
                            json!({"jsonrpc":"2.0","id":2,"method":"thread/start","params":{
                                "model":self.nt.turn.model,
                                "baseInstructions":self.nt.turn.system,
                                "cwd":cwd,
                                "approvalPolicy":policy.approval_policy,
                                "approvalsReviewer":policy.reviewer,
                                "sandbox":policy.sandbox,
                                "config":{"mcp_servers":{"praxi":{
                                    "url":self.server.url,
                                    "http_headers":{"Authorization":self.server.authorization}}}}
                            }}),
                        )
                        .await?;
                    }
                    Some(2) => {
                        let id = v["result"]["thread"]["id"]
                            .as_str()
                            .ok_or(CliError::Protocol)?;
                        // The requested mode must be the one Codex applied.
                        let applied = &v["result"]["sandbox"]["type"];
                        if applied != policy.sandbox_policy
                            || v["result"]
                                .get("approvalPolicy")
                                .is_some_and(|p| p != policy.approval_policy)
                        {
                            return Err(CliError::NativeToolsOpen);
                        }
                        thread = Some(id.to_owned());
                        send(
                            stdin,
                            json!({"jsonrpc":"2.0","id":3,"method":"turn/start","params":{
                                "threadId":id,"cwd":cwd,
                                "input":[{"type":"text","text":self.nt.turn.prompt}],
                                "approvalPolicy":policy.approval_policy,
                                "approvalsReviewer":policy.reviewer,
                                "sandboxPolicy":{"type":policy.sandbox_policy}}}),
                        )
                        .await?;
                    }
                    _ => {}
                }
                continue;
            }
            let id = v["id"].clone();
            let p = &v["params"];
            match v["method"].as_str().unwrap_or("") {
                "item/agentMessage/delta" => {
                    if thread.is_none() {
                        return Err(CliError::Protocol);
                    }
                    if let Some(t) = p["delta"].as_str() {
                        self.host.text(t);
                    }
                }
                "thread/tokenUsage/updated" => read_usage(&p["tokenUsage"]["last"], &mut usage),
                "turn/completed" => {
                    if thread.is_none() || p["turn"]["status"] != "completed" {
                        return Err(child_error(&v));
                    }
                    return Ok(usage);
                }
                "item/mcpToolCall/requestApproval" if p["server"] == "praxi" => {
                    // Praxi's own tool: Praxi's permission engine decides, as in the confined mode.
                    let name = p["tool"]
                        .as_str()
                        .and_then(|n| self.bridge.resolve(n))
                        .ok_or(CliError::Protocol)?;
                    let allow = self.host.approve(name, p["arguments"].clone()).await;
                    send(stdin, json!({"jsonrpc":"2.0","id":id,"result":{"decision":if allow {"accept"} else {"decline"}}})).await?;
                }
                "item/commandExecution/requestApproval"
                | "item/fileChange/requestApproval"
                | "item/permissions/requestApproval"
                | "item/mcpToolCall/requestApproval"
                | "execCommandApproval"
                | "applyPatchApproval" => {
                    let method = v["method"].as_str().unwrap_or("");
                    let legacy = matches!(method, "execCommandApproval" | "applyPatchApproval");
                    let tool = if method.contains("fileChange") || method == "applyPatchApproval" {
                        "edit"
                    } else if method.contains("permissions") {
                        "permissions"
                    } else if method.contains("mcpToolCall") {
                        "mcp"
                    } else {
                        "shell"
                    };
                    let input = json!({"command":p["command"],"cwd":p["cwd"],"reason":p["reason"],"server":p["server"],"tool":p["tool"]});
                    let summary = describe(tool, &input);
                    let allow = self
                        .allowed(NativePermission {
                            tool: tool.into(),
                            summary,
                            input,
                        })
                        .await;
                    let result = if tool == "permissions" {
                        json!({"permissions":if allow {p["permissions"].clone()} else {json!({})},"scope":"turn"})
                    } else if legacy {
                        json!({"decision":if allow {"approved"} else {"denied"}})
                    } else {
                        json!({"decision":if allow {"accept"} else {"decline"}})
                    };
                    send(stdin, json!({"jsonrpc":"2.0","id":id,"result":result})).await?;
                }
                "mcpServer/elicitation/request" => {
                    send(
                        stdin,
                        json!({"jsonrpc":"2.0","id":id,"result":{"action":"decline"}}),
                    )
                    .await?;
                }
                m if !id.is_null() => {
                    let _ = m;
                    send(stdin, json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unsupported request"}})).await?;
                }
                _ => {}
            }
        }
    }

    async fn acp(
        &self,
        stdin: &mut tokio::process::ChildStdin,
        mut frames: Frames,
    ) -> Result<TurnUsage, CliError> {
        let cwd = self.nt.workspace.to_string_lossy().into_owned();
        // fs and terminal stay with the agent: Praxi does not offer to be its file system or shell.
        send(
            stdin,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":1,
                "clientInfo":{"name":"praxi","version":"0.1.0"},
                "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}}),
        )
        .await?;
        let mut session: Option<String> = None;
        loop {
            let v = frames.next().await?;
            if let Some(method) = v["method"].as_str() {
                let id = v["id"].clone();
                match method {
                    "session/update" => {
                        let u = &v["params"]["update"];
                        if u["sessionUpdate"] == "agent_message_chunk"
                            && u["content"]["type"] == "text"
                            && let Some(t) = u["content"]["text"].as_str()
                        {
                            self.host.text(t);
                        }
                    }
                    "session/request_permission" => {
                        let tc = &v["params"]["toolCall"];
                        let title = tc["title"].as_str().unwrap_or("tool");
                        let kind = tc["kind"].as_str().unwrap_or(title);
                        let input = if tc["rawInput"].is_null() {
                            tc.clone()
                        } else {
                            tc["rawInput"].clone()
                        };
                        let allow = self
                            .allowed(NativePermission {
                                tool: kind.into(),
                                summary: describe(title, &input),
                                input,
                            })
                            .await;
                        let outcome = acp_outcome(&v["params"]["options"], allow);
                        send(
                            stdin,
                            json!({"jsonrpc":"2.0","id":id,"result":{"outcome":outcome}}),
                        )
                        .await?;
                    }
                    _ if !id.is_null() => {
                        // fs/* and terminal/* were not offered: refuse, keep the session.
                        send(stdin, json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"not supported by this client"}})).await?;
                    }
                    _ => {}
                }
                continue;
            }
            if v.get("error").is_some() {
                if v["error"]["code"] == -32000
                    || v["error"]["message"]
                        .as_str()
                        .is_some_and(|m| m.to_ascii_lowercase().contains("auth"))
                {
                    return Err(CliError::SignedOut);
                }
                return Err(child_error(&v));
            }
            match v["id"].as_i64() {
                Some(1) => {
                    let http = v["result"]["agentCapabilities"]["mcpCapabilities"]["http"] == true;
                    let servers = if http && !self.nt.turn.tools.is_empty() {
                        json!([{"type":"http","name":"praxi","url":self.server.url,
                            "headers":[{"name":"Authorization","value":self.server.authorization}]}])
                    } else {
                        json!([])
                    };
                    send(stdin, json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":cwd,"mcpServers":servers}})).await?;
                }
                Some(2) => {
                    let id = v["result"]["sessionId"]
                        .as_str()
                        .ok_or(CliError::Protocol)?;
                    session = Some(id.to_owned());
                    send(
                        stdin,
                        json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{
                        "sessionId":id,"prompt":[{"type":"text","text":self.prompt_text()}]}}),
                    )
                    .await?;
                }
                Some(3) => {
                    if session.is_none() {
                        return Err(CliError::Protocol);
                    }
                    return Ok(TurnUsage::default());
                }
                _ => {}
            }
        }
    }
}

/// ACP permission answer: an allow or reject option the agent offered, else cancelled.
fn acp_outcome(options: &Value, allow: bool) -> Value {
    let want = if allow { "allow" } else { "reject" };
    let list = options.as_array().map_or(&[][..], Vec::as_slice);
    let pick = list
        .iter()
        .find(|o| o["kind"] == format!("{want}_once"))
        .or_else(|| {
            list.iter()
                .find(|o| o["kind"].as_str().is_some_and(|k| k.starts_with(want)))
        })
        .and_then(|o| o["optionId"].as_str());
    match pick {
        Some(option_id) => json!({"outcome":"selected","optionId":option_id}),
        None => json!({"outcome":"cancelled"}),
    }
}
