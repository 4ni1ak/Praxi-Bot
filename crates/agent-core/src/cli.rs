//! Whole-turn CLI handoff. Tool execution stays in `exec_tools`, including policy,
//! approvals, redaction, spills, timeouts and the existing run stream events.
use crate::{
    exec::{CliApproval, TurnExecutor},
    shared::{ModelOut, STEP_TOOLS, Shared, xe},
};
use async_trait::async_trait;
use cli_provider::runtime::{
    CliEngine, CliError, Engine, NativePermission, NativeTurn, ToolReply, Turn, TurnHost,
    run_native_turn,
};
use permission_engine::Decision;
use protocol::{Event, ToolCallId, event::MessageDelta};
use provider_catalog::{
    ChatRequest, MessageRole, ProviderConfig, Usage,
    config::{CliEngine as ConfigEngine, config_path},
};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::Arc;
use storage::Run;
use task_runtime::{ExecError, StepSpec};
use tokio_util::sync::CancellationToken;
use tool_registry::ToolSpec;

/// Upper bound of one native-tools turn (Stop and the kill switch end it sooner).
const NATIVE_TURN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

struct Host {
    sh: Arc<Shared>,
    run: Run,
    approved: parking_lot::Mutex<Vec<CliApproval>>,
}
#[async_trait]
impl TurnHost for Host {
    fn text(&self, text: &str) {
        if let Ok(ctx) = self.sh.ctx(&self.run) {
            self.sh.emit(
                &ctx,
                Event::MessageDelta(MessageDelta {
                    run_id: self.run.id,
                    text: self.sh.redact(text.to_owned()),
                }),
            );
        }
    }
    async fn call(&self, name: &str, input: Value) -> ToolReply {
        let exec = TurnExecutor {
            sh: self.sh.clone(),
        };
        let step = StepSpec {
            name: STEP_TOOLS.into(),
            input: json!({"calls":[{"id":ToolCallId::new().to_string(),"name":name,"arguments":input.to_string(),"repeat":1}]}),
            idempotent: false,
            retry: None,
        };
        let approval = {
            let resolved = self.sh.ctx(&self.run).ok().and_then(|ctx| {
                self.sh
                    .resolve_call(&ctx.profile, ctx.bot_id, name, &input, &[])
            });
            let mut approvals = self.approved.lock();
            approvals
                .iter()
                .position(|card| {
                    resolved
                        .as_ref()
                        .is_some_and(|r| card.name == r.name && card.input == r.input)
                })
                .map(|i| approvals.remove(i))
        };
        match exec.exec_tools_approved(&self.run, &step, approval).await {
            Ok(v) => ToolReply {
                text: v["results"][0]["text"]
                    .as_str()
                    .unwrap_or("tool failed")
                    .into(),
                is_error: v["results"][0]["is_error"].as_bool().unwrap_or(true),
            },
            Err(_) => ToolReply {
                text: "tool execution failed".into(),
                is_error: true,
            },
        }
    }
    async fn approve(&self, name: &str, input: Value) -> bool {
        let Ok(ctx) = self.sh.ctx(&self.run) else {
            return false;
        };
        let Some(r) = self
            .sh
            .resolve_call(&ctx.profile, ctx.bot_id, name, &input, &[])
        else {
            return false;
        };
        let Some(spec) = r.spec else {
            return false;
        };
        let decision = self
            .sh
            .decide(&ctx, &r.name, spec.effect, spec.category, &r.input);
        // An allow is not stored. Recording it would let a later ask, including
        // one that carries unevaluated limits, skip the interactive path.
        match &decision {
            Decision::Allow => true,
            Decision::Deny => false,
            Decision::Ask { .. } => {
                let honored_limits = decision.unsupported_limits().cloned();
                let Ok(details) = self.sh.approval_details(&ctx, &r.name, &r.input).await else {
                    return false;
                };
                let Ok(permit) = self
                    .sh
                    .request_approval(
                        &ctx,
                        ToolCallId::new(),
                        &r.name,
                        spec.effect,
                        spec.category,
                        &r.input,
                        details,
                    )
                    .await
                else {
                    return false;
                };
                let mut approved = self.approved.lock();
                // Bound stale approvals when a CLI never follows through with a call.
                // Dropping a permit records no grant.
                if approved.len() == 128 {
                    approved.remove(0);
                }
                approved.push(CliApproval {
                    name: r.name,
                    input: r.input,
                    permit,
                    honored_limits,
                });
                true
            }
        }
    }
}

