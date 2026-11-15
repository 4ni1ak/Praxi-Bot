//! Skill catalog and per-bot skill assignment routes (`/v1/skills`,
//! `/v1/bots/{id}/skills`), plus [`SkillAssignments`].

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Json;
use axum::extract::{Extension, Path as AxumPath, State};
use axum::http::StatusCode;
use protocol::BotId;
use serde::{Deserialize, Serialize};
use skills::hub::{HubPaths, SkillHub};
use storage::{SkillAssignmentMode, Storage};
use utoipa::ToSchema;

use crate::AppState;
use crate::error::{ApiError, ErrorBody};
use crate::routes::{ApiJson, id};

/// Maximum number of skill names allowed in a per-bot `selected` assignment.
pub const MAX_ASSIGNED_SKILLS: usize = 200;

/// [`::skills::AssignmentSource`] backed by [`Storage`] (`bot_skill_assignment`
/// in `bot.db`). The cache generation is read from the database (a counter its
/// triggers bump on every insert, update and delete of an assignment row), so a
/// write from any writer, including a state-bundle import on a live daemon or a
/// second connection, invalidates cached per-bot views; there is no in-memory
/// counter to forget to bump.
pub struct SkillAssignments {
    storage: Arc<Storage>,
    /// Counts failed generation reads; folded into the returned value so a
    /// failing database never produces a repeating (cacheable) generation.
    failures: AtomicU64,
}

impl SkillAssignments {
    /// Create a new assignment source backed by `storage`.
    #[must_use]
    pub fn new(storage: Arc<Storage>) -> Self {
        Self {
            storage,
            failures: AtomicU64::new(0),
        }
    }
}

impl ::skills::AssignmentSource for SkillAssignments {
    fn assignment(&self, bot_id: &str) -> Result<Option<::skills::Assignment>, String> {
        let parsed: uuid::Uuid = bot_id
            .parse()
            .map_err(|e| format!("invalid bot id `{bot_id}`: {e}"))?;
        let row = self
            .storage
            .get_bot_skill_assignment(BotId(parsed))
            .map_err(|e| e.to_string())?;
        Ok(row.map(|r| ::skills::Assignment {
            mode: match r.mode {
                SkillAssignmentMode::All => ::skills::AssignmentMode::All,
                SkillAssignmentMode::Selected => ::skills::AssignmentMode::Selected,
            },
            names: r.names.into_iter().collect(),
        }))
    }

    fn generation(&self) -> u64 {
        match self.storage.bot_skill_assignment_generation() {
            Ok(g) => g,
            // Unreadable: hand out a value no cached view carries (high bit
            // set, never repeats), so the view is rebuilt and fails closed.
            Err(_) => (1 << 63) | self.failures.fetch_add(1, Ordering::AcqRel),
        }
    }
}

/// Scope of a catalog skill (`shared` or `bundled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    /// User-shared or hub-installed skill (`<data_dir>/skills/`).
    Shared,
    /// Built-in skill bundled with Praxi-Bot.
    Bundled,
}

/// Scope where a skill load issue occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillIssueScope {
    /// Bundled skill layer.
    Bundled,
    /// Shared skill layer.
    Shared,
    /// Per-bot learned skill layer.
    Bot,
}

/// Auto-suggestion triggers declared in a skill's `when:` frontmatter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillRequiresDto {
    /// Domains (e.g. `github.com`).
    pub domains: Vec<String>,
    /// Application names.
    pub apps: Vec<String>,
    /// Task text keywords.
    pub keywords: Vec<String>,
}

/// Hub lockfile metadata for a shared skill installed via `SkillHub`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillHubInfoDto {
    /// Provenance tag (e.g. `git:https://...`, `local:...`).
    pub source: String,
    /// True when the on-disk tree matches the lockfile hash.
    pub locked: bool,
}

/// Catalog entry for a shared or bundled skill (metadata only, never the body).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogSkillDto {
    /// Unique skill name.
    pub name: String,
    /// One-line description from frontmatter.
    pub description: String,
    /// Skill version string.
    pub version: String,
    /// Skill scope (`shared` or `bundled`).
    pub scope: SkillScope,
    /// Optional `when:` triggers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires: Option<SkillRequiresDto>,
    /// Hub provenance when installed through `SkillHub`, else `null`.
    pub hub: Option<SkillHubInfoDto>,
}

