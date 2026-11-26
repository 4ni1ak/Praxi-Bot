//! MCP elicitation: a tool (standing in for an MCP tool call through the bridge)
//! asks the run's owner through the loop's `Elicitor`. The answer reaches the
//! tool and nothing else.

use std::fmt::Write as _;

use tool_registry::{ElicitationAnswer, ElicitationRequest, Elicitor};

use super::*;
use crate::ElicitationRefusal;
use crate::account_register_tests::LogCapture;
use protocol::event::{ElicitationField, ElicitationFieldType};

const CANARY: &str = "canary-elicit-Tr0ub4dor-9921";

/// What the server would have received.
type Seen = Arc<Mutex<Vec<ElicitationAnswer>>>;

struct FormTool {
    spec: ToolSpec,
    elicitor: Arc<dyn Elicitor>,
    seen: Seen,
}

#[async_trait]
impl Tool for FormTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    async fn call(&self, ctx: &ToolContext, _: Value) -> Result<ToolOutput, ToolError> {
        let mut field = ElicitationField {
            name: "token".into(),
            title: Some("Token".into()),
            description: None,
            kind: ElicitationFieldType::String,
            required: true,
            options: Vec::new(),
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            format: None,
            default: None,
        };
        field.max_length = Some(64);
        let answer = self
            .elicitor
            .elicit(
                &ctx.run_id,
                ElicitationRequest {
                    server: "acme".into(),
                    message: "Paste your Acme token".into(),
                    fields: vec![field],
                },
            )
            .await;
        // Like a server that does not echo what it got.
        let text = match &answer {
            ElicitationAnswer::Accept(_) => "accepted",
            ElicitationAnswer::Decline => "declined",
            ElicitationAnswer::Cancel => "cancelled",
        };
        self.seen.lock().push(answer);
        Ok(ToolOutput::text(text))
    }
}

fn rig(env: &Env, fake: &Arc<Fake>, config: LoopConfig) -> (AgentLoop, Seen) {
    let lp = make_loop(&env.storage, env.bot, fake, &[], profile(""), config);
    let seen = Seen::default();
    lp.sh.tools.register(Arc::new(FormTool {
        spec: ToolSpec {
            name: "acme_login".into(),
            description: "test MCP tool that asks for input".into(),
            input_schema: json!({"type": "object"}),
            effect: Effect::Read,
            category: PermissionCategory::Fs,
            platforms: vec![Platform::All],
            content: ContentClass::none(),
        },
        elicitor: lp.elicitor(),
        seen: Arc::clone(&seen),
    }));
    (lp, seen)
}

fn script() -> Arc<Fake> {
    Fake::new(vec![calls(&[("c1", "acme_login", json!({}))]), text("ok")])
}

fn requested(e: &Event) -> bool {
    matches!(e, Event::ElicitationRequested(_))
}