impl Host {
    /// A permission request for the CLI's own tool becomes a normal approval card.
    /// The CLI's text is untrusted: it is shown, never executed or trusted, and
    /// the answer is a single allow or deny for exactly this request.
    async fn native_permission(&self, request: NativePermission) -> bool {
        let Ok(ctx) = self.sh.ctx(&self.run) else {
            return false;
        };
        let name = format!("cli.native.{}", sanitize_tool_label(&request.tool));
        let details = json!({
            "kind": "cli_native_action",
            "engine_label": ctx.profile.models.executor,
            "tool": request.tool,
            "summary": request.summary,
            "input": request.input,
        });
        // `Meta` is never denied by policy: the owner's level decides which requests reach
        // this card, and a click here is the whole decision (no grant is recorded).
        self.sh
            .request_approval(
                &ctx,
                ToolCallId::new(),
                &name,
                tool_registry::Effect::Write,
                protocol::PermissionCategory::Meta,
                &json!({"summary": request.summary, "input": request.input}),
                Some(details),
            )
            .await
            .is_ok()
    }
}

fn sanitize_tool_label(label: &str) -> String {
    let clean: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .take(32)
        .collect();
    if clean.is_empty() {
        "action".into()
    } else {
        clean
    }
}

struct RecordingHost {
    inner: Host,
    text: parking_lot::Mutex<String>,
}
#[async_trait]
impl TurnHost for RecordingHost {
    fn text(&self, text: &str) {
        self.text.lock().push_str(text);
        self.inner.text(text);
    }
    async fn call(&self, name: &str, input: Value) -> ToolReply {
        self.inner.call(name, input).await
    }
    async fn approve(&self, name: &str, input: Value) -> bool {
        self.inner.approve(name, input).await
    }
    async fn permission(&self, request: NativePermission) -> bool {
        self.inner.native_permission(request).await
    }
}

impl TurnExecutor {
    /// The settings of a native-tools bot, re-checked at every turn: consent on
    /// record and a workspace the protected-path rules still accept.
    fn native_turn(
        &self,
        engine: &crate::profile::EngineSettings,
        driver: cli_provider::Driver,
        turn: Turn,
    ) -> Result<NativeTurn, ExecError> {
        if engine.consent.native_tools_at_ms.is_none()
            || (engine.approval_level == cli_provider::ApprovalLevel::Full
                && engine.consent.full_access_at_ms.is_none())
        {
            return Err(xe(
                "the owner has not agreed to this engine setting; open the bot's engine settings",
            ));
        }
        let workspace = engine
            .workspace
            .as_deref()
            .ok_or_else(|| xe("this bot has no workspace folder"))?;
        let workspace = std::path::PathBuf::from(workspace);
        if let Some(why) = builtin_tools::workspace_refusal(&workspace, self.sh.storage.data_dir())
        {
            return Err(xe(format!("workspace refused: {why}")));
        }
        Ok(NativeTurn {
            turn,
            driver,
            level: engine.approval_level,
            workspace,
        })
    }

