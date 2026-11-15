//! Real Composio adapters through the production loop and replay/subagent gates.
use super::*;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Audit;
impl composio::Audit for Audit {
    fn record(&self, _: &str, _: Value) -> Result<(), String> {
        Ok(())
    }
}

async fn connected_registry(env: &Env) -> (ToolRegistry, MockServer) {
    let server = MockServer::start().await;
    for (url, body) in [
        (
            "/toolkits",
            json!({"items":[{"slug":"github","name":"GitHub"}]}),
        ),
        ("/auth_configs", json!({"items":[{"id":"ac_install"}]})),
        (
            "/connected_accounts/link",
            json!({"redirect_url":"https://connect.composio.dev/link/fake","connected_account_id":"ca_install","expires_at":"2030-01-01"}),
        ),
        (
            "/connected_accounts",
            json!({"items":[{"id":"ca_install","toolkit":{"slug":"github"},"status":"ACTIVE"}]}),
        ),
        (
            "/tools",
            json!({"items":[{"slug":"GITHUB_LIST_REPOSITORY_ISSUES","toolkit":{"slug":"github"},"version":"20260901_00","description":"read issues","input_parameters":{"type":"object","properties":{"owner":{"type":"string"}}}}]}),
        ),
        (
            "/tools/execute/GITHUB_LIST_REPOSITORY_ISSUES",
            json!({"successful":true,"data":{}}),
        ),
    ] {
        Mock::given(path(url))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    let registry = ToolRegistry::new();
    let svc = composio::ComposioService::new(composio::ServiceConfig {
        data_dir: env.dir.path().to_owned(),
        keys: Arc::new(composio::LocalKeySource::new(Arc::new(
            secret_store::MemoryStore::new(),
        ))),
        registry: registry.clone(),
        redact: Arc::new(str::to_owned),
        register_secret: Arc::new(|_| {}),
        audit: Arc::new(Audit),
        loopback_base: Some(server.uri()),
    })
    .unwrap();
    svc.set_key("fake-offline-key").await.unwrap();
    svc.connect("github").await.unwrap();
    svc.app_status("github").await.unwrap();
    (registry, server)
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Keep the end-to-end policy/caller matrix together.
async fn composio_read_asks_for_all_policies_grants_and_callers() {
    for mode in [
        "full",
        "accounts_allow",
        "tool_grant",
        "category_grant",
        "chat_grant",
        "waiver",
        "schedule",
        "schedule_tool",
        "scheduled_replay",
        "subagent",
        "replay",
        "generic_run",
    ] {
        eprintln!("approval scenario: {mode}");
        let env = setup();
        let (registry, server) = connected_registry(&env).await;
        let direct = "app:github/list_repository_issues";
        let (name, args) = if mode == "generic_run" {
            (
                "apps_run_action",
                json!({"app":"github","action":"list_repository_issues","arguments":{"owner":"o"}}),
            )
        } else {
            (direct, json!({"owner":"o"}))
        };
        assert!(registry.get(name).unwrap().always_ask());
        let model_call = if name == "apps_run_action" {
            calls(&[("c1", name, args.clone())])
        } else {
            calls(&[("c1", "use_tool", json!({"name":name,"input":args}))])
        };
        let mut replies = vec![model_call, text("done")];
        if mode == "subagent" {
            replies = vec![
                calls(&[(
                    "spawn",
                    "spawn_subagent",
                    json!({"task":"read issues","context":"use the connected GitHub app"}),
                )]),
                calls(&[("child", "use_tool", json!({"name":name,"input":args}))]),
                text("child done"),
                text("parent done"),
            ];
        }
        let fake = Fake::new(replies);
        let mut policy = if matches!(
            mode,
            "full"
                | "subagent"
                | "replay"
                | "schedule"
                | "schedule_tool"
                | "scheduled_replay"
                | "generic_run"
        ) {
            Policy::full()
        } else {
            Policy::approve_destructive()
        };
        if mode == "accounts_allow" {
            policy
                .categories
                .insert(PermissionCategory::Accounts, Rule::new(Level::Allow));
        }
        let lp = AgentLoop::new(
            env.storage.clone(),
            registry,
            Providers::single(fake as Arc<dyn ChatProvider>),
            cfg(policy),
        )
        .with_subagents();
        lp.add_profile(env.bot, profile(""));
        {
            let mut grants = lp.sh.grants.lock();
            if matches!(mode, "tool_grant" | "category_grant") {
                grants.always.push(crate::Grant {
                    bot_id: env.bot,
                    target: if mode == "tool_grant" {
                        GrantTarget::Tool(name.into())
                    } else {
                        GrantTarget::Category(PermissionCategory::Accounts)
                    },
                    granted_at_ms: 1,
                    computer: None,
                    inert: false,
                });
            }
            if mode == "chat_grant" {
                grants.this_chat.insert((env.chat, name.into(), None));
            }
        }
        let store = workflow_replay::WorkflowStore::new(env.dir.path());
        let mut workflow = workflow_replay::Workflow::new("app_read");
        workflow.steps = vec![workflow_replay::Step::Tool(workflow_replay::ToolStep {
            tool: name.into(),
            input: args.clone(),
        })];
        store.save(&workflow).unwrap();
        let lp = lp.with_workflows(
            store,
            workflow_replay::Handles {
                desktop: None,
                browser: None,
                tools: ToolRegistry::new(),
            },
        );
        let mut rx = lp.subscribe();
        let run = match mode {
            "schedule" => lp
                .run_scheduled(
                    env.chat,
                    protocol::TaskId::new(),
                    crate::ScheduleWork::Prompt("read issues".into()),
                )
                .unwrap(),
            "schedule_tool" => lp
                .run_scheduled(
                    env.chat,
                    protocol::TaskId::new(),
                    crate::ScheduleWork::Tool {
                        name: "use_tool".into(),
                        input: json!({"name":name,"input":args}),
                    },
                )
                .unwrap(),
            "scheduled_replay" => lp
                .run_scheduled(
                    env.chat,
                    protocol::TaskId::new(),
                    crate::ScheduleWork::Workflow {
                        name: "app_read".into(),
                        params: std::collections::BTreeMap::default(),
                    },
                )
                .unwrap(),
            "replay" => lp.run_turn(env.chat, "/workflow app_read").unwrap(),
            _ => lp.run_turn(env.chat, "read issues").unwrap(),
        };
        let mut observed = Vec::new();
        let a = loop {
            let next = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await;
            let item = next
                .unwrap_or_else(|_| panic!("{mode}: no approval; events: {observed:?}"))
                .unwrap();
            if let Event::ApprovalNeeded(a) = item.envelope.event {
                break a;
            }
            observed.push(item.envelope.event);
        };
        assert_eq!(a.tool_name, name, "{mode}");
        assert_eq!(a.details.as_ref().unwrap()["kind"], "app_action");
        assert!(
            !server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.url.path().starts_with("/tools/execute")),
            "{mode}: executed before approval"
        );
        for scope in [
            ApprovalScope::Always,
            ApprovalScope::AlwaysCategory,
            ApprovalScope::ThisChat,
        ] {
            assert!(
                lp.approve(a.call_id, scope, env.user).is_err(),
                "{mode}: standing approval accepted"
            );
        }
        if mode == "waiver" {
            lp.set_global_policy(Policy::full());
        }
        lp.approve(a.call_id, ApprovalScope::Once, env.user)
            .unwrap();
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded, "{mode}");
        let sent = server.received_requests().await.unwrap();
        assert_eq!(
            sent.iter()
                .filter(|r| r.url.path().starts_with("/tools/execute"))
                .count(),
            1,
            "{mode}"
        );
    }
}