/// A non-fatal skill load problem surfaced by `SkillIndex::issues`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillIssueDto {
    /// Layer where the problem occurred.
    pub scope: SkillIssueScope,
    /// Path (or `bundled:<name>`).
    pub path: String,
    /// Diagnostic message.
    pub message: String,
}

/// Response of `GET /v1/skills`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillsCatalogDto {
    /// Shared and bundled catalog skills.
    pub skills: Vec<CatalogSkillDto>,
    /// Non-fatal problems found while loading skills.
    pub issues: Vec<SkillIssueDto>,
    /// Host-local paths, omitted for paired peers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locations: Option<SkillLocationsDto>,
}

/// Stable storage locations for the owner's skill-management UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SkillLocationsDto {
    /// Shared owner-managed skill folder.
    pub shared: String,
    /// Per-bot skill folder template; these routes do not mutate it.
    pub bot_template: String,
    /// Bundled skills are embedded in the application.
    pub bundled: String,
}

/// Per-bot skill assignment mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillAssignmentModeDto {
    /// All shared and bundled skills are visible to the bot (default).
    All,
    /// Only the explicitly assigned shared/bundled skills are visible.
    Selected,
}

/// Availability of an assigned skill name in the current catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssignedSkillState {
    /// Present in the shared or bundled catalog.
    Ok,
    /// Assigned to this bot, but not currently installed in shared/bundled.
    Missing,
}

/// One explicitly assigned skill name and whether it is currently installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AssignedSkillDto {
    /// Assigned skill name.
    pub name: String,
    /// `ok` when present in catalog, `missing` otherwise.
    pub state: AssignedSkillState,
}

/// Catalog skill entry annotated with whether it is assigned to the bot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AvailableSkillDto {
    /// Unique skill name.
    pub name: String,
    /// One-line description from frontmatter.
    pub description: String,
    /// Skill version string.
    pub version: String,
    /// Skill scope (`shared` or `bundled`).
    pub scope: SkillScope,
    /// Optional `when:` triggers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires: Option<SkillRequiresDto>,
    /// Hub provenance when installed through `SkillHub`, else `null`.
    pub hub: Option<SkillHubInfoDto>,
    /// True when this skill is visible to the bot under its current assignment.
    pub assigned: bool,
}

/// A skill learned by the bot itself (per-bot scope, read-only here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OwnSkillDto {
    /// Skill name.
    pub name: String,
    /// Skill version string.
    pub version: String,
}

/// Response of `GET /v1/bots/{id}/skills` and `PUT /v1/bots/{id}/skills`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BotSkillsDto {
    /// Version of the stored assignment (`0` when none). Send it back as
    /// `expected_version` on `PUT` so a stale dialog gets `409` instead of
    /// overwriting a newer assignment.
    pub version: u64,
    /// Current assignment mode (`all` or `selected`).
    pub mode: SkillAssignmentModeDto,
    /// Explicitly assigned names when `mode == "selected"`.
    pub assigned: Vec<AssignedSkillDto>,
    /// Shared + bundled catalog entries with `assigned` state.
    pub available: Vec<AvailableSkillDto>,
    /// Skills learned by this bot (`bots/<id>/skills/`), read-only.
    pub own: Vec<OwnSkillDto>,
}

/// Body of `PUT /v1/bots/{id}/skills`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PutBotSkillsInput {
    /// Assignment mode (`all` or `selected`).
    pub mode: SkillAssignmentModeDto,
    /// Skill names to assign when `mode == "selected"` (max 200).
    #[serde(default)]
    pub names: Vec<String>,
    /// Version the client loaded (`BotSkillsDto::version`). When set and not
    /// current the write is refused with `409`; omit for an unconditional write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<u64>,
}

