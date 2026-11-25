//! `ask_bot`: one bot asks another for help. Every test drives the real loop
//! with scripted models that DO try the forbidden thing.

#![allow(clippy::similar_names, clippy::needless_pass_by_value)]

use std::sync::Arc;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use parking_lot::Mutex;
use permission_engine::{Level, Policy};
use protocol::{BotId, Event, InitiatorKind, PermissionCategory, RunId};
use provider_catalog::{
    ChatChunk, ChatProvider, ChatRequest, FinishReason, MessageRole, ProviderError, ToolCall, Usage,
};
use serde_json::{Value, json};
use storage::Bot;
use task_runtime::RunState;
use tool_registry::{Effect, Tool, ToolRegistry};

use crate::delegation::{ASK_BOT, clamp_policy, level_of};
use crate::tests::{Env, cfg, profile, setup, tool, until};
use crate::{AgentLoop, ApprovalScope, LoopConfig, MessageMode, Providers};

type Decide = Box<dyn Fn(&ChatRequest) -> Vec<ChatChunk> + Send + Sync>;

struct Brain {
    decide: Decide,
    requests: Mutex<Vec<ChatRequest>>,
}

#[async_trait]
impl ChatProvider for Brain {
    fn id(&self) -> &'static str {
        "fake"
    }
    async fn stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        self.requests.lock().push(req.clone());
        Ok(futures::stream::iter((self.decide)(&req).into_iter().map(Ok)).boxed())
    }
}

