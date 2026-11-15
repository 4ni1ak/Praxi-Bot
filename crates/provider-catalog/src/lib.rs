//! LLM provider plugin registry: trait definitions and built-in provider adapters.
//!
//! Abstracts over Anthropic, `OpenRouter`, `DeepSeek`, Ollama, and any
//! OpenAI-compatible endpoint through a uniform streaming provider trait.

pub mod anthropic;
pub mod catalog;
pub mod config;
pub mod embedding;
pub mod entry;
pub mod error;
pub mod factory;
pub mod gemini;
mod limits;
pub mod model_metadata;
pub mod openai_compat;
pub mod provider;
pub mod selector;
mod util;

pub use anthropic::AnthropicProvider;
pub use catalog::Catalog;
pub use config::{
    ConfigError, ConnectionReport, ProviderConfig, ProviderSettings, api_key_secret_name,
    config_path, test_connection,
};
pub use embedding::{
    EmbeddingInput, EmbeddingProvider, EmbeddingRequest, EmbeddingResponse, EmbeddingTask,
    EmbeddingTaskFormat,
};
pub use entry::{
    Auth, CapabilityOverrides, CompatFlags, ModelCapabilities, ModelInfo, ProviderEntry,
    ProviderKind,
};
pub use error::ProviderError;
pub use factory::{build_chat_provider, build_embedding_provider};
pub use gemini::GeminiProvider;
pub use model_metadata::{ModelKind, ModelMetadata, model_metadata};
pub use openai_compat::{OpenAiCompatConfig, OpenAiCompatProvider};
pub use provider::{
    ChatChunk, ChatProvider, ChatRequest, FinishReason, Message, MessageRole, ModelRef, ToolCall,
    ToolChoice, Usage,
};
pub use selector::ModelSelector;
