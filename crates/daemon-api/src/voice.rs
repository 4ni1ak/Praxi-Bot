//! Speech-to-text for dictation (`/v1/voice*`, ADR 0021): DTOs and the [`VoiceControl`] trait the
//! embedder implements (`apps/cli/src/voice_host.rs`).
//!
//! Recorded audio is sent, on the owner's explicit action, to an OpenAI-compatible
//! `/audio/transcriptions` endpoint the owner configured (a local server or a cloud provider).
//! The API key is **write-only**: it goes into the secret store and no response carries it.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Dictation configuration (`GET /v1/voice`): non-secret fields only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct VoiceConfigDto {
    /// Base URL of the OpenAI-compatible transcription service (without `/audio/transcriptions`).
    pub stt_base_url: String,
    /// Model name sent with each request.
    pub stt_model: String,
    /// Whether an API key is stored (never returned).
    pub stt_key_set: bool,
}

/// Body of `PUT /v1/voice`; omitted fields keep their value.
#[derive(Clone, Default, Deserialize, ToSchema)]
pub struct VoiceConfigInput {
    /// `https://...`, or `http://` for a loopback server only.
    #[serde(default)]
    pub stt_base_url: Option<String>,
    /// Model name.
    #[serde(default)]
    pub stt_model: Option<String>,
    /// API key (**write-only**: stored in the secret store, never returned or logged).
    #[serde(default)]
    pub stt_key: Option<String>,
    /// Remove the stored key.
    #[serde(default)]
    pub clear_stt_key: Option<bool>,
}

impl std::fmt::Debug for VoiceConfigInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceConfigInput")
            .field("stt_base_url", &self.stt_base_url)
            .field("stt_model", &self.stt_model)
            .field("stt_key", &self.stt_key.as_ref().map(|_| "<redacted>"))
            .field("clear_stt_key", &self.clear_stt_key)
            .finish()
    }
}

/// Body of `POST /v1/voice/transcribe`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct TranscribeInput {
    /// The recording, base64 (standard alphabet).
    pub audio_base64: String,
    /// MIME type of the recording, e.g. `audio/webm`.
    pub mime: String,
    /// Optional ISO-639-1 language hint (`en`, `tr`, `de`).
    #[serde(default)]
    pub language: Option<String>,
}

/// Result of `POST /v1/voice/transcribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TranscribeOutput {
    /// The recognised text.
    pub text: String,
}

/// Why a [`VoiceControl`] operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceControlError {
    /// Bad configuration or input (`422`).
    Invalid(String),
    /// Dictation is not configured yet (`409`).
    NotConfigured,
    /// The transcription service failed (`502`).
    Upstream(String),
    /// Persistence / secret-store failure (`500`).
    Internal(String),
}

/// What `/v1/voice*` needs from the embedder.
#[async_trait]
pub trait VoiceControl: Send + Sync {
    /// Current configuration (non-secret fields only).
    fn get(&self) -> VoiceConfigDto;

    /// Validate and persist the configuration (key to the secret store).
    ///
    /// # Errors
    /// [`VoiceControlError`].
    async fn update(&self, input: VoiceConfigInput) -> Result<VoiceConfigDto, VoiceControlError>;

    /// Transcribe one recording.
    ///
    /// # Errors
    /// [`VoiceControlError`].
    async fn transcribe(
        &self,
        audio: Vec<u8>,
        mime: String,
        language: Option<String>,
    ) -> Result<TranscribeOutput, VoiceControlError>;
}