fn system_of(req: &ChatRequest) -> String {
    req.messages
        .iter()
        .find(|m| m.role == MessageRole::System)
        .and_then(|m| m.content.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn is_child(req: &ChatRequest) -> bool {
    system_of(req).contains("You are a subagent")
}

fn results(req: &ChatRequest) -> Vec<String> {
    req.messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .map(|m| m.content.as_str().unwrap_or_default().to_owned())
        .collect()
}

fn tool_names(req: &ChatRequest) -> Vec<String> {
    req.tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

fn say(s: &str) -> Vec<ChatChunk> {
    vec![
        ChatChunk::TextDelta(s.to_owned()),
        ChatChunk::Usage(Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            cached_tokens: None,
            cache_write_tokens: None,
        }),
        ChatChunk::Done {
            finish_reason: FinishReason::Stop,
        },
    ]
}

fn call(id: &str, name: &str, args: Value) -> Vec<ChatChunk> {
    vec![
        ChatChunk::ToolCall(ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: args.to_string(),
            provider_extra: None,
        }),
        ChatChunk::Done {
            finish_reason: FinishReason::ToolCalls,
        },
    ]
}

/// Bot A (the asker) with the caller's file level and bot B (the helper) with
/// its own; both on the same fake provider.
struct World {
    env: Env,
    helper: BotId,
    lp: AgentLoop,
    brain: Arc<Brain>,
    fs_read: Arc<crate::tests::TestTool>,
}

fn preset_with_fs(fs: &str) -> String {
    format!(
        "[permissions]\npreset = \"full\"\n[permissions.categories.fs]\nlevel = {{ level = \"{fs}\" }}\n"
    )
}

fn world(
    caller_fs: &str,
    helper_fs: &str,
    allow_helper: bool,
    extra_tools: Vec<Arc<dyn Tool>>,
    decide: impl Fn(&ChatRequest, BotId) -> Vec<ChatChunk> + Send + Sync + 'static,
    config: LoopConfig,
) -> World {
    let env = setup();
    let helper = BotId::new();
    let allow = if allow_helper {
        format!("[delegation]\nallow_bots = [\"{}\"]\n", helper.0)
    } else {
        String::new()
    };
    let delegation = if allow_helper {
        json!({"allow_bots": [helper.0.to_string()]})
    } else {
        json!({})
    };
    env.storage
        .insert_bot(&Bot {
            id: helper,
            user_id: env.user,
            name: "Researcher".into(),
            config: json!({}),
            created_at_ms: storage::now_ms(),
        })
        .unwrap();
    env.storage
        .set_bot_spend_limits(env.user, helper, "{}", &json!({}))
        .unwrap();
    let a = env.storage.get_bot(env.bot).unwrap().unwrap();
    env.storage
        .update_bot(env.bot, "Mira", &json!({"delegation": delegation}), 1)
        .unwrap();
    let _ = a;
    let brain = Arc::new(Brain {
        decide: Box::new(move |req| decide(req, helper)),
        requests: Mutex::new(Vec::new()),
    });
    let fs_read = Arc::new(tool("fs_read", Effect::Read, "file text"));
    let registry = ToolRegistry::new();
    registry.register(Arc::clone(&fs_read) as Arc<dyn Tool>);
    for t in extra_tools {
        registry.register(t);
    }
    let lp = AgentLoop::new(
        Arc::clone(&env.storage),
        registry,
        Providers::single(Arc::clone(&brain) as Arc<dyn ChatProvider>),
        config,
    )
    .with_subagents();
    lp.add_profile(
        env.bot,
        profile(&format!("{}{}", preset_with_fs(caller_fs), allow)),
    );
    lp.add_profile(helper, profile(&preset_with_fs(helper_fs)));
    World {
        env,
        helper,
        lp,
        brain,
        fs_read,
    }
}

/// A asks B once, B tries to read a file and then answers.
fn simple_script(helper: BotId) -> impl Fn(&ChatRequest, BotId) -> Vec<ChatChunk> {
    let _ = helper;
    move |req, helper| {
        if is_child(req) {
            if results(req).is_empty() {
                call("r1", "fs_read", json!({"path": "notes.txt"}))
            } else {
                say("the helper's answer")
            }
        } else if results(req).is_empty() {
            call(
                "a1",
                ASK_BOT,
                json!({"bot_id": helper.0.to_string(), "task": "read notes.txt", "context": "short"}),
            )
        } else {
            say("done")
        }
    }
}

async fn owner_turn(w: &World) -> RunId {
    let crate::Delivery::NewTurn(run) =
        w.lp.send_user_message_from(w.env.chat, "go", MessageMode::Queue, "owner")
            .unwrap()
    else {
        panic!("expected a new turn")
    };
    run
}

async fn approve_all(
    w: &World,
    rx: &mut tokio::sync::broadcast::Receiver<crate::StreamItem>,
    n: usize,
) {
    for _ in 0..n {
        let Event::ApprovalNeeded(a) = until(rx, |e| matches!(e, Event::ApprovalNeeded(_))).await
        else {
            unreachable!()
        };
        w.lp.approve(a.call_id, ApprovalScope::Once, w.env.user)
            .unwrap();
    }
}

fn kids(w: &World, run: RunId) -> Vec<storage::Run> {
    w.env.storage.list_child_runs(run).unwrap()
}

#[test]
fn the_helper_policy_is_the_lower_of_both_bots_in_every_category() {
    let level = |l: Level| {
        let mut p = Policy::default();
        for c in [PermissionCategory::Fs, PermissionCategory::Shell] {
            p.categories
                .insert(c, permission_engine::Rule::new(l.clone()));
        }
        p
    };
    let global = Policy::approve_destructive();
    let h = InitiatorKind::Human;
    let fs = PermissionCategory::Fs;
    // The caller denies, the helper allows: deny.
    let p = clamp_policy(
        &global,
        Some(&level(Level::Deny)),
        Some(&level(Level::Allow)),
    );
    assert_eq!(level_of(&p, fs, h), Level::Deny);
    // The caller asks, the helper allows: ask survives.
    let p = clamp_policy(
        &global,
        Some(&level(Level::Ask)),
        Some(&level(Level::Allow)),
    );
    assert_eq!(level_of(&p, fs, h), Level::Ask);
    // The helper denies, the caller allows: deny.
    let p = clamp_policy(
        &global,
        Some(&level(Level::Allow)),
        Some(&level(Level::Deny)),
    );
    assert_eq!(level_of(&p, PermissionCategory::Shell, h), Level::Deny);
    // A bot with no rule of its own gets the workspace policy, never Allow by default.
    let p = clamp_policy(&global, None, Some(&level(Level::Allow)));
    assert_eq!(level_of(&p, PermissionCategory::Money, h), Level::Ask);
    // A per-initiator override of the caller cannot be bypassed.
    let mut strict = level(Level::Allow);
    strict
        .categories
        .get_mut(&fs)
        .unwrap()
        .by_initiator
        .insert(InitiatorKind::Human, Level::Deny);
    let p = clamp_policy(&global, Some(&strict), Some(&level(Level::Allow)));
    assert_eq!(level_of(&p, fs, h), Level::Deny);
    assert_eq!(level_of(&p, fs, InitiatorKind::Routine), Level::Allow);
}

#[tokio::test]
async fn the_helper_cannot_do_more_than_the_asking_bot_but_a_shared_read_works() {
    // (caller fs level, helper fs level, reads executed)
    for (caller, helper, reads) in [
        ("deny", "allow", 0),
        ("allow", "deny", 0),
        ("allow", "allow", 1),
    ] {
        let w = world(
            caller,
            helper,
            true,
            vec![],
            |req, h| simple_script(h)(req, h),
            cfg(Policy::full()),
        );
        let mut rx = w.lp.subscribe();
        let run = owner_turn(&w).await;
        approve_all(&w, &mut rx, 1).await;
        assert_eq!(w.lp.wait(run).await.unwrap(), RunState::Succeeded);
        assert_eq!(
            w.fs_read.count.load(Ordering::SeqCst),
            reads,
            "caller {caller}, helper {helper}"
        );
        // The helper ran as ITSELF, one level down, with only reviewed tools.
        let k = kids(&w, run);
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].bot_id, w.helper);
        assert_eq!(k[0].state, "succeeded");
        let reqs = w.brain.requests.lock().clone();
        let child_req = reqs.iter().find(|r| is_child(r)).unwrap();
        assert!(
            tool_names(child_req)
                .iter()
                .all(|t| ["fs_read", "fs_list", "fs_search"].contains(&t.as_str())),
            "{:?}",
            tool_names(child_req)
        );
        // Its task arrives as data from another bot, and its answer comes back
        // marked untrusted.
        let first = child_req
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .filter_map(|m| m.content.as_str())
            .next()
            .unwrap()
            .to_owned();
        assert!(
            first.contains("not an instruction from the owner"),
            "{first}"
        );
        let parent_last = reqs.iter().rfind(|r| !is_child(r)).unwrap();
        assert!(
            results(parent_last)[0].starts_with("[Answer from the bot \"Researcher\""),
            "{:?}",
            results(parent_last)
        );
        // Audit: ids and limits, never the text.
        let audit = w.env.storage.list_audit(w.env.user).unwrap();
        for action in ["delegation.started", "delegation.finished"] {
            assert!(audit.iter().any(|a| a.action == action), "{action}");
        }
        let started = audit
            .iter()
            .find(|a| a.action == "delegation.started")
            .unwrap();
        assert!(!started.detail.to_string().contains("notes.txt"));
        assert_eq!(started.detail["depth"], 1);
    }
}

