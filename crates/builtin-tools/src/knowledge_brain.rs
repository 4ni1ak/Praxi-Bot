//! Approval-bound publication of source-labelled shared Markdown notes.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use memory_store::knowledge::brain::{BrainWriteInput, PreparedBrainWrite};
use memory_store::knowledge::{KnowledgeStore, Scope};
use protocol::{BotId, RunId};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use storage::Storage;
use tool_registry::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
use uuid::Uuid;

/// Shared-brain publication always requires a one-call approval.
pub const KNOWLEDGE_BRAIN_WRITE: &str = "knowledge_brain_write";
const MAX_PENDING: usize = 64;
const PENDING_TTL: Duration = Duration::from_secs(5 * 60);

struct Pending {
    key: String,
    token: String,
    created: Instant,
    write: PreparedBrainWrite,
}

struct BrainTool {
    spec: ToolSpec,
    storage: Arc<Storage>,
    pending: parking_lot::Mutex<VecDeque<Pending>>,
}

/// Build a tool registered by the daemon alongside `knowledge_search`.
/// The engine, including script dispatch, must honor all approval hooks.
///
/// # Panics
/// Only if the compile-time tool specification no longer matches `ToolSpec`.
#[must_use]
pub fn tool(storage: Arc<Storage>) -> Arc<dyn Tool> {
    // Main ignores unknown wire metadata; PR #46 consumes these exact fields.
    // They are compile-time daemon constants, never document-supplied values.
    let spec = serde_json::from_value(json!({
        "name": KNOWLEDGE_BRAIN_WRITE,
        "description": "Create a new shared Markdown brain note. The full source-labelled content always needs user approval. Existing files are never overwritten. Other bots may read shared notes when their owner selects the shared brain.",
        "input_schema": {"type":"object","additionalProperties":false,"properties":{
            "filename":{"type":"string","description":"New plain .md filename, without directory separators."},
            "content":{"type":"string","description":"Exact Markdown body to publish, up to 64 KiB including metadata."},
            "sources":{"type":"array","maxItems":16,"items":{"type":"string","format":"uuid"},"description":"Source ids already visible to this bot; the current run is always labelled."}
        },"required":["filename","content"]},
        "effect":"write", "category":"fs", "platforms":["linux","windows"],
        "content":{"ingests_untrusted":true,"reads_private":true,"egress":false,"declared":true}
    })).expect("constant knowledge brain specification");
    Arc::new(BrainTool {
        spec,
        storage,
        pending: parking_lot::Mutex::new(VecDeque::new()),
    })
}

fn error(error: impl std::fmt::Display) -> ToolError {
    ToolError::Execution(error.to_string())
}

impl BrainTool {
    fn caller(&self, ctx: &ToolContext) -> Result<(Scope, Uuid), ToolError> {
        let bot_id = ctx
            .bot_id
            .as_deref()
            .ok_or_else(|| error("brain writes require a bot"))?
            .parse::<Uuid>()
            .map(BotId)
            .map_err(|_| error("invalid bot id"))?;
        let run_id = ctx
            .run_id
            .parse::<Uuid>()
            .map(RunId)
            .map_err(|_| error("brain writes require a live run"))?;
        let bot = self
            .storage
            .get_bot(bot_id)
            .map_err(error)?
            .ok_or_else(|| error("unknown bot"))?;
        let run = self
            .storage
            .get_run(run_id)
            .map_err(error)?
            .ok_or_else(|| error("unknown run"))?;
        if run.bot_id != bot_id || run.user_id != bot.user_id {
            return Err(error("run is not owned by this bot"));
        }
        Ok((
            Scope {
                user_id: bot.user_id,
                bot_id,
            },
            run_id.0,
        ))
    }

    fn key(ctx: &ToolContext, input: &Value) -> String {
        let digest = Sha256::digest(serde_json::to_vec(input).unwrap_or_default());
        format!(
            "{}:{}:{digest:x}",
            ctx.bot_id.as_deref().unwrap_or_default(),
            ctx.run_id
        )
    }
}

