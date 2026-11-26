//! Explicit-artifact local E5 inference. No network/download API is called here.
//!
//! Each input is covered by at most eight non-overlapping tokenizer windows.
//! Every prefixed window is checked against the 512-token model context, and
//! its normalized embedding contributes to a token-weighted mean. The complete
//! input remains the cited chunk; over-cap input is rejected without truncation.
//! This recipe is part of the artifact profile, so older vectors cannot mix.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{EmbeddingProfile, EmbeddingTaskFormat, invalid, is_digest};
use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedEmbeddingArtifact {
    /// Absolute path selected by the operator, never a document-provided path.
    pub path: PathBuf,
    /// SHA-256 of the exact artifact bytes.
    pub sha256: String,
}

/// Self-contained multilingual E5 small ONNX + tokenizer assets, and a matching
/// ONNX Runtime shared library. The operator acquires and licenses these files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalEmbeddingArtifacts {
    pub runtime: PinnedEmbeddingArtifact,
    pub model: PinnedEmbeddingArtifact,
    pub tokenizer: PinnedEmbeddingArtifact,
    pub config: PinnedEmbeddingArtifact,
    pub special_tokens_map: PinnedEmbeddingArtifact,
    pub tokenizer_config: PinnedEmbeddingArtifact,
}

impl LocalEmbeddingArtifacts {
    /// Derive an immutable profile without loading native code or opening files.
    ///
    /// # Errors
    /// Relative paths, malformed hashes, or invalid model profile metadata.
    pub fn profile(&self) -> Result<EmbeddingProfile> {
        let artifacts = [
            &self.runtime,
            &self.model,
            &self.tokenizer,
            &self.config,
            &self.special_tokens_map,
            &self.tokenizer_config,
        ];
        for artifact in &artifacts {
            if !artifact.path.is_absolute() || !is_digest(&artifact.sha256) {
                return Err(invalid(
                    "local embedding artifacts require absolute paths and SHA-256 hashes",
                ));
            }
        }
        let hashes = artifacts
            .iter()
            .map(|artifact| artifact.sha256.as_str())
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&(
            "local-e5-small-mean384-window512x8-token-mean-cpu-v2",
            hashes,
        ))?;
        let digest = format!("{:x}", Sha256::digest(bytes));
        Ok(EmbeddingProfile {
            provider_id: "local-onnx".into(),
            model: "multilingual-e5-small".into(),
            model_revision: Some(digest.clone()),
            endpoint_identity: format!("local-onnx:{digest}"),
            dimensions: 384,
            request_dimensions: false,
            format: EmbeddingTaskFormat::E5,
        })
    }
}