    #[allow(clippy::too_many_lines)] // Governed and native branches share the request assembly.
    pub(crate) async fn exec_cli(
        &self,
        run: &Run,
        req: &ChatRequest,
        specs: &[ToolSpec],
        cancel: &CancellationToken,
    ) -> Result<ModelOut, ExecError> {
        let cfg = ProviderConfig::load(&config_path(self.sh.storage.data_dir())).map_err(xe)?;
        let id = req.model.provider().unwrap_or(&req.model.0);
        let settings = cfg
            .get(id)
            .filter(|p| p.enabled)
            .ok_or_else(|| xe("CLI provider is not configured"))?;
        let cli = settings
            .cli
            .as_ref()
            .ok_or_else(|| xe("CLI provider is not configured"))?;
        let (cfg_engine, command) = cli.parts();
        let ctx = self.sh.ctx(run).map_err(xe)?;
        let engine_settings = ctx.profile.engine.clone();
        let native = engine_settings.mode == cli_provider::EngineMode::NativeTools;
        let host = Arc::new(RecordingHost {
            inner: Host {
                sh: self.sh.clone(),
                run: run.clone(),
                approved: parking_lot::Mutex::new(Vec::new()),
            },
            text: parking_lot::Mutex::new(String::new()),
        });
        let mut system = String::new();
        let mut prompt = String::new();
        for m in &req.messages {
            let text = m
                .content
                .as_str()
                .map_or_else(|| m.content.to_string(), str::to_owned);
            if m.role == MessageRole::System {
                system.push_str(&text);
                system.push('\n');
            } else {
                writeln!(&mut prompt, "{:?}: {text}", m.role).expect("string write");
            }
        }
        // MCP encodes names reversibly; permissions receive the original name.
        // In native mode the CLI has its own file, shell and script tools, so Praxi's
        // versions are not offered next to them (everything else is).
        let tools: Vec<ToolSpec> = specs
            .iter()
            .filter(|s| {
                !native
                    || !matches!(
                        s.category,
                        protocol::PermissionCategory::Fs
                            | protocol::PermissionCategory::Shell
                            | protocol::PermissionCategory::Script
                    )
            })
            .cloned()
            .collect();
        let model = cli_model_arg(&req.model).map_err(xe)?;
        let usage = if native {
            let driver = cli_provider::Driver::from_id(cfg_engine.as_str())
                .ok_or_else(|| xe("this CLI has no native-tools driver"))?;
            let nt = self.native_turn(
                &engine_settings,
                driver,
                Turn {
                    prompt,
                    system,
                    model,
                    tools,
                },
            )?;
            let token = cancel.child_token();
            let key = uuid::Uuid::now_v7();
            self.sh
                .native_runs
                .lock()
                .entry(ctx.bot_id)
                .or_default()
                .push((key, token.clone()));
            // Coding turns outlast a chat completion and wait on approval cards.
            let result = tokio::time::timeout(
                self.sh.cfg.guards.model_timeout.max(NATIVE_TURN_TIMEOUT),
                run_native_turn(
                    command,
                    &cli_provider::PathEnv::from_ambient(),
                    nt,
                    host.clone(),
                    token,
                ),
            )
            .await;
            if let Some(list) = self.sh.native_runs.lock().get_mut(&ctx.bot_id) {
                list.retain(|(k, _)| *k != key);
            }
            result
                .map_err(|_| xe("CLI turn timed out"))?
                .map_err(|e| cli_exec_error_for(driver.display_name(), driver.id(), &e))?
        } else {
            let engine = match cfg_engine {
                ConfigEngine::Claude => Engine::Claude,
                ConfigEngine::Codex => Engine::Codex,
                other => {
                    return Err(xe(format!(
                        "{} only runs with its own tools: set this bot's engine mode to \"native tools\" in its settings",
                        other.as_str()
                    )));
                }
            };
            let runtime = CliEngine {
                engine,
                command: command.into(),
                env: cli_provider::PathEnv::from_ambient(),
            };
            tokio::time::timeout(
                self.sh.cfg.guards.model_timeout,
                runtime.run_turn(
                    Turn {
                        prompt,
                        system,
                        model,
                        tools,
                    },
                    host.clone(),
                    cancel.clone(),
                ),
            )
            .await
            .map_err(|_| xe("CLI turn timed out"))?
            .map_err(|e| cli_exec_error(engine, &e))?
        };
        Ok(ModelOut {
            text: self.sh.redact(host.text.lock().clone()),
            model: req.model.0.clone(),
            usage: usage.reported.then_some(Usage {
                prompt_tokens: usage.input,
                completion_tokens: usage.output,
                cached_tokens: usage.cached,
                cache_write_tokens: None,
            }),
            finish: "Stop".into(),
            ..ModelOut::default()
        })
    }
}

fn cli_exec_error(engine: Engine, err: &CliError) -> ExecError {
    let (name, cmd) = match engine {
        Engine::Claude => ("Claude Code", "claude"),
        Engine::Codex => ("Codex CLI", "codex"),
    };
    cli_exec_error_for(name, cmd, err)
}