fn content(token: &str) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("token".into(), json!(token));
    m
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one ordered scenario: run, then scan every sink
async fn the_owner_answers_and_only_the_server_gets_the_value() {
    let log = Arc::new(Mutex::new(String::new()));
    let _guard = tracing::subscriber::set_default(LogCapture(Arc::clone(&log)));
    let env = setup();
    let fake = script();
    let (lp, seen) = rig(&env, &fake, cfg(Policy::full()));
    let mut rx = lp.subscribe();
    let run = lp.run_turn(env.chat, "log in to acme").unwrap();
    let Event::ElicitationRequested(card) = until(&mut rx, requested).await else {
        unreachable!()
    };
    assert_eq!(card.server, "acme");
    assert_eq!(card.message, "Paste your Acme token");
    assert_eq!(card.fields[0].name, "token");
    assert_eq!(lp.runtime().state(run).unwrap(), RunState::WaitingApproval);

    // An answer that does not fit keeps the form waiting.
    let too_long = "x".repeat(65);
    assert!(matches!(
        lp.finish_elicitation(card.call_id, ElicitationAnswer::Accept(content(&too_long))),
        Err(ElicitationRefusal::Invalid(_))
    ));
    lp.finish_elicitation(card.call_id, ElicitationAnswer::Accept(content(CANARY)))
        .unwrap();
    assert_eq!(
        lp.finish_elicitation(card.call_id, ElicitationAnswer::Decline),
        Err(ElicitationRefusal::Gone),
        "single use"
    );
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);

    assert_eq!(
        *seen.lock(),
        [ElicitationAnswer::Accept(content(CANARY))],
        "the server got the value"
    );
    let msgs = tool_messages(&fake.request(1));
    assert_eq!(msgs, ["accepted"]);

    // No sink the run wrote to contains the value.
    let mut all = serde_json::to_string(&msgs).unwrap();
    while let Ok(i) = rx.try_recv() {
        all.push_str(&serde_json::to_string(&i.envelope.event).unwrap());
    }
    all.push_str(&serde_json::to_string(&read_event_log(env.dir.path(), run).unwrap()).unwrap());
    let _ = write!(all, "{:?}", env.storage.list_audit(env.user).unwrap());
    let _ = write!(all, "{:?}", env.storage.list_steps(run).unwrap());
    let _ = write!(all, "{:?}", seen.lock());
    all.push_str(&log.lock());
    assert!(!all.contains(CANARY), "the typed value leaked");
    assert!(all.contains("elicitation.requested") && all.contains("elicitation.finished"));
}

#[tokio::test]
async fn decline_and_cancel_reach_the_server_as_such() {
    for answer in [ElicitationAnswer::Decline, ElicitationAnswer::Cancel] {
        let env = setup();
        let fake = script();
        let (lp, seen) = rig(&env, &fake, cfg(Policy::full()));
        let mut rx = lp.subscribe();
        let run = lp.run_turn(env.chat, "go").unwrap();
        let Event::ElicitationRequested(card) = until(&mut rx, requested).await else {
            unreachable!()
        };
        lp.finish_elicitation(card.call_id, answer.clone()).unwrap();
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
        assert_eq!(*seen.lock(), [answer]);
        assert!(lp.elicitation(card.call_id).is_none());
    }
}

#[tokio::test]
async fn a_form_nobody_answers_expires_and_the_server_gets_cancel() {
    let env = setup();
    let fake = script();
    let mut config = cfg(Policy::full());
    config.approval_timeout = Duration::from_millis(50);
    let (lp, seen) = rig(&env, &fake, config);
    let mut rx = lp.subscribe();
    let run = lp.run_turn(env.chat, "go").unwrap();
    let Event::ElicitationRequested(card) = until(&mut rx, requested).await else {
        unreachable!()
    };
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(*seen.lock(), [ElicitationAnswer::Cancel]);
    assert_eq!(
        lp.finish_elicitation(card.call_id, ElicitationAnswer::Accept(content(CANARY))),
        Err(ElicitationRefusal::Gone)
    );
}

#[tokio::test]
async fn unattended_runs_decline_at_once_and_show_no_form() {
    for initiator in [InitiatorKind::Schedule, InitiatorKind::Channel] {
        let env = setup();
        let fake = script();
        let (lp, seen) = rig(&env, &fake, cfg(Policy::full()));
        let mut rx = lp.subscribe();
        let run = lp.run_turn_as(env.chat, "go", initiator).unwrap();
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
        assert_eq!(*seen.lock(), [ElicitationAnswer::Decline], "{initiator:?}");
        while let Ok(i) = rx.try_recv() {
            assert!(!requested(&i.envelope.event), "a form was shown");
        }
    }
}

#[tokio::test]
async fn a_tool_call_without_a_live_run_is_declined() {
    let env = setup();
    let fake = script();
    let (lp, _) = rig(&env, &fake, cfg(Policy::full()));
    let req = ElicitationRequest {
        server: "acme".into(),
        message: "x".into(),
        fields: Vec::new(),
    };
    let elicitor = lp.elicitor();
    assert_eq!(
        elicitor.elicit("not-a-run", req.clone()).await,
        ElicitationAnswer::Decline
    );
    assert_eq!(
        elicitor
            .elicit(&protocol::RunId::new().to_string(), req)
            .await,
        ElicitationAnswer::Decline
    );
}