/// Strip credentials, query and fragment from a hub provenance tag such as
/// `git:https://user:token@host/repo?x=y`, so the catalog (also served to
/// paired phones) never carries a secret.
pub(crate) fn redact_source_tag(tag: &str) -> String {
    let Some(scheme_end) = tag.find("://") else {
        return tag.to_owned();
    };
    let (head, rest) = tag.split_at(scheme_end + 3);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let tail = tail.split(['?', '#']).next().unwrap_or("");
    format!("{head}{host}{tail}")
}

fn load_hub_map(data_dir: &Path) -> HashMap<String, SkillHubInfoDto> {
    let hub = SkillHub::new(HubPaths::from_data_dir(data_dir));
    let Ok(installed) = hub.list() else {
        return HashMap::new();
    };
    installed
        .into_iter()
        .map(|entry| {
            let name = entry.entry.name;
            let info = SkillHubInfoDto {
                source: redact_source_tag(&entry.source_tag),
                locked: entry.present && !entry.modified,
            };
            (name, info)
        })
        .collect()
}

fn catalog_entry(
    skill: &::skills::Skill,
    hub_skills_dir: &Path,
    hub_map: &HashMap<String, SkillHubInfoDto>,
) -> CatalogSkillDto {
    let scope = match skill.source {
        ::skills::Source::Bundled => SkillScope::Bundled,
        ::skills::Source::User | ::skills::Source::Bot => SkillScope::Shared,
    };
    let requires = (!skill.when.is_empty()).then(|| SkillRequiresDto {
        domains: skill.when.domains.clone(),
        apps: skill.when.apps.clone(),
        keywords: skill.when.keywords.clone(),
    });
    let hub = if skill.source == ::skills::Source::User
        && skill.dir.as_deref() == Some(hub_skills_dir.join(&skill.name).as_path())
    {
        hub_map.get(&skill.name).cloned()
    } else {
        None
    };
    CatalogSkillDto {
        name: skill.name.clone(),
        description: skill.description.clone(),
        version: skill.version.clone(),
        scope,
        requires,
        hub,
    }
}

fn issue_dto(issue: &::skills::SkillIssue) -> SkillIssueDto {
    let scope = match issue.source {
        ::skills::Source::Bundled => SkillIssueScope::Bundled,
        ::skills::Source::User => SkillIssueScope::Shared,
        ::skills::Source::Bot => SkillIssueScope::Bot,
    };
    SkillIssueDto {
        scope,
        path: issue.path.clone(),
        message: issue.message.clone(),
    }
}

fn build_catalog(
    index: &::skills::SkillIndex,
    data_dir: &Path,
) -> Result<SkillsCatalogDto, ApiError> {
    let (skills, issues) = index.catalog().map_err(ApiError::internal)?;
    let hub_map = load_hub_map(data_dir);
    let hub_skills_dir = HubPaths::from_data_dir(data_dir).skills_dir;
    Ok(SkillsCatalogDto {
        skills: skills
            .iter()
            .map(|s| catalog_entry(s, &hub_skills_dir, &hub_map))
            .collect(),
        issues: issues.iter().map(issue_dto).collect(),
        locations: Some(SkillLocationsDto {
            shared: data_dir.join("skills").to_string_lossy().into_owned(),
            bot_template: data_dir
                .join("bots")
                .join("{bot_id}")
                .join("skills")
                .to_string_lossy()
                .into_owned(),
            bundled: "embedded".into(),
        }),
    })
}

