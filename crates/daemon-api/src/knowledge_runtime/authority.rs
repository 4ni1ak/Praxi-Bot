//! Owner confirmations and host authority live outside portable bot/source JSON.
//! The caller must authenticate a local owner and hold the bot authority lane for
//! mutations. Every dispatch re-evaluates policy and the current index identity.

use memory_store::knowledge::{RemoteEmbeddingConsent, SourceUpdate};
use permission_engine::{ActionRequest, Decision, PermissionResolver};
use protocol::{InitiatorKind, PermissionCategory};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EmbeddingGrantTarget {
    Queries,
    Documents { source_id: Uuid },
}

impl EmbeddingGrantTarget {
    fn key(&self) -> String {
        match self {
            Self::Queries => "queries".into(),
            Self::Documents { source_id } => format!("source:{source_id}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EmbeddingApprovalChallenge {
    pub id: Uuid,
    pub target: EmbeddingGrantTarget,
    pub profile_fingerprint: String,
    pub provider_id: String,
    pub model: String,
    pub endpoint_identity: String,
    pub source_label: Option<String>,
    pub expires_at_ms: i64,
    pub policy_decision: String,
}

fn unavailable(_: impl std::fmt::Display) -> ApiError {
    ApiError::unavailable("embedding authority store unavailable")
}

fn open(state: &AppState, create: bool) -> Result<Option<Connection>, ApiError> {
    let path = state.data_dir().join("knowledge-authority.db");
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(ApiError::forbidden(
                "embedding authority must be a regular host file",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(unavailable(error));
        }
        _ => {}
    }
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
        | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW
        | if create {
            rusqlite::OpenFlags::SQLITE_OPEN_CREATE
        } else {
            rusqlite::OpenFlags::empty()
        };
    let conn = Connection::open_with_flags(path, flags).map_err(unavailable)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(unavailable)?;
    if create {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS grants(
            owner TEXT NOT NULL,bot TEXT NOT NULL,target TEXT NOT NULL,profile TEXT NOT NULL,
            index_epoch TEXT NOT NULL,source_identity TEXT NOT NULL,created_ms INTEGER NOT NULL,
            PRIMARY KEY(owner,bot,target,profile));
            CREATE TABLE IF NOT EXISTS challenges(
            id TEXT PRIMARY KEY,owner TEXT NOT NULL,bot TEXT NOT NULL,payload TEXT NOT NULL,
            index_epoch TEXT NOT NULL,source_identity TEXT NOT NULL,source_revision INTEGER,expires_ms INTEGER NOT NULL);").map_err(unavailable)?;
    }
    Ok(Some(conn))
}

fn index(state: &AppState, bot: BotId) -> Result<KnowledgeStore, ApiError> {
    KnowledgeStore::open_existing(
        state.data_dir(),
        Scope {
            user_id: state.user_id(),
            bot_id: bot,
        },
    )
    .map_err(unavailable)?
    .ok_or_else(|| ApiError::conflict("knowledge index is not configured"))
}

fn source_identity(
    store: &KnowledgeStore,
    target: &EmbeddingGrantTarget,
) -> Result<String, ApiError> {
    match target {
        EmbeddingGrantTarget::Queries => Ok(String::new()),
        EmbeddingGrantTarget::Documents { source_id } => {
            let source = store
                .source(*source_id)
                .map_err(unavailable)?
                .ok_or_else(|| ApiError::not_found("knowledge source"))?;
            if !source.enabled {
                return Err(ApiError::conflict("knowledge source is disabled"));
            }
            let bytes = serde_json::to_vec(&(source.kind, source.locator)).map_err(unavailable)?;
            Ok(format!("{:x}", Sha256::digest(bytes)))
        }
    }
}

/// Resolve the real permission engine, ignoring general tool/category grants.
/// A generic grant cannot authorize private knowledge egress.
fn decision(state: &AppState, bot: BotId) -> Result<Decision, ApiError> {
    let effective = state
        .agent()
        .effective_policy(bot)
        .map_err(|_| ApiError::forbidden("embedding permission policy unavailable"))?;
    if state.policy_load_error().is_some()
        || effective
            .categories
            .get(&PermissionCategory::Remote)
            .is_none_or(|entry| entry.decision == Decision::Deny)
    {
        return Ok(Decision::Deny);
    }
    Ok(PermissionResolver::new().resolve(
        &state.agent().global_policy(),
        effective.policy.as_ref(),
        None,
        &ActionRequest {
            category: PermissionCategory::Remote,
            raw_command: String::new(),
            initiator: InitiatorKind::Human,
        },
    ))
}

fn grantable(decision: &Decision) -> Result<(), ApiError> {
    if matches!(decision, Decision::Deny) {
        return Err(ApiError::forbidden(
            "embedding egress is denied by permission policy",
        ));
    }
    if decision.unsupported_limits().is_some() {
        return Err(ApiError::forbidden(
            "embedding standing consent cannot bypass unenforced permission limits",
        ));
    }
    Ok(())
}

pub fn grant_request(
    state: &AppState,
    bot: BotId,
    target: EmbeddingGrantTarget,
) -> Result<EmbeddingApprovalChallenge, ApiError> {
    let _policy = state.policy_lock();
    state.bot_of_user(bot)?;
    let config = configured_settings(state, bot)?;
    if config.profile.endpoint_identity.starts_with("local-onnx:") {
        return Err(ApiError::bad_request(
            "local embeddings do not need an egress grant",
        ));
    }
    let policy = decision(state, bot)?;
    grantable(&policy)?;
    let store = index(state, bot)?;
    let (source_label, revision) = match target {
        EmbeddingGrantTarget::Queries => (None, None),
        EmbeddingGrantTarget::Documents { source_id } => {
            let source = store
                .source(source_id)
                .map_err(unavailable)?
                .ok_or_else(|| ApiError::not_found("knowledge source"))?;
            if !source.enabled {
                return Err(ApiError::conflict("knowledge source is disabled"));
            }
            (Some(source.label), Some(source.revision))
        }
    };
    let now = storage::now_ms();
    let challenge = EmbeddingApprovalChallenge {
        id: Uuid::now_v7(),
        target,
        profile_fingerprint: config.profile.fingerprint().map_err(unavailable)?,
        provider_id: config.profile.provider_id,
        model: config.profile.model,
        endpoint_identity: config.profile.endpoint_identity,
        source_label,
        expires_at_ms: now.saturating_add(300_000),
        policy_decision: if policy == Decision::Allow {
            "allow"
        } else {
            "ask"
        }
        .into(),
    };
    let conn = open(state, true)?.ok_or_else(|| unavailable("authority"))?;
    conn.execute(
        "DELETE FROM challenges WHERE expires_ms<=?1 OR (owner=?2 AND bot=?3)",
        params![now, state.user_id().to_string(), bot.to_string()],
    )
    .map_err(unavailable)?;
    conn.execute("INSERT INTO challenges(id,owner,bot,payload,index_epoch,source_identity,source_revision,expires_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![challenge.id.to_string(),state.user_id().to_string(),bot.to_string(),serde_json::to_string(&challenge).map_err(unavailable)?,store.authority_epoch().map_err(unavailable)?,source_identity(&store,&challenge.target)?,revision.map(i64::try_from).transpose().map_err(unavailable)?,challenge.expires_at_ms]).map_err(unavailable)?;
    state
        .storage()
        .append_audit(
            state.user_id(),
            None,
            "user",
            "knowledge.embedding_approval_requested",
            &json!({"bot_id":bot,"challenge":challenge}),
        )
        .map_err(unavailable)?;
    Ok(challenge)
}

pub fn grant_confirm(
    state: &AppState,
    bot: BotId,
    challenge_id: Uuid,
) -> Result<RemoteEmbeddingConsent, ApiError> {
    let _policy = state.policy_lock();
    state.bot_of_user(bot)?;
    grantable(&decision(state, bot)?)?;
    let config = configured_settings(state, bot)?;
    let fingerprint = config.profile.fingerprint().map_err(unavailable)?;
    let store = index(state, bot)?;
    let epoch = store.authority_epoch().map_err(unavailable)?;
    let mut conn = open(state, false)?.ok_or_else(|| ApiError::not_found("embedding approval"))?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let saved: Option<(String,String,String,Option<u64>,i64)> = tx.query_row("SELECT payload,index_epoch,source_identity,source_revision,expires_ms FROM challenges WHERE id=?1 AND owner=?2 AND bot=?3",params![challenge_id.to_string(),state.user_id().to_string(),bot.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional().map_err(unavailable)?;
    let (payload, saved_epoch, saved_identity, revision, expiry) =
        saved.ok_or_else(|| ApiError::not_found("embedding approval"))?;
    let challenge: EmbeddingApprovalChallenge =
        serde_json::from_str(&payload).map_err(unavailable)?;
    if saved_identity != source_identity(&store, &challenge.target)?
        || saved_epoch != epoch
        || expiry <= storage::now_ms()
        || challenge.profile_fingerprint != fingerprint
    {
        return Err(ApiError::conflict(
            "embedding approval expired or configuration changed",
        ));
    }
    let consent = RemoteEmbeddingConsent {
        profile_fingerprint: fingerprint.clone(),
        granted_at_ms: storage::now_ms(),
    };
    // The source metadata is a revocation/filtering hint. The separate host row
    // below remains indispensable; writing this JSON alone grants nothing.
    if let EmbeddingGrantTarget::Documents { source_id } = challenge.target {
        let current = store
            .source(source_id)
            .map_err(unavailable)?
            .ok_or_else(|| ApiError::not_found("knowledge source"))?;
        if !current.enabled || Some(current.revision) != revision {
            return Err(ApiError::conflict(
                "knowledge source changed while approval was open",
            ));
        }
        drop(store);
        KnowledgeStore::open(
            state.data_dir(),
            Scope {
                user_id: state.user_id(),
                bot_id: bot,
            },
        )
        .map_err(unavailable)?
        .update_source(
            source_id,
            current.revision,
            SourceUpdate {
                remote_embedding: Some(Some(consent.clone())),
                ..SourceUpdate::default()
            },
        )
        .map_err(unavailable)?;
    }
    tx.execute("INSERT INTO grants(owner,bot,target,profile,index_epoch,source_identity,created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(owner,bot,target,profile) DO UPDATE SET index_epoch=excluded.index_epoch,source_identity=excluded.source_identity,created_ms=excluded.created_ms",params![state.user_id().to_string(),bot.to_string(),challenge.target.key(),fingerprint,epoch,saved_identity,consent.granted_at_ms]).map_err(unavailable)?;
    tx.execute(
        "DELETE FROM challenges WHERE id=?1",
        [challenge_id.to_string()],
    )
    .map_err(unavailable)?;
    tx.commit().map_err(unavailable)?;
    invalidate_bot(state, bot);
    state.storage().append_audit(state.user_id(),None,"user","knowledge.embedding_approved",&json!({"bot_id":bot,"challenge_id":challenge_id,"target":challenge.target,"profile":consent.profile_fingerprint})).map_err(unavailable)?;
    Ok(consent)
}

pub(crate) fn authorized(
    state: &AppState,
    bot: BotId,
    target: &EmbeddingGrantTarget,
    fingerprint: &str,
) -> Option<RemoteEmbeddingConsent> {
    state.bot_of_user(bot).ok()?;
    grantable(&decision(state, bot).ok()?).ok()?;
    let store = index(state, bot).ok()?;
    let epoch = store.authority_epoch().ok()?;
    let identity = source_identity(&store, target).ok()?;
    if let EmbeddingGrantTarget::Documents { source_id } = target {
        let source = store.source(*source_id).ok()??;
        if source
            .remote_embedding
            .is_none_or(|grant| grant.profile_fingerprint != fingerprint)
        {
            return None;
        }
    }
    let conn = open(state, false).ok()??;
    let created_ms: Option<i64> = conn.query_row("SELECT created_ms FROM grants WHERE owner=?1 AND bot=?2 AND target=?3 AND profile=?4 AND index_epoch=?5 AND source_identity=?6",params![state.user_id().to_string(),bot.to_string(),target.key(),fingerprint,epoch,identity],|row|row.get(0)).optional().ok()?;
    Some(RemoteEmbeddingConsent {
        profile_fingerprint: fingerprint.into(),
        granted_at_ms: created_ms?,
    })
}

pub fn revoke_grant(
    state: &AppState,
    bot: BotId,
    target: EmbeddingGrantTarget,
) -> Result<(), ApiError> {
    if let Some(conn) = open(state, false)? {
        conn.execute(
            "DELETE FROM grants WHERE owner=?1 AND bot=?2 AND target=?3",
            params![state.user_id().to_string(), bot.to_string(), target.key()],
        )
        .map_err(unavailable)?;
        conn.execute(
            "DELETE FROM challenges WHERE owner=?1 AND bot=?2",
            params![state.user_id().to_string(), bot.to_string()],
        )
        .map_err(unavailable)?;
    }
    if let EmbeddingGrantTarget::Documents { source_id } = target {
        if let Ok(store) = index(state, bot) {
            if let Some(source) = store.source(source_id).map_err(unavailable)? {
                if source.remote_embedding.is_some() {
                    drop(store);
                    KnowledgeStore::open(
                        state.data_dir(),
                        Scope {
                            user_id: state.user_id(),
                            bot_id: bot,
                        },
                    )
                    .map_err(unavailable)?
                    .update_source(
                        source_id,
                        source.revision,
                        SourceUpdate {
                            remote_embedding: Some(None),
                            ..SourceUpdate::default()
                        },
                    )
                    .map_err(unavailable)?;
                }
            }
        }
    }
    invalidate_bot(state, bot);
    Ok(())
}

pub fn revoke_bot_grants(state: &AppState, bot: BotId) -> Result<(), ApiError> {
    if let Some(conn) = open(state, false)? {
        for table in ["grants", "challenges"] {
            conn.execute(
                &format!("DELETE FROM {table} WHERE owner=?1 AND bot=?2"),
                params![state.user_id().to_string(), bot.to_string()],
            )
            .map_err(unavailable)?;
        }
    }
    invalidate_bot(state, bot);
    Ok(())
}
