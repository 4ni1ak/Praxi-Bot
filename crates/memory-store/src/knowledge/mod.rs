//! Per-bot knowledge: bounded document indexing and offline lexical retrieval.
//!
//! This index is separate from learned conversational `MemoryStore` records.
//! No embedding model, download, network call, or native inference runtime is
//! needed to open it, index text, or search. Source adapters decide what may be
//! read; all text returned here is data, never trusted instructions.

mod chunk;
mod schema;
mod search;
mod store;
mod types;

pub use chunk::chunk_text;
pub use store::KnowledgeStore;
pub use types::*;
