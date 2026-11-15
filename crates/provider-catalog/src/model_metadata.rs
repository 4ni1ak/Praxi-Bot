//! Picker metadata, deliberately separate from execution capabilities and prices.
//!
//! The curated table only annotates ids supplied by the live listing. It never
//! installs models, changes a bot's configured model, or becomes an allowlist.

use std::{collections::BTreeSet, sync::OnceLock};

use serde::{Deserialize, Serialize};

/// Primary use of a model. Reasoning and vision-capable text models are chat
/// candidates; image/audio/video *generation* models are not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    Chat,
    Reasoning,
    VisionCapable,
    Embedding,
    Image,
    Audio,
    Video,
    Other,
}

impl ModelKind {
    #[must_use]
    pub const fn is_chat(self) -> bool {
        matches!(self, Self::Chat | Self::Reasoning | Self::VisionCapable)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Reasoning => "reasoning",
            Self::VisionCapable => "vision_capable",
            Self::Embedding => "embedding",
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Other => "other",
        }
    }
}

/// Secret-free presentation metadata. The id remains the exact wire id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelMetadata {
    pub id: String,
    pub display_name: String,
    pub kind: ModelKind,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub recommended_rank: Option<u8>,
    #[serde(default)]
    pub deprecated: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderMetadata {
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    sources: Vec<String>,
    models: Vec<ModelMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CuratedModels {
    verified_at: String,
    providers: Vec<ProviderMetadata>,
}

impl CuratedModels {
    fn validate(&self) -> Result<(), &'static str> {
        if self.verified_at.is_empty() {
            return Err("verification date is required");
        }
        let mut providers = BTreeSet::new();
        for provider in &self.providers {
            for id in std::iter::once(&provider.id).chain(&provider.aliases) {
                if !safe_model_id(id) || !providers.insert(id) {
                    return Err("invalid or duplicate provider id");
                }
            }
            if provider.sources.is_empty()
                || provider.sources.iter().any(|s| !s.starts_with("https://"))
            {
                return Err("official source URLs are required");
            }
            let mut ids = BTreeSet::new();
            let mut ranks = BTreeSet::new();
            for model in &provider.models {
                if !safe_model_id(&model.id) || !ids.insert(&model.id) {
                    return Err("invalid or duplicate model id");
                }
                if model.display_name.trim().is_empty()
                    || model.display_name.chars().any(char::is_control)
                {
                    return Err("display name is required");
                }
                if model.recommended {
                    if model.kind != ModelKind::Chat || model.deprecated {
                        return Err("only current chat models can be recommended");
                    }
                    let Some(rank @ 1..=4) = model.recommended_rank else {
                        return Err("recommendation rank must be 1..4");
                    };
                    if !ranks.insert(rank) || model.id.ends_with(":free") {
                        return Err("duplicate rank or unstable free recommendation");
                    }
                } else if model.recommended_rank.is_some() {
                    return Err("only recommendations carry a rank");
                }
                if model.note.as_deref() == Some("free") {
                    return Err("free notes must originate from the live list");
                }
            }
        }
        Ok(())
    }

    fn describe(&self, provider: &str, id: &str) -> ModelMetadata {
        self.providers
            .iter()
            .find(|p| p.id == provider || p.aliases.iter().any(|a| a == provider))
            .and_then(|p| p.models.iter().find(|m| m.id == id))
            .cloned()
            .unwrap_or_else(|| inferred_metadata(id))
    }
}

fn embedded() -> &'static CuratedModels {
    static CURATED: OnceLock<CuratedModels> = OnceLock::new();
    CURATED.get_or_init(|| {
        let table: CuratedModels = serde_json::from_str(include_str!("model_metadata.json"))
            .expect("checked-in model metadata must parse");
        table
            .validate()
            .expect("checked-in model metadata must validate");
        table
    })
}

/// Annotate *only* live ids, deduplicate, filter and order recommendations
/// before other models. This is a presentation helper, never an execution gate.
/// `chat_only = false` also includes deprecated and non-chat models.
#[must_use]
pub fn model_metadata(provider: &str, ids: &[String], chat_only: bool) -> Vec<ModelMetadata> {
    let mut seen = BTreeSet::new();
    let mut models: Vec<_> = ids
        .iter()
        .filter(|id| safe_model_id(id) && seen.insert(id.as_str()))
        .map(|id| {
            let mut model = embedded().describe(provider, id);
            if provider == "openrouter" && id.ends_with(":free") {
                // This id came from the provider's current list, not the table.
                model.note = Some("free".into());
            }
            model
        })
        .filter(|m| !chat_only || (m.kind.is_chat() && !m.deprecated))
        .collect();
    models.sort_by(|a, b| {
        a.recommended_rank
            .unwrap_or(u8::MAX)
            .cmp(&b.recommended_rank.unwrap_or(u8::MAX))
            .then_with(|| a.display_name.cmp(&b.display_name))
            .then_with(|| a.id.cmp(&b.id))
    });
    models
}

fn safe_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id.bytes().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'/' | b':' | b'@' | b'+')
        })
}

fn inferred_metadata(id: &str) -> ModelMetadata {
    let lower = id.to_ascii_lowercase();
    ModelMetadata {
        id: id.into(),
        display_name: humanize(id),
        kind: infer_kind(&lower),
        recommended: false,
        recommended_rank: None,
        deprecated: false,
        note: lower.contains("preview").then(|| "preview".into()),
    }
}

fn infer_kind(id: &str) -> ModelKind {
    let groups: &[(ModelKind, &[&str])] = &[
        (
            ModelKind::Embedding,
            &["embed", "bge-", "e5-", "nomic-", "snowflake-arctic-embed"],
        ),
        (
            ModelKind::Video,
            &["video", "veo", "sora", "seedance", "gemini-omni"],
        ),
        (
            ModelKind::Audio,
            &[
                "tts",
                "audio",
                "music",
                "realtime",
                "real-time",
                "whisper",
                "transcri",
                "speech",
                "lyria",
                "-live",
            ],
        ),
        (
            ModelKind::Image,
            &[
                "image",
                "imagen",
                "dall-e",
                "dalle",
                "flux",
                "stable-diffusion",
                "nano-banana",
            ],
        ),
        (
            ModelKind::Other,
            &[
                "deep-research",
                "deep_research",
                "moderation",
                "rerank",
                "computer-use",
                "computer_use",
                "robotics",
                "aqa",
            ],
        ),
        (ModelKind::Reasoning, &["reasoning", "reasoner"]),
        (ModelKind::VisionCapable, &["vision", "-vl"]),
    ];
    groups
        .iter()
        .find(|(_, patterns)| patterns.iter().any(|p| id.contains(p)))
        .map_or(ModelKind::Chat, |(kind, _)| *kind)
}

fn humanize(id: &str) -> String {
    id.split(['-', '_', '/', ':'])
        .filter(|s| !s.is_empty())
        .map(|word| {
            if matches!(word, "gpt" | "tts" | "api" | "vl") {
                word.to_ascii_uppercase()
            } else if word == "openai" {
                "OpenAI".into()
            } else {
                let mut chars = word.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_uppercase().chain(chars).collect()
                })
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests;