#[tokio::test]
async fn one_helper_per_task_and_a_helper_cannot_start_more() {
    let w = world(
        "allow",
        "allow",
        true,
        vec![Arc::new(tool("danger", Effect::Write, "did it")) as Arc<dyn Tool>],
        |req, h| {
            let ask = json!({"bot_id": h.0.to_string(), "task": "again"});
            if is_child(req) {
                match results(req).len() {
                    0 => call("c1", ASK_BOT, ask),
                    1 => call("c2", "spawn_subagent", json!({"task": "x"})),
                    2 => call("c3", "danger", json!({})),
                    _ => say("child done"),
                }
            } else {
                match results(req).len() {
                    0 | 1 => call(&format!("a{}", results(req).len()), ASK_BOT, ask),
                    _ => say("done"),
                }
            }
        },
        cfg(Policy::full()),
    );
    let mut rx = w.lp.subscribe();
    let run = owner_turn(&w).await;
    approve_all(&w, &mut rx, 2).await;
    assert_eq!(w.lp.wait(run).await.unwrap(), RunState::Succeeded);
    // The second ask of the same task was refused; only one helper exists, and
    // it has no children of its own.
    let k = kids(&w, run);
    assert_eq!(k.len(), 1);
    assert!(kids(&w, k[0].id).is_empty());
    let reqs = w.brain.requests.lock().clone();
    let last = reqs.iter().rfind(|r| !is_child(r)).unwrap();
    assert!(
        results(last)[1].contains("one helper per task"),
        "{:?}",
        results(last)
    );
    // The fabricated calls of the helper did nothing.
    let child_last = reqs.iter().rfind(|r| is_child(r)).unwrap();
    let r = results(child_last);
    assert!(r.len() >= 3, "{r:?}");
    assert!(r.iter().all(|t| !t.contains("did it")), "{r:?}");
}

