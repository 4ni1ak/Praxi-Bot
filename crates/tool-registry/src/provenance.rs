//! Trusted execution-side provenance, independent of model-visible tool bytes.

use serde::{Deserialize, Serialize};

/// What an executed tool can expose. This is supplied by daemon tool code;
/// never deserialize it from a document, model argument, or tool-result text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputProvenance {
    /// Output may contain instructions planted in an external source.
    pub ingests_untrusted: bool,
    /// Output may contain data belonging to the user.
    pub reads_private: bool,
    /// Short daemon-owned source label, without document text or paths.
    pub label: String,
}

impl OutputProvenance {
    /// Conservative classification for knowledge and delegated content.
    #[must_use]
    pub fn untrusted_private(label: impl Into<String>) -> Self {
        Self {
            ingests_untrusted: true,
            reads_private: true,
            label: label.into(),
        }
    }

    /// The persisted execution-fact wire format used by the egress gate.
    /// A fact is emitted only after execution starts; a refused call has none.
    #[must_use]
    pub fn executed(&self) -> serde_json::Value {
        serde_json::json!({
            "ran": true,
            "ingests_untrusted": self.ingests_untrusted,
            "reads_private": self.reads_private,
            "label": self.label,
        })
    }
}