#[cfg(any(test, feature = "local-embeddings"))]
fn read_artifact(artifact: &PinnedEmbeddingArtifact, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(&artifact.path)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > limit {
        return Err(invalid("local artifact is not a bounded regular file"));
    }
    let file = std::fs::File::open(&artifact.path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("local artifact changed while opening"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit || format!("{:x}", Sha256::digest(&bytes)) != artifact.sha256 {
        return Err(invalid("local artifact size or SHA-256 mismatch"));
    }
    Ok(bytes)
}

/// Own the verified bytes for the native library's entire process lifetime.
/// Linux uses a sealed memfd (no writable or replaceable filesystem name).
/// Windows uses a private directory and denies write/delete sharing while its
/// open handle remains alive. Other platforms fail closed.
#[cfg(any(test, feature = "local-embeddings"))]
struct VerifiedRuntime {
    path: PathBuf,
    #[allow(dead_code)] // held only to keep the mapping/lock alive
    file: Option<std::fs::File>,
    #[cfg(windows)]
    directory: Option<tempfile::TempDir>,
}

#[cfg(all(windows, any(test, feature = "local-embeddings")))]
impl Drop for VerifiedRuntime {
    fn drop(&mut self) {
        // Windows cannot delete a readonly or share-locked DLL snapshot.
        // Release the lock only after this runtime owner is being destroyed.
        drop(self.file.take());
        if let Ok(metadata) = std::fs::metadata(&self.path) {
            let mut permissions = metadata.permissions();
            // The lint concerns Unix world-writable bits; on Windows this only
            // clears the read-only attribute so the snapshot can be deleted.
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(&self.path, permissions);
        }
        if let Some(directory) = self.directory.take() {
            let _ = directory.close();
        }
    }
}

#[cfg(any(test, feature = "local-embeddings"))]
impl VerifiedRuntime {
    fn copy(artifact: &PinnedEmbeddingArtifact) -> Result<Self> {
        #[cfg(not(any(target_os = "linux", windows)))]
        return Err(invalid(
            "verified native loading is supported only on Linux and Windows",
        ));
        #[cfg(windows)]
        use std::io::Read as _;
        #[cfg(any(target_os = "linux", windows))]
        use std::io::Write;
        #[cfg(any(target_os = "linux", windows))]
        let bytes = read_artifact(artifact, 256 * 1024 * 1024)?;
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
            use std::os::fd::AsRawFd;
            let fd = memfd_create(
                "praxi-verified-onnx",
                MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
            )
            .map_err(std::io::Error::from)?;
            let mut file = std::fs::File::from(fd);
            file.write_all(&bytes)?;
            fcntl_add_seals(
                &file,
                SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
            )
            .map_err(std::io::Error::from)?;
            let path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
            Ok(Self {
                path,
                file: Some(file),
            })
        }
        #[cfg(windows)]
        {
            let directory = tempfile::Builder::new()
                .prefix("praxi-verified-onnx-")
                .tempdir()?;
            let path = directory.path().join(if cfg!(windows) {
                "onnxruntime.dll"
            } else {
                "libonnxruntime.dylib"
            });
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            output.write_all(&bytes)?;
            output.sync_all()?;
            let mut permissions = output.metadata()?.permissions();
            permissions.set_readonly(true);
            output.set_permissions(permissions)?;
            drop(output);
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(1); // FILE_SHARE_READ, no FILE_SHARE_WRITE/DELETE.
            }
            let mut file = options.open(&path)?;
            let mut frozen = Vec::new();
            file.read_to_end(&mut frozen)?;
            if frozen != bytes {
                return Err(invalid("verified runtime copy changed before locking"));
            }
            Ok(Self {
                path,
                file: Some(file),
                directory: Some(directory),
            })
        }
    }
}

#[cfg(test)]
mod verified_copy_tests {
    use super::*;

    #[test]
    #[cfg(any(target_os = "linux", windows))]
    fn swapping_the_original_cannot_change_the_native_load_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("library");
        let bytes = b"trusted runtime fixture";
        std::fs::write(&original, bytes).unwrap();
        let artifact = PinnedEmbeddingArtifact {
            path: original.clone(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
        };
        let verified = VerifiedRuntime::copy(&artifact).unwrap();
        std::fs::remove_file(&original).unwrap();
        std::fs::write(&original, b"replaced executable bytes").unwrap();
        assert_ne!(verified.path, original);
        assert_eq!(std::fs::read(&verified.path).unwrap(), bytes);
        assert!(std::fs::write(&verified.path, b"mutated verified bytes").is_err());
        assert_eq!(std::fs::read(&verified.path).unwrap(), bytes);
        #[cfg(windows)]
        {
            let snapshot = verified.path.clone();
            let directory = snapshot.parent().unwrap().to_path_buf();
            drop(verified);
            assert!(!snapshot.exists());
            assert!(!directory.exists());
        }
    }

    #[test]
    fn model_and_tokenizer_bytes_are_owned_snapshots_after_verification() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("model");
        let bytes = b"verified model bytes";
        std::fs::write(&original, bytes).unwrap();
        let artifact = PinnedEmbeddingArtifact {
            path: original.clone(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
        };
        let verified = read_artifact(&artifact, 1024).unwrap();
        std::fs::write(original, b"different model").unwrap();
        assert_eq!(verified, bytes);
    }
}