fn cli_exec_error_for(name: &str, cmd: &str, err: &CliError) -> ExecError {
    let code = err.code();
    let msg = match err {
        CliError::Cancelled => return ExecError(code.to_owned()),
        CliError::NotInstalled => format!(
            "{name} is not installed or needs an update. Install or update {name}, then sign in by running `{cmd}` in a terminal. ({code})"
        ),
        CliError::ShimRefused => format!(
            "{name} could not be launched because a wrapper shim was refused. Install {name} natively or put the real `{cmd}` binary on PATH. ({code})"
        ),
        CliError::SignedOut => format!(
            "{name} is signed out. Sign in by running `{cmd}` in a terminal, then try again. ({code})"
        ),
        CliError::Protocol => format!(
            "{name} ended unexpectedly or returned an unreadable response. Sign in by running `{cmd}` in a terminal, or update the CLI and try again. ({code})"
        ),
        CliError::NativeToolsOpen => format!(
            "{name} could not be started safely because required isolation flags are missing or unsupported. Update the CLI and try again. ({code})"
        ),
        CliError::Quota => format!(
            "{name} subscription usage limit reached. Wait for your quota to reset or switch providers. ({code})"
        ),
        CliError::Io => format!(
            "Could not communicate with {name}. Verify `{cmd}` runs in a terminal and try again. ({code})"
        ),
        CliError::BinaryChanged => format!(
            "The {name} binary changed while starting the turn. Try sending your message again. ({code})"
        ),
        CliError::CodexUnconfinable => format!(
            "The installed Codex cannot disable unified_exec, so Praxi will not run it. ({code})"
        ),
        CliError::WorkspaceUnavailable => format!(
            "The workspace folder chosen for this bot is missing. Choose it again in the bot's engine settings. ({code})"
        ),
    };
    ExecError(msg)
}

/// Validate a `--model` override destined for a subprocess CLI engine.
///
/// Rejects leading hyphens (flag injection), whitespace/control bytes, shell
/// metacharacters, empty strings, and values longer than 128 bytes.
pub(crate) fn validate_cli_model(model: &str) -> Result<(), String> {
    if model.is_empty() || model.len() > 128 {
        return Err(format!("invalid CLI model length: {model:?}"));
    }
    if model.starts_with('-') {
        return Err(format!("invalid CLI model starting with '-': {model:?}"));
    }
    if !model
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'))
    {
        return Err(format!(
            "CLI model contains disallowed characters: {model:?}"
        ));
    }
    Ok(())
}