fn build_bot_skills(
    storage: &Storage,
    index: &::skills::SkillIndex,
    data_dir: &Path,
    bot_id: BotId,
) -> Result<BotSkillsDto, ApiError> {
    // A malformed row must not 500 the page: runtime fails closed for it, so it
    // is shown as `selected` with nothing assigned, and the user can overwrite
    // it (with the version reported here) to repair it.
    let (row, version) = match storage.inspect_bot_skill_assignment(bot_id)? {
        storage::SkillAssignmentLookup::Ok(row) => {
            let v = row.as_ref().map_or(0, |r| r.version);
            (row, v)
        }
        storage::SkillAssignmentLookup::Corrupt { version, .. } => (
            Some(storage::records::BotSkillAssignment {
                bot_id,
                mode: SkillAssignmentMode::Selected,
                names: Vec::new(),
                updated_at_ms: 0,
                version,
            }),
            version,
        ),
    };
    let (catalog, _) = index.catalog().map_err(ApiError::internal)?;
    let hub_map = load_hub_map(data_dir);
    let hub_skills_dir = HubPaths::from_data_dir(data_dir).skills_dir;

    let bot_view = index
        .for_bot(Some(&bot_id.to_string()))
        .map_err(ApiError::internal)?;
    let own = bot_view
        .list()
        .into_iter()
        .filter(|s| s.source == ::skills::Source::Bot)
        .map(|s| OwnSkillDto {
            name: s.name,
            version: s.version,
        })
        .collect();

    let (mode, selected_names) = match row {
        Some(r) if r.mode == SkillAssignmentMode::Selected => {
            (SkillAssignmentModeDto::Selected, r.names)
        }
        _ => (SkillAssignmentModeDto::All, Vec::new()),
    };

    let catalog_names: BTreeSet<&str> = catalog.iter().map(|s| s.name.as_str()).collect();
    let selected_set: BTreeSet<&str> = selected_names.iter().map(String::as_str).collect();

    let assigned = if mode == SkillAssignmentModeDto::Selected {
        selected_names
            .iter()
            .map(|name| AssignedSkillDto {
                name: name.clone(),
                state: if catalog_names.contains(name.as_str()) {
                    AssignedSkillState::Ok
                } else {
                    AssignedSkillState::Missing
                },
            })
            .collect()
    } else {
        Vec::new()
    };

    let available = catalog
        .iter()
        .map(|s| {
            let entry = catalog_entry(s, &hub_skills_dir, &hub_map);
            let is_assigned =
                mode == SkillAssignmentModeDto::All || selected_set.contains(entry.name.as_str());
            AvailableSkillDto {
                name: entry.name,
                description: entry.description,
                version: entry.version,
                scope: entry.scope,
                requires: entry.requires,
                hub: entry.hub,
                assigned: is_assigned,
            }
        })
        .collect();

    Ok(BotSkillsDto {
        version,
        mode,
        assigned,
        available,
        own,
    })
}

/// Catalog of shared and bundled skills (metadata only, never bodies).
#[utoipa::path(get, path = "/v1/skills", tag = "skills", security(("bearer" = [])),
    responses((status = 200, body = SkillsCatalogDto),
              (status = 503, body = ErrorBody, description = "skills are not available")))]
pub(crate) async fn list_skills(
    State(s): State<AppState>,
    peer: Option<Extension<crate::PeerIdentity>>,
) -> Result<Json<SkillsCatalogDto>, ApiError> {
    let index = s.skills_index()?;
    let data_dir = s.data_dir();
    let mut dto = tokio::task::spawn_blocking(move || build_catalog(&index, &data_dir))
        .await
        .map_err(ApiError::internal)??;
    if peer.is_some() {
        dto.locations = None;
    }
    s.agent().refresh_external_redactor();
    let redact = |text: &mut String| {
        *text = s.agent().redact_external_text(std::mem::take(text));
    };
    if let Some(locations) = &mut dto.locations {
        redact(&mut locations.shared);
        redact(&mut locations.bot_template);
    }
    for skill in &mut dto.skills {
        redact(&mut skill.name);
        redact(&mut skill.description);
        redact(&mut skill.version);
        if let Some(hub) = &mut skill.hub {
            redact(&mut hub.source);
        }
        if let Some(when) = &mut skill.requires {
            for text in when
                .domains
                .iter_mut()
                .chain(&mut when.apps)
                .chain(&mut when.keywords)
            {
                redact(text);
            }
        }
    }
    for issue in &mut dto.issues {
        redact(&mut issue.path);
        redact(&mut issue.message);
    }
    Ok(Json(dto))
}

/// Effective skill assignment, available catalog, and bot-learned skills for one bot.
#[utoipa::path(get, path = "/v1/bots/{id}/skills", tag = "skills", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Bot id")),
    responses((status = 200, body = BotSkillsDto),
              (status = 404, body = ErrorBody, description = "unknown bot"),
              (status = 503, body = ErrorBody, description = "skills are not available")))]