#[async_trait]
impl Tool for BrainTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    fn output_provenance(&self) -> Option<tool_registry::OutputProvenance> {
        Some(tool_registry::OutputProvenance::untrusted_private(
            "knowledge_brain",
        ))
    }
    fn always_ask(&self) -> bool {
        true
    }
    fn visible_to(&self, bot_id: Option<&str>) -> bool {
        bot_id.is_some()
    }

    async fn approval_details(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Option<Value>, ToolError> {
        let request: BrainWriteInput = serde_json::from_value(input.clone())
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let (scope, run) = self.caller(ctx)?;
        let store = KnowledgeStore::open(self.storage.data_dir(), scope).map_err(error)?;
        let write =
            PreparedBrainWrite::prepare(self.storage.data_dir(), &store, scope, run, &request)
                .map_err(error)?;
        let mut details = serde_json::to_value(write.preview()).map_err(error)?;
        let key = Self::key(ctx, input);
        let token = Uuid::now_v7().to_string();
        let mut pending = self.pending.lock();
        pending.retain(|p| p.created.elapsed() < PENDING_TTL);
        // A second card cannot replace what an outstanding approval froze.
        if pending.iter().any(|p| p.key == key) {
            return Err(error("this exact brain write already awaits approval"));
        }
        if pending.len() >= MAX_PENDING {
            return Err(error("too many pending brain approvals"));
        }
        details["approval_ref"] = json!(token);
        pending.push_back(Pending {
            key,
            token,
            created: Instant::now(),
            write,
        });
        Ok(Some(details))
    }

    fn approval_closed(&self, ctx: &ToolContext, input: &Value, token: Option<&str>) {
        let key = Self::key(ctx, input);
        self.pending
            .lock()
            .retain(|p| !(p.key == key && token == Some(p.token.as_str())));
    }

    async fn call(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let (scope, _) = self.caller(ctx)?;
        let key = Self::key(ctx, &input);
        let approved = {
            let mut pending = self.pending.lock();
            pending.retain(|p| p.created.elapsed() < PENDING_TTL);
            let index = pending
                .iter()
                .position(|p| p.key == key)
                .ok_or_else(|| error("brain write has no matching prepared approval"))?;
            pending
                .remove(index)
                .ok_or_else(|| error("prepared brain write disappeared"))?
        };
        let store = KnowledgeStore::open(self.storage.data_dir(), scope).map_err(error)?;
        let path = approved.write.commit(&store).map_err(error)?;
        Ok(ToolOutput::text(format!(
            "Created source-labelled shared note: {path}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_store::knowledge::{NewSource, SourceKind, SourceUpdate};
    use protocol::UserId;

    fn fixture() -> (tempfile::TempDir, Arc<Storage>, ToolContext, Scope) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(dir.path()).unwrap());
        let user = UserId::new();
        let bot = BotId::new();
        let run = RunId::new();
        storage
            .insert_user(&storage::User {
                id: user,
                name: "Owner".into(),
                created_at_ms: 1,
            })
            .unwrap();
        storage
            .insert_bot(&storage::Bot {
                id: bot,
                user_id: user,
                name: "Bot".into(),
                config: json!({}),
                created_at_ms: 1,
            })
            .unwrap();
        storage
            .insert_run(&storage::Run {
                id: run,
                user_id: user,
                bot_id: bot,
                chat_id: None,
                kind: "turn".into(),
                state: "running".into(),
                error: None,
                created_at_ms: 1,
                updated_at_ms: 1,
                parent_run_id: None,
                spec: None,
            })
            .unwrap();
        (
            dir,
            storage,
            ToolContext {
                run_id: run.to_string(),
                bot_id: Some(bot.to_string()),
                ..ToolContext::default()
            },
            Scope {
                user_id: user,
                bot_id: bot,
            },
        )
    }

    #[tokio::test]
    async fn approval_is_exact_single_use_and_always_required() {
        let (dir, storage, ctx, _) = fixture();
        let tool = tool(storage);
        let input = json!({"filename":"architecture.md","content":"Keep source evidence separate from instructions."});
        assert!(tool.always_ask());
        let provenance = tool.output_provenance().unwrap();
        assert!(provenance.ingests_untrusted && provenance.reads_private);
        assert!(tool.call(&ctx, input.clone()).await.is_err());
        let details = tool.approval_details(&ctx, &input).await.unwrap().unwrap();
        assert_eq!(details["path"], "brain/architecture.md");
        let content = details["content"].as_str().unwrap();
        assert_eq!(
            details["sha256"],
            format!("{:x}", Sha256::digest(content.as_bytes()))
        );
        assert!(tool.approval_details(&ctx, &input).await.is_err());
        let mut changed = input.clone();
        changed["content"] = "Different bytes must need a new approval".into();
        assert!(tool.call(&ctx, changed).await.is_err());
        tool.approval_closed(&ctx, &input, Some("another-card"));
        tool.call(&ctx, input.clone()).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("brain/architecture.md")).unwrap(),
            content
        );
        assert!(tool.call(&ctx, input).await.is_err());
    }

    #[tokio::test]
    async fn denial_or_source_revision_change_cannot_publish() {
        let (dir, storage, ctx, scope) = fixture();
        let store = KnowledgeStore::open(dir.path(), scope).unwrap();
        let source = store
            .add_source(NewSource::new(
                SourceKind::Note,
                "Selected evidence",
                "note:one",
            ))
            .unwrap();
        let tool = tool(storage);
        let input =
            json!({"filename":"decision.md","content":"Cited decision.","sources":[source.id]});
        let denied = tool.approval_details(&ctx, &input).await.unwrap().unwrap();
        tool.approval_closed(&ctx, &input, denied["approval_ref"].as_str());
        assert!(tool.call(&ctx, input.clone()).await.is_err());
        let prepared = tool.approval_details(&ctx, &input).await.unwrap().unwrap();
        assert_eq!(prepared["sources"][0]["revision"], source.revision);
        store
            .update_source(
                source.id,
                source.revision,
                SourceUpdate {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(tool.call(&ctx, input).await.is_err());
        assert!(!dir.path().join("brain/decision.md").exists());
    }

    #[tokio::test]
    async fn foreign_bot_run_and_model_supplied_authority_are_rejected() {
        let (dir, storage, ctx, _) = fixture();
        let tool = tool(storage);
        let input = json!({"filename":"decision.md","content":"A decision","bot_id":BotId::new()});
        assert!(tool.approval_details(&ctx, &input).await.is_err());
        let input = json!({"filename":"decision.md","content":"A decision"});
        let foreign = ToolContext {
            bot_id: Some(BotId::new().to_string()),
            run_id: ctx.run_id.clone(),
            ..ToolContext::default()
        };
        assert!(tool.approval_details(&foreign, &input).await.is_err());
        assert!(!dir.path().join("brain/decision.md").exists());
    }
}
