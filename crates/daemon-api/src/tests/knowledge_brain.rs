//! The installed daemon tool must use the real, once-only approval flow.

use super::*;
use memory_store::knowledge::{KnowledgeStore, NewSource, Scope, SourceKind};
use protocol::event::ApprovalNeeded;
use protocol::{BotId, Event};
use sha2::{Digest, Sha256};

async fn propose_note(t: &T, chat: &str, source: uuid::Uuid, filename: &str) -> ApprovalNeeded {
    let mut stream = t.agent.subscribe();
    let input = json!({
        "filename": filename,
        "content": "Keep the selected source with this architectural decision.",
        "sources": [source],
    });
    let run = send_msg(
        t,
        chat,
        &format!("call:knowledge_brain_write {input}"),
        "queue",
    )
    .await["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.recv().await.unwrap().envelope.event {
                Event::ApprovalNeeded(approval) if approval.run_id.to_string() == run => {
                    return approval;
                }
                Event::RunFinished(finished) if finished.run_id.to_string() == run => {
                    panic!("brain publication completed without an approval");
                }
                _ => {}
            }
        }
    })
    .await
    .expect("installed brain tool must ask for exact-call approval")
}

#[tokio::test]
async fn installed_brain_tool_requires_exact_once_only_approval_even_under_full_policy() {
    let t = setup();
    t.agent.set_global_policy(permission_engine::Policy::full());
    let (bot, chat) = make_chat(&t).await;
    let tool = t
        .agent
        .tool_registry()
        .get("knowledge_brain_write")
        .expect("AppState must install the shared-brain tool");
    assert!(tool.always_ask());
    let bot_id = BotId(bot.parse().unwrap());
    let store = KnowledgeStore::open(
        t.state.data_dir(),
        Scope {
            user_id: t.state.user_id(),
            bot_id,
        },
    )
    .unwrap();
    let source = store
        .add_source(NewSource::new(
            SourceKind::Note,
            "Reference decision",
            "note:reference",
        ))
        .unwrap();
    let approval = propose_note(&t, &chat, source.id, "approved.md").await;
    let card = approval.details.as_ref().unwrap();
    let content = card["content"].as_str().unwrap().to_owned();
    assert_eq!(approval.tool_name, "knowledge_brain_write");
    assert_eq!(card["path"], "brain/approved.md");
    assert_eq!(card["bot_id"], bot);
    assert_eq!(card["run_id"], approval.run_id.to_string());
    assert_eq!(card["sources"][0]["id"], source.id.to_string());
    assert_eq!(card["sources"][0]["revision"], source.revision);
    assert_eq!(card["sources"][0]["label"], source.label);
    assert_eq!(
        card["sha256"],
        format!("{:x}", Sha256::digest(content.as_bytes()))
    );
    assert!(card.get("approval_ref").is_none());
    let target = t.state.data_dir().join("brain/approved.md");
    assert!(!target.exists());
    let path = format!("/v1/approvals/{}", approval.call_id);
    let permanent = call(
        &t,
        Method::POST,
        &path,
        Some(json!({"decision":"approve", "scope":"always"})),
    )
    .await;
    assert!(permanent.status.is_client_error());
    assert!(!target.exists());
    let once = call(
        &t,
        Method::POST,
        &path,
        Some(json!({"decision":"approve", "scope":"once"})),
    )
    .await;
    assert_eq!(once.status, StatusCode::OK, "{}", once.json());
    wait_state(&t, &approval.run_id.to_string(), "succeeded").await;
    assert_eq!(std::fs::read_to_string(target).unwrap(), content);
    assert_eq!(t.agent.list_grants(bot_id), [] as [agent_core::Grant; 0]);

    let denied = propose_note(&t, &chat, source.id, "denied.md").await;
    let response = call(
        &t,
        Method::POST,
        &format!("/v1/approvals/{}", denied.call_id),
        Some(json!({"decision":"deny"})),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    let frames = events(&t, &format!("?run_id={}", denied.run_id), None).await;
    assert!(frames.iter().any(|frame| frame.1 == "tool_call_denied"));
    assert!(!t.state.data_dir().join("brain/denied.md").exists());
}