fn cli_model_arg(model: &provider_catalog::ModelRef) -> Result<Option<String>, String> {
    let Some(m) = model
        .provider()
        .map(|_| model.model())
        .filter(|m| !m.is_empty() && *m != "default")
    else {
        return Ok(None);
    };
    validate_cli_model(m)?;
    Ok(Some(m.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{cfg, profile, setup, tool};
    use permission_engine::Policy;
    use std::sync::atomic::Ordering;
    use tool_registry::{Effect, ToolRegistry};
    #[tokio::test]
    async fn bridge_reuses_policy_and_tool_events() {
        for allow in [false, true] {
            let env = setup();
            let t = Arc::new(tool("test.read", Effect::Write, "bridge output"));
            let registry = ToolRegistry::new();
            registry.register(t.clone());
            let lp = crate::AgentLoop::new(
                env.storage.clone(),
                registry,
                crate::Providers::single(Arc::new(UnusedHttp)),
                cfg(if allow {
                    Policy::full()
                } else {
                    Policy::read_only()
                }),
            );
            lp.add_profile(env.bot, profile(""));
            // A normal completed HTTP turn supplies the durable run context;
            // the host then exercises precisely the bridge's permission path.
            let run_id = lp.run_turn(env.chat, "hi").unwrap();
            lp.wait(run_id).await.unwrap();
            let run = env.storage.get_run(run_id).unwrap().unwrap();
            let host = Host {
                sh: lp.sh.clone(),
                run,
                approved: parking_lot::Mutex::new(Vec::new()),
            };
            let reply = host.call("test.read", json!({})).await;
            assert_eq!(reply.is_error, !allow);
            assert_eq!(t.count.load(Ordering::SeqCst), usize::from(allow));
        }
    }
    struct UnusedHttp;
    #[async_trait]
    impl provider_catalog::ChatProvider for UnusedHttp {
        fn id(&self) -> &'static str {
            "fake"
        }
        async fn stream(
            &self,
            _: ChatRequest,
        ) -> Result<
            futures::stream::BoxStream<
                'static,
                Result<provider_catalog::ChatChunk, provider_catalog::ProviderError>,
            >,
            provider_catalog::ProviderError,
        > {
            Ok(Box::pin(futures::stream::iter([Ok(
                provider_catalog::ChatChunk::Done {
                    finish_reason: provider_catalog::FinishReason::Stop,
                },
            )])))
        }
        async fn list_models(&self) -> Result<Vec<String>, provider_catalog::ProviderError> {
            Ok(vec![])
        }
    }
    #[tokio::test]
    async fn cli_executor_does_not_call_http_for_missing_binary() {
        let env = setup();
        let registry = ToolRegistry::new();
        let lp = crate::AgentLoop::new(
            env.storage.clone(),
            registry,
            crate::Providers::single(Arc::new(PanicHttp)),
            cfg(Policy::full()),
        );
        let mut p = profile("");
        p.models.executor = "cli:claude".into();
        lp.add_profile(env.bot, p);
        let mut setting = provider_catalog::ProviderSettings::new("cli:claude", 0);
        setting.cli = Some(provider_catalog::config::CliSettings::Cli {
            engine: ConfigEngine::Claude,
            command: "praxi-c062-nonexistent-fixture".into(),
        });
        provider_catalog::ProviderConfig {
            quarantined: std::collections::BTreeMap::default(),
            providers: vec![setting],
            default_provider: None,
        }
        .save(&config_path(env.dir.path()))
        .unwrap();
        let run = lp.run_turn(env.chat, "private\n%PATH%").unwrap();
        assert_eq!(lp.wait(run).await.unwrap(), task_runtime::RunState::Failed);
        let events = storage::read_event_log(env.dir.path(), run).unwrap();
        assert!(format!("{events:?}").contains("not_installed"));
        assert!(format!("{events:?}").contains("Install or update Claude Code"));
    }

    #[test]
    fn cli_exec_error_includes_actionable_guidance_and_stable_code() {
        for (err, expected_code, expected_phrase) in [
            (
                CliError::NotInstalled,
                "not_installed",
                "Install or update Claude Code",
            ),
            (
                CliError::ShimRefused,
                "shim_refused",
                "Install Claude Code natively",
            ),
            (
                CliError::SignedOut,
                "signed_out",
                "Sign in by running `claude` in a terminal",
            ),
            (CliError::Protocol, "cli_protocol_error", "update the CLI"),
        ] {
            let out = cli_exec_error(Engine::Claude, &err);
            assert!(out.0.contains(expected_code), "{err:?} -> {}", out.0);
            assert!(out.0.contains(expected_phrase), "{err:?} -> {}", out.0);
        }
    }
    #[test]
    fn cli_model_arg_omits_default_placeholder() {
        assert_eq!(
            cli_model_arg(&provider_catalog::ModelRef("cli:claude".into())),
            Ok(None)
        );
        assert_eq!(
            cli_model_arg(&provider_catalog::ModelRef("cli:claude/default".into())),
            Ok(None)
        );
        assert_eq!(
            cli_model_arg(&provider_catalog::ModelRef("cli:claude/sonnet".into())),
            Ok(Some("sonnet".into()))
        );
        assert_eq!(
            cli_model_arg(&provider_catalog::ModelRef(
                "cli:claude/claude-sonnet-4-5:latest".into()
            )),
            Ok(Some("claude-sonnet-4-5:latest".into()))
        );
    }

    #[test]
    fn cli_model_arg_rejects_flags_and_unsafe_chars() {
        let too_long = format!("cli:claude/{}", "a".repeat(129));
        for bad in [
            "cli:claude/--dangerously-skip-permissions",
            "cli:claude/-p",
            "cli:claude/sonnet --flag",
            "cli:claude/sonnet\n--evil",
            "cli:claude/sonnet;id",
            too_long.as_str(),
        ] {
            assert!(
                cli_model_arg(&provider_catalog::ModelRef(bad.into())).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    struct PanicHttp;
    #[async_trait]
    impl provider_catalog::ChatProvider for PanicHttp {
        fn id(&self) -> &'static str {
            "fake"
        }
        async fn stream(
            &self,
            _: ChatRequest,
        ) -> Result<
            futures::stream::BoxStream<
                'static,
                Result<provider_catalog::ChatChunk, provider_catalog::ProviderError>,
            >,
            provider_catalog::ProviderError,
        > {
            panic!("CLI turn called HTTP")
        }
        async fn list_models(&self) -> Result<Vec<String>, provider_catalog::ProviderError> {
            Ok(vec![])
        }
    }
}