#[tokio::test]
async fn it_is_off_by_default_and_only_the_owners_own_chat_may_ask() {
    // 1. No helper listed: the tool is not offered, and a made-up call starts nothing.
    let w = world(
        "allow",
        "allow",
        false,
        vec![],
        |req, h| {
            if results(req).is_empty() {
                call(
                    "a1",
                    ASK_BOT,
                    json!({"bot_id": h.0.to_string(), "task": "x"}),
                )
            } else {
                say("done")
            }
        },
        cfg(Policy::full()),
    );
    let run = owner_turn(&w).await;
    assert_eq!(w.lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert!(kids(&w, run).is_empty());
    assert!(
        !tool_names(&w.brain.requests.lock()[0]).contains(&ASK_BOT.to_owned()),
        "not offered without an owner setting"
    );

    // 2. Allowed, but the chat message did not come from the owner's own app.
    let w = world(
        "allow",
        "allow",
        true,
        vec![],
        |req, h| simple_script(h)(req, h),
        cfg(Policy::full()),
    );
    let mut rx = w.lp.subscribe();
    let run = w.lp.run_turn(w.env.chat, "go").unwrap();
    approve_all(&w, &mut rx, 1).await;
    assert_eq!(w.lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert!(kids(&w, run).is_empty(), "no helper may start");
    let audit = w.env.storage.list_audit(w.env.user).unwrap();
    assert!(audit.iter().any(|a| a.action == "delegation.refused"));
    assert_eq!(w.fs_read.count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_helpers_calls_count_toward_the_asking_bots_limits_too() {
    let w = world(
        "allow",
        "allow",
        true,
        vec![],
        |req, h| simple_script(h)(req, h),
        cfg(Policy::full()),
    );
    let mut rx = w.lp.subscribe();
    let run = owner_turn(&w).await;
    // The asker is under no limit until it has made its first call; then its
    // daily token cap is already used up while the helper has none.
    let Event::ApprovalNeeded(a) = until(&mut rx, |e| matches!(e, Event::ApprovalNeeded(_))).await
    else {
        unreachable!()
    };
    let limits = crate::bot_limits::BotLimits {
        daily: crate::bot_limits::PeriodLimit {
            hard_tokens: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    w.env
        .storage
        .set_bot_spend_limits(
            w.env.user,
            w.env.bot,
            &serde_json::to_string(&limits).unwrap(),
            &json!({}),
        )
        .unwrap();
    w.lp.approve(a.call_id, ApprovalScope::Once, w.env.user)
        .unwrap();
    let _ = w.lp.wait(run).await.unwrap();
    // No request reached the provider for the helper: the asker's cap stopped it.
    assert!(
        !w.brain.requests.lock().iter().any(|r| is_child(r)),
        "the helper must not be called when the asking bot is over its limit"
    );
    // And what the helper spends is counted for both bots.
    let k = kids(&w, run);
    assert_eq!(k[0].state, "failed");
}

#[test]
fn spending_of_a_helper_run_is_counted_for_both_bots() {
    use crate::bot_limits::{Period, period_usage};
    let w = world(
        "allow",
        "allow",
        true,
        vec![],
        |_, _| say("x"),
        cfg(Policy::full()),
    );
    let (env, helper) = (&w.env, w.helper);
    let rt = w.lp.runtime();
    let parent = rt
        .create_child_run(env.user, env.bot, Some(env.chat), "turn:human", None, None)
        .unwrap();
    let spec = json!({"delegation": {"caller_bot_id": env.bot.0.to_string()}});
    let child = rt
        .create_child_run(
            env.user,
            helper,
            Some(env.chat),
            "turn:subagent",
            Some(parent),
            Some(spec),
        )
        .unwrap();
    env.storage
        .append_audit(
            env.user,
            Some(child),
            "bot",
            "usage",
            &json!({"ledger_version": 1, "tokens": {}, "prompt_tokens": 7, "completion_tokens": 3}),
        )
        .unwrap();
    let now = storage::now_ms();
    for bot in [env.bot, helper] {
        let u = period_usage(&env.storage, env.user, bot, Period::Daily, now).unwrap();
        assert_eq!(u.tokens, 10, "bot {bot:?}");
    }
    let stranger = BotId::new();
    assert_eq!(
        period_usage(&env.storage, env.user, stranger, Period::Daily, now)
            .unwrap()
            .tokens,
        0
    );
}