pub(crate) async fn get_bot_skills(
    State(s): State<AppState>,
    AxumPath(bid): AxumPath<String>,
) -> Result<Json<BotSkillsDto>, ApiError> {
    let bot = s.bot_of_user(BotId(id(&bid)?))?;
    let index = s.skills_index()?;
    let storage = s.storage_arc();
    let data_dir = s.data_dir();
    let dto =
        tokio::task::spawn_blocking(move || build_bot_skills(&storage, &index, &data_dir, bot.id))
            .await
            .map_err(ApiError::internal)??;
    Ok(Json(dto))
}

/// Assign shared/bundled skills to a bot (`mode: "all"` restores the default).
/// Local user or paired desktop only; peer phones are refused.
#[utoipa::path(put, path = "/v1/bots/{id}/skills", tag = "skills", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Bot id")),
    request_body = PutBotSkillsInput,
    responses((status = 200, body = BotSkillsDto),
              (status = 403, body = ErrorBody, description = "peer role not allowed"),
              (status = 404, body = ErrorBody, description = "unknown bot"),
              (status = 409, body = ErrorBody, description = "the assignment changed since `expected_version`"),
              (status = 422, body = ErrorBody, description = "invalid skill name or too many skills"),
              (status = 503, body = ErrorBody, description = "skills are not available")))]
pub(crate) async fn put_bot_skills(
    State(s): State<AppState>,
    peer: Option<Extension<crate::PeerIdentity>>,
    AxumPath(bid): AxumPath<String>,
    ApiJson(input): ApiJson<PutBotSkillsInput>,
) -> Result<Json<BotSkillsDto>, ApiError> {
    if peer
        .as_ref()
        .is_some_and(|p| p.role != crate::PeerRole::Desktop)
    {
        return Err(ApiError::forbidden(
            "only the local user or a paired desktop may assign bot skills",
        ));
    }
    let bot = s.bot_of_user(BotId(id(&bid)?))?;
    let index = s.skills_index()?;

    if input.names.len() > MAX_ASSIGNED_SKILLS {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "too_many_skills",
            format!("at most {MAX_ASSIGNED_SKILLS} skills may be assigned"),
        ));
    }
    let mut deduped = BTreeSet::new();
    for name in &input.names {
        ::skills::validate_name(name).map_err(|msg| {
            ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_skill_name", msg)
        })?;
        deduped.insert(name.clone());
    }
    let clean_names: Vec<String> = deduped.into_iter().collect();

    let storage = s.storage_arc();
    let user_id = s.user_id();
    let actor = peer
        .as_ref()
        .map_or_else(|| "user".to_owned(), |p| format!("desktop:{}", p.0.key));
    let mode = input.mode;
    let expected_version = input.expected_version;
    let storage_for_write = Arc::clone(&storage);
    let assignment_state = s.clone();
    tokio::task::spawn_blocking(move || -> Result<(), ApiError> {
        let _guard = assignment_state.0.skill_mutations.lock();
        let (storage_mode, mode_str, write_names): (SkillAssignmentMode, &str, Vec<String>) =
            match mode {
                SkillAssignmentModeDto::All => (SkillAssignmentMode::All, "all", Vec::new()),
                SkillAssignmentModeDto::Selected => {
                    (SkillAssignmentMode::Selected, "selected", clean_names)
                }
            };
        let detail = serde_json::json!({
            "bot_id": bot.id.to_string(),
            "mode": mode_str,
        });
        storage_for_write.set_bot_skill_assignment_audited_versioned(
            user_id,
            bot.id,
            storage_mode,
            &write_names,
            &actor,
            &detail,
            expected_version,
        )?;
        Ok(())
    })
    .await
    .map_err(ApiError::internal)??;

    // No cache bump: the views' cache key is the database generation, which
    // the commit above already advanced.

    let data_dir = s.data_dir();
    let dto =
        tokio::task::spawn_blocking(move || build_bot_skills(&storage, &index, &data_dir, bot.id))
            .await
            .map_err(ApiError::internal)??;
    Ok(Json(dto))
}