#[cfg(feature = "local-embeddings")]
mod inference {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, Weak};

    use async_trait::async_trait;
    use fastembed::{
        InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
    };
    use provider_catalog::{
        EmbeddingProvider, EmbeddingRequest, EmbeddingResponse, EmbeddingTask, ProviderError,
    };
    use tokio::sync::Semaphore;

    #[cfg(test)]
    use super::{Digest, PinnedEmbeddingArtifact, Sha256};
    use super::{
        EmbeddingProfile, EmbeddingTaskFormat, LocalEmbeddingArtifacts, Result, VerifiedRuntime,
        invalid, read_artifact,
    };

    struct Model {
        embedding: TextEmbedding,
        tokenizer: tokenizers::Tokenizer,
    }
    type SharedModel = Arc<Mutex<Model>>;
    type ModelCache = HashMap<String, Weak<Mutex<Model>>>;
    static MODELS: OnceLock<Mutex<ModelCache>> = OnceLock::new();
    static RUNTIME: OnceLock<
        Mutex<Option<(String, VerifiedRuntime, ort::environment::Environment)>>,
    > = OnceLock::new();
    static INFERENCE: OnceLock<Arc<Semaphore>> = OnceLock::new();

    /// CPU-only, one inference at a time across bots, batches of two inputs.
    /// Identical artifact generations share model weights, never document data.
    pub struct LocalOnnxEmbedder {
        model: SharedModel,
        profile: EmbeddingProfile,
    }

    impl LocalOnnxEmbedder {
        /// Load only hash-verified local files. Call on a blocking worker.
        /// No environment-selected library or model download is used.
        ///
        /// # Errors
        /// Missing/oversized/changed artifacts, incompatible runtime/model, or
        /// more than two simultaneously active local model generations.
        pub fn load(artifacts: &LocalEmbeddingArtifacts) -> Result<Self> {
            let profile = artifacts.profile()?;
            let mut cache = MODELS
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.retain(|_, model| model.strong_count() > 0);
            if let Some(model) = cache
                .get(&profile.endpoint_identity)
                .and_then(Weak::upgrade)
            {
                return Ok(Self { model, profile });
            }
            if cache.len() >= 2 {
                return Err(invalid(
                    "two local embedding model generations are already active",
                ));
            }
            // Validate every artifact before loading native code. Only the
            // byte-based local loader is used; no Hub API is invoked.
            let runtime_copy = VerifiedRuntime::copy(&artifacts.runtime)?;
            let model_bytes = read_artifact(&artifacts.model, 512 * 1024 * 1024)?;
            let files = TokenizerFiles {
                tokenizer_file: read_artifact(&artifacts.tokenizer, 32 * 1024 * 1024)?,
                config_file: read_artifact(&artifacts.config, 4 * 1024 * 1024)?,
                special_tokens_map_file: read_artifact(
                    &artifacts.special_tokens_map,
                    4 * 1024 * 1024,
                )?,
                tokenizer_config_file: read_artifact(&artifacts.tokenizer_config, 4 * 1024 * 1024)?,
            };
            tokenizers::Tokenizer::from_bytes(&files.tokenizer_file)
                .map_err(|_| invalid("invalid local tokenizer"))?;
            tokenizers::utils::parallelism::set_parallelism(false);
            {
                let mut runtime = RUNTIME
                    .get_or_init(|| Mutex::new(None))
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some((hash, _copy, _env)) = runtime.as_ref() {
                    if hash != &artifacts.runtime.sha256 {
                        return Err(invalid("another ONNX runtime generation is already loaded"));
                    }
                } else {
                    let options = ort::init_from(&runtime_copy.path)
                        .map_err(|_| invalid("cannot load the pinned ONNX runtime library"))?;
                    // ort 2.0.0-rc.15 hands back the environment and only keeps a weak handle to
                    // it: hold it for the life of the process so telemetry stays disabled and
                    // later sessions reuse this environment instead of creating a default one.
                    // `build` fails when an environment already exists (the old `commit()` ==
                    // false case), so the "initialized outside the loader" stop stays.
                    let Ok(environment) = options.with_telemetry(false).build() else {
                        return Err(invalid(
                            "ONNX runtime was initialized outside the pinned local model loader",
                        ));
                    };
                    *runtime = Some((artifacts.runtime.sha256.clone(), runtime_copy, environment));
                }
            }
            let mut embedding = TextEmbedding::try_new_from_user_defined(
                UserDefinedEmbeddingModel::new(model_bytes, files).with_pooling(Pooling::Mean),
                InitOptionsUserDefined::new()
                    .with_max_length(512)
                    .with_intra_threads(1),
            )
            .map_err(|_| invalid("cannot load the pinned local E5 model"))?;
            // Use the exact tokenizer FastEmbed built with special-token files.
            // Disable its automatic truncation too: only our checked windows
            // enter inference, even if an artifact advertises a shorter limit.
            embedding
                .tokenizer
                .with_truncation(None)
                .map_err(|_| invalid("cannot disable local inference truncation"))?;
            let mut tokenizer = embedding.tokenizer.clone();
            tokenizer.with_padding(None);
            let model = Arc::new(Mutex::new(Model {
                embedding,
                tokenizer,
            }));
            cache.insert(profile.endpoint_identity.clone(), Arc::downgrade(&model));
            Ok(Self { model, profile })
        }

        #[must_use]
        pub fn profile(&self) -> &EmbeddingProfile {
            &self.profile
        }
    }

    struct CancelOnDrop(Arc<AtomicBool>);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    struct TokenWindow {
        text: String,
        weight: usize,
    }

    fn tokenizer_error() -> ProviderError {
        ProviderError::InvalidRequest("local tokenizer rejected an input".into())
    }

    /// Offset cuts cover every original byte exactly once. Prefixes and special
    /// tokens are included in the context check but not in pooling weights.
    fn token_windows(
        tokenizer: &tokenizers::Tokenizer,
        prefix: &str,
        text: &str,
    ) -> std::result::Result<Vec<TokenWindow>, ProviderError> {
        let prefix_tokens = tokenizer
            .encode(prefix, true)
            .map_err(|_| tokenizer_error())?
            .len();
        let mut remaining = text;
        let mut windows = Vec::new();
        while !remaining.is_empty() {
            if windows.len() == 8 {
                return Err(ProviderError::InvalidRequest(
                    "local E5 input exceeds eight tokenizer windows".into(),
                ));
            }
            let full = format!("{prefix}{remaining}");
            let encoding = tokenizer
                .encode(full.as_str(), true)
                .map_err(|_| tokenizer_error())?;
            if encoding.len() <= 512 {
                windows.push(TokenWindow {
                    text: full,
                    weight: encoding.len().saturating_sub(prefix_tokens).max(1),
                });
                break;
            }
            // Leave headroom for a cut changing word-boundary tokenization,
            // and verify the resulting window with the same exact tokenizer.
            let mut boundary_tokens = 480;
            let (window, consumed) = loop {
                let end = encoding
                    .get_offsets()
                    .iter()
                    .take(boundary_tokens)
                    .filter_map(|&(_, end)| end.checked_sub(prefix.len()))
                    .filter(|&end| {
                        end > 0 && end < remaining.len() && remaining.is_char_boundary(end)
                    })
                    .max()
                    .ok_or_else(tokenizer_error)?;
                let candidate = format!("{prefix}{}", &remaining[..end]);
                let tokens = tokenizer
                    .encode(candidate.as_str(), true)
                    .map_err(|_| tokenizer_error())?
                    .len();
                if tokens <= 512 {
                    break (
                        TokenWindow {
                            text: candidate,
                            weight: tokens.saturating_sub(prefix_tokens).max(1),
                        },
                        end,
                    );
                }
                boundary_tokens /= 2;
                if boundary_tokens == 0 {
                    return Err(tokenizer_error());
                }
            };
            windows.push(window);
            remaining = &remaining[consumed..];
        }
        Ok(windows)
    }

    struct WindowInput {
        input: usize,
        window: TokenWindow,
    }

    fn add_window_vector(
        sum: &mut [f64],
        vector: &[f32],
        weight: usize,
    ) -> std::result::Result<(), ProviderError> {
        if vector.len() != sum.len() || vector.iter().any(|value| !value.is_finite()) {
            return Err(ProviderError::InvalidRequest(
                "local E5 returned an invalid vector".into(),
            ));
        }
        let norm = vector
            .iter()
            .map(|value| f64::from(*value).powi(2))
            .sum::<f64>()
            .sqrt();
        if norm <= 0. || !norm.is_finite() {
            return Err(ProviderError::InvalidRequest(
                "local E5 returned an empty vector".into(),
            ));
        }
        let weight = f64::from(u32::try_from(weight).map_err(|_| tokenizer_error())?);
        for (coordinate, value) in sum.iter_mut().zip(vector) {
            *coordinate += (f64::from(*value) / norm) * weight;
        }
        Ok(())
    }

    fn finish_window_vector(sum: Vec<f64>) -> std::result::Result<Vec<f32>, ProviderError> {
        let norm = sum.iter().map(|value| value * value).sum::<f64>().sqrt();
        if !norm.is_finite() || norm <= 0. {
            return Err(ProviderError::InvalidRequest(
                "local E5 returned an empty vector".into(),
            ));
        }
        // Normalized coordinates are bounded to [-1,1]; no overflow on cast.
        #[allow(clippy::cast_possible_truncation)]
        Ok(sum.into_iter().map(|value| (value / norm) as f32).collect())
    }

    #[async_trait]
    impl EmbeddingProvider for LocalOnnxEmbedder {
        fn id(&self) -> &str {
            &self.profile.provider_id
        }
        fn endpoint_identity(&self) -> std::result::Result<String, ProviderError> {
            Ok(self.profile.endpoint_identity.clone())
        }
        fn is_remote(&self) -> bool {
            false
        }
        async fn embed(
            &self,
            request: EmbeddingRequest,
        ) -> std::result::Result<EmbeddingResponse, ProviderError> {
            request.validate()?;
            if request.format != EmbeddingTaskFormat::E5
                || request.dimensions != 384
                || request.model.model() != self.profile.model
            {
                return Err(ProviderError::InvalidRequest(
                    "local model requires its pinned E5 profile".into(),
                ));
            }
            let permit = INFERENCE
                .get_or_init(|| Arc::new(Semaphore::new(1)))
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| ProviderError::Connection("local inference worker closed".into()))?;
            let cancelled = Arc::new(AtomicBool::new(false));
            let _cancel_guard = CancelOnDrop(cancelled.clone());
            let model = self.model.clone();
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut model = model
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let prefix = match request.task {
                    EmbeddingTask::Document => "passage: ",
                    EmbeddingTask::Query => "query: ",
                };
                let mut windows = Vec::new();
                for (input, text) in request.inputs.iter().enumerate() {
                    if cancelled.load(Ordering::Acquire) {
                        return Err(ProviderError::Timeout);
                    }
                    windows.extend(
                        token_windows(&model.tokenizer, prefix, &text.text)?
                            .into_iter()
                            .map(|window| WindowInput { input, window }),
                    );
                }
                let mut sums = vec![vec![0.0_f64; 384]; request.inputs.len()];
                for batch in windows.chunks(2) {
                    if cancelled.load(Ordering::Acquire) {
                        return Err(ProviderError::Timeout);
                    }
                    let texts = batch
                        .iter()
                        .map(|input| input.window.text.as_str())
                        .collect::<Vec<_>>();
                    let vectors = model.embedding.embed(texts, Some(2)).map_err(|_| {
                        ProviderError::InvalidRequest("local E5 inference failed".into())
                    })?;
                    if vectors.len() != batch.len() {
                        return Err(ProviderError::InvalidRequest(
                            "local E5 returned an invalid vector count".into(),
                        ));
                    }
                    for (input, vector) in batch.iter().zip(vectors) {
                        add_window_vector(&mut sums[input.input], &vector, input.window.weight)?;
                    }
                }
                let vectors = sums
                    .into_iter()
                    .map(finish_window_vector)
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(EmbeddingResponse {
                    vectors,
                    model: Some("multilingual-e5-small".into()),
                    input_tokens: None,
                })
            })
            .await
            .map_err(|_| ProviderError::Connection("local inference worker failed".into()))?
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn fixture_tokenizer() -> tokenizers::Tokenizer {
            let model = tokenizers::models::wordlevel::WordLevel::builder()
                .vocab(
                    [
                        ("[UNK]".into(), 0),
                        ("[CLS]".into(), 1),
                        ("[SEP]".into(), 2),
                        ("passage".into(), 3),
                        ("query".into(), 4),
                        (":".into(), 5),
                    ]
                    .into_iter()
                    .collect(),
                )
                .unk_token("[UNK]".into())
                .build()
                .unwrap();
            let mut tokenizer = tokenizers::Tokenizer::new(model);
            tokenizer.with_pre_tokenizer(Some(tokenizers::pre_tokenizers::whitespace::Whitespace));
            tokenizer.with_post_processor(Some(
                tokenizers::processors::template::TemplateProcessing::builder()
                    .try_single("[CLS] $A [SEP]")
                    .unwrap()
                    .special_tokens(vec![("[CLS]", 1), ("[SEP]", 2)])
                    .build()
                    .unwrap(),
            ));
            tokenizer
        }

        #[test]
        fn local_token_windows_cover_unicode_and_code_without_truncation() {
            let tokenizer = fixture_tokenizer();
            let text = "İşlem veri_akışı 😺 SELECT tenant_id FROM müşteri;\n".repeat(180);
            for prefix in ["passage: ", "query: "] {
                let windows = token_windows(&tokenizer, prefix, &text).unwrap();
                assert!(windows.len() > 1 && windows.len() <= 8);
                let reconstructed = windows
                    .iter()
                    .map(|window| {
                        assert!(tokenizer.encode(window.text.as_str(), true).unwrap().len() <= 512);
                        assert!(window.weight > 0 && window.weight <= 512);
                        window.text.strip_prefix(prefix).unwrap()
                    })
                    .collect::<String>();
                assert_eq!(reconstructed, text);
            }
        }

        #[test]
        fn local_token_windows_reject_over_cap_and_preserve_fitting_input() {
            let tokenizer = fixture_tokenizer();
            let text = "Küçük fatura notu 😺";
            let windows = token_windows(&tokenizer, "passage: ", text).unwrap();
            assert_eq!(windows.len(), 1);
            assert_eq!(windows[0].text, format!("passage: {text}"));
            assert!(token_windows(&tokenizer, "passage: ", &"fatura ".repeat(4096)).is_err());
        }

        #[test]
        fn local_window_pooling_respects_weights_and_rejects_invalid_vectors() {
            let mut sum = vec![0.0_f64; 2];
            add_window_vector(&mut sum, &[2., 0.], 3).unwrap();
            add_window_vector(&mut sum, &[0., 4.], 1).unwrap();
            let pooled = finish_window_vector(sum).unwrap();
            assert!((pooled[0] - 3.0 / 10.0_f32.sqrt()).abs() < 1e-6);
            assert!((pooled[1] - 1.0 / 10.0_f32.sqrt()).abs() < 1e-6);
            assert!(add_window_vector(&mut [0.; 2], &[f32::NAN, 0.], 1).is_err());
            assert!(add_window_vector(&mut [0.; 2], &[0., 0.], 1).is_err());
            assert!(add_window_vector(&mut [0.; 2], &[1.], 1).is_err());
            assert!(finish_window_vector(vec![0., 0.]).is_err());
        }

        #[test]
        fn bad_hash_and_missing_artifacts_fail_without_loading_a_runtime() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("fake-runtime.bin");
            std::fs::write(&path, b"fixture").unwrap();
            let artifact = PinnedEmbeddingArtifact {
                path: path.clone(),
                sha256: "0".repeat(64),
            };
            assert!(read_artifact(&artifact, 100).is_err());
            let missing = PinnedEmbeddingArtifact {
                path: dir.path().join("absent"),
                sha256: "0".repeat(64),
            };
            assert!(read_artifact(&missing, 100).is_err());
            let valid = PinnedEmbeddingArtifact {
                path,
                sha256: format!("{:x}", Sha256::digest(b"fixture")),
            };
            assert_eq!(read_artifact(&valid, 100).unwrap(), b"fixture");
            assert!(read_artifact(&valid, 3).is_err());
        }
    }
}

#[cfg(feature = "local-embeddings")]
pub use inference::LocalOnnxEmbedder;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_generation_binds_all_artifact_hashes_but_not_machine_paths() {
        let root = std::env::temp_dir();
        let artifact = PinnedEmbeddingArtifact {
            path: root.join("fixture"),
            sha256: "0".repeat(64),
        };
        let mut artifacts = LocalEmbeddingArtifacts {
            runtime: artifact.clone(),
            model: artifact.clone(),
            tokenizer: artifact.clone(),
            config: artifact.clone(),
            special_tokens_map: artifact.clone(),
            tokenizer_config: artifact,
        };
        let profile = artifacts.profile().unwrap();
        artifacts.model.path = root.join("relocated");
        assert_eq!(profile, artifacts.profile().unwrap());
        artifacts.tokenizer.sha256 = "1".repeat(64);
        assert_ne!(profile, artifacts.profile().unwrap());
        artifacts.model.path = PathBuf::from("relative");
        assert!(artifacts.profile().is_err());
    }
}
