//! Exercise the shipped host's actual local backend, scope and live visibility.

use std::sync::Arc;

use memory_store::knowledge::{
    DocumentFormat, DocumentInput, KnowledgeStore, NewSource, Scope, SourceKind, SourceUpdate,
};
use protocol::{BotId, UserId};
use serde_json::json;
use tool_registry::{ContentPart, ToolContext};

#[tokio::test]
async fn shipped_agent_searches_only_the_enabled_scoped_index_without_an_embedder() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(storage::Storage::open(dir.path()).unwrap());
    let user = UserId::new();
    let bot = BotId::new();
    let other = BotId::new();
    storage
        .insert_user(&storage::User {
            id: user,
            name: "fixture owner".into(),
            created_at_ms: 1,
        })
        .unwrap();
    for id in [bot, other] {
        storage
            .insert_bot(&storage::Bot {
                id,
                user_id: user,
                name: "fixture bot".into(),
                config: json!({}),
                created_at_ms: 1,
            })
            .unwrap();
    }
    let agent = crate::serve::build_agent(
        &storage,
        agent_core::Providers::unconfigured(),
        dir.path(),
        None,
    );
    let tool = agent
        .tool_registry()
        .get("knowledge_search")
        .expect("shipped backend registration");
    assert!(!tool.visible_to(Some(&bot.to_string())));
    assert!(
        !dir.path()
            .join("bots")
            .join(bot.to_string())
            .join("knowledge.db")
            .exists()
    );
    let store = KnowledgeStore::open(
        dir.path(),
        Scope {
            user_id: user,
            bot_id: bot,
        },
    )
    .unwrap();
    let source = store
        .add_source(NewSource::new(SourceKind::Note, "Runbook", "note:runbook"))
        .unwrap();
    let outcome = store
        .index_document(
            source.id,
            source.revision,
            &DocumentInput {
                locator: "runbook.md".into(),
                title: "Saffron restore".into(),
                format: DocumentFormat::Markdown,
                content: "The saffron snapshot is on the backup volume.".into(),
                mtime_ms: None,
            },
        )
        .unwrap();
    assert!(tool.visible_to(Some(&bot.to_string())));
    let context = |id: BotId| ToolContext {
        bot_id: Some(id.to_string()),
        ..ToolContext::default()
    };
    let found = tool
        .call(&context(bot), json!({"query":"saffron snapshot"}))
        .await
        .unwrap();
    assert!(
        found.content.iter().any(
            |part| matches!(part, ContentPart::Text { text } if text.contains("backup volume"))
        )
    );
    assert!(!tool.visible_to(Some(&other.to_string())));
    let foreign = tool
        .call(&context(other), json!({"query":"saffron snapshot"}))
        .await
        .unwrap();
    assert!(
        !foreign.content.iter().any(
            |part| matches!(part, ContentPart::Text { text } if text.contains("backup volume"))
        )
    );
    assert!(
        !dir.path()
            .join("bots")
            .join(other.to_string())
            .join("knowledge.db")
            .exists()
    );
    store
        .update_source(
            source.id,
            outcome.source_revision,
            SourceUpdate {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!tool.visible_to(Some(&bot.to_string())));
}
