//! Optional, cheap pre-retrieval through the normal tool execution path.

use std::sync::Arc;

use builtin_tools::knowledge::KNOWLEDGE_SEARCH;
use protocol::BotId;
use serde_json::json;
use storage::{RunStep, Storage};
use task_runtime::StepSpec;

use crate::exec::TurnExecutor;
use crate::shared::{RunCtx, STEP_TOOLS, STEP_USER};

/// Own-side hook for a routing layer's already-computed retrieval decision.
/// Implementations must be cheap, bounded, and perform no model/network call.
/// An external decider may provide a cached decision through this interface.
/// Returning a query schedules `knowledge_search` with ordinary permissions.
pub trait KnowledgeDecider: Send + Sync + std::fmt::Debug {
    /// `None` adds no retrieval step and no retrieval text to the prompt.
    fn query(&self, bot_id: BotId, user_text: &str) -> Option<String>;
}

/// Opt-in heuristic: reads `bot.config.knowledge.auto_retrieve` (default false).
/// It adds no separate model call and ignores commands and short greetings.
pub struct HeuristicKnowledgeDecider {
    storage: Arc<Storage>,
}

impl std::fmt::Debug for HeuristicKnowledgeDecider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeuristicKnowledgeDecider")
            .finish_non_exhaustive()
    }
}

impl HeuristicKnowledgeDecider {
    /// Resolve the setting from the current stored bot, so changes take effect
    /// on the next turn without a process restart.
    #[must_use]
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }
}

impl KnowledgeDecider for HeuristicKnowledgeDecider {
    fn query(&self, bot_id: BotId, user_text: &str) -> Option<String> {
        let bot = self.storage.get_bot(bot_id).ok()??;
        if bot.config.pointer("/knowledge/auto_retrieve") != Some(&json!(true)) {
            return None;
        }
        let text = user_text.trim();
        if text.starts_with('/') || text.len() > 4096 || text.split_whitespace().count() < 2 {
            return None;
        }
        let lower = text.to_lowercase();
        let relevant = text.contains('?')
            || [
                "find ",
                "recall ",
                "document",
                "knowledge",
                "hatırla",
                "belge",
                "nerede",
                "neydi",
                "nasıl",
            ]
            .iter()
            .any(|word| lower.contains(word));
        relevant.then(|| {
            text.split_whitespace()
                .take(24)
                .collect::<Vec<_>>()
                .join(" ")
        })
    }
}

impl TurnExecutor {
    pub(crate) fn knowledge_step(&self, ctx: &RunCtx, steps: &[RunStep]) -> Option<StepSpec> {
        if ctx.parent_run_id.is_some() || steps.iter().any(|s| s.input["pre_retrieval"] == true) {
            return None;
        }
        let last = steps.last().filter(|s| s.name == STEP_USER)?;
        let text = last.input["text"].as_str()?;
        let query = self
            .sh
            .cfg
            .knowledge_decider
            .as_ref()?
            .query(ctx.bot_id, text)?;
        if query.trim().is_empty() || query.len() > 4096 {
            return None;
        }
        let tool = self.sh.tools.get(KNOWLEDGE_SEARCH)?;
        if !self
            .sh
            .tool_visible_to_run(&ctx.run_id.to_string(), tool.spec())
        {
            return None;
        }
        let input = json!({"query":query,"top_k":3,"budget_tokens":600,"max_bytes":8192});
        let hints = crate::router::route_hints(steps).tools;
        let (name, arguments) = if self
            .sh
            .resolve_call(&ctx.profile, ctx.bot_id, KNOWLEDGE_SEARCH, &input, &hints)
            .is_some()
        {
            (KNOWLEDGE_SEARCH, input)
        } else {
            let wrapper = json!({"name":KNOWLEDGE_SEARCH,"input":input});
            self.sh
                .resolve_call(&ctx.profile, ctx.bot_id, "use_tool", &wrapper, &hints)?;
            ("use_tool", wrapper)
        };
        Some(StepSpec {
            name: STEP_TOOLS.into(),
            input: json!({
                "pre_retrieval":true,
                "calls":[{"id":format!("knowledge-{}",uuid::Uuid::now_v7()),"name":name,"arguments":arguments.to_string(),"repeat":1}]
            }),
            idempotent: true,
            retry: None,
        })
    }
}

/// Apply output limits after masking or another trusted text transform.
/// If masking changed JSON keys/identifiers, discard that result instead of
/// reconstructing unverifiable citations or reintroducing masked strings.
pub(crate) fn rebudget_text(
    text: &str,
    request: &memory_store::knowledge::SearchRequest,
) -> String {
    serde_json::from_str::<memory_store::knowledge::SearchResponse>(text)
        .ok()
        .and_then(|response| response.rebudget(request).ok())
        .and_then(|response| serde_json::to_string(&response).ok())
        .unwrap_or_default()
}

pub(crate) fn request_budgets(
    request: &provider_catalog::ChatRequest,
) -> std::collections::HashMap<String, memory_store::knowledge::SearchRequest> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.tool_calls)
        .filter_map(|call| {
            let input: serde_json::Value = serde_json::from_str(&call.arguments).ok()?;
            let input = match call.name.as_str() {
                KNOWLEDGE_SEARCH => input,
                "use_tool" if input["name"] == KNOWLEDGE_SEARCH => input["input"].clone(),
                _ => return None,
            };
            serde_json::from_value(input)
                .ok()
                .map(|budget| (call.id.clone(), budget))
        })
        .collect()
}

pub(crate) fn rebudget_request(
    request: &mut provider_catalog::ChatRequest,
    budgets: &std::collections::HashMap<String, memory_store::knowledge::SearchRequest>,
) {
    for message in &mut request.messages {
        if message.role == provider_catalog::MessageRole::Tool
            && let Some(budget) = message.tool_call_id.as_ref().and_then(|id| budgets.get(id))
            && let Some(text) = message
                .content
                .as_str()
                .filter(|text| text.starts_with('{'))
        {
            message.content = json!(rebudget_text(text, budget));
        }
    }
}

#[cfg(test)]
mod tests;
