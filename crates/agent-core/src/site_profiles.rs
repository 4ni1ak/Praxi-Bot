//! Per-site and per-app permission profiles in the decision pipeline (issue #227).
//!
//! The rules and their matching live in [`permission_engine::profiles`]; this module
//! decides what a call acts on and keeps the owner's rules.
//!
//! * The site is the destination of `browser_open`, or the origin the BROWSER reports
//!   for the frame that would receive a click / typing (never a tool argument or page
//!   text); the program is the verified image of a desktop input call.
//! * A browser action whose origin cannot be read, when the bot has a narrowing site
//!   rule that would cover its class, asks (it cannot be shown to be somewhere safe).
//!   A program that cannot be identified simply matches no rule (the default applies).
//! * `block` and `ask` rules sit above grants, read shortcuts and autonomy modes; an
//!   `allow` rule counts as one more remembered grant and sits below every floor.
//! * Rules are kept as audit-log events (`profile_rule.set` / `.removed`), like the
//!   "always allow" grants: audited by construction, durable, and not part of the bot
//!   profile file, so profile sharing / export can never carry them.

use permission_engine::{
    Capability, ProfileAction, ProfileRule, Subject, SubjectKind, Verdict, evaluate, normalize_app,
    site_of_url,
};
use protocol::BotId;
use serde_json::{Value, json};
use storage::Storage;
use tool_registry::ControlTarget;
use uuid::Uuid;

use crate::shared::{RunCtx, Shared, is_page_action};

/// Audit action: a rule was created.
pub(crate) const AUDIT_PROFILE_SET: &str = "profile_rule.set";
/// Audit action: a rule was removed.
pub(crate) const AUDIT_PROFILE_REMOVED: &str = "profile_rule.removed";
/// Audit action: a rule decided (or narrowed) a call.
pub(crate) const AUDIT_PROFILE_APPLIED: &str = "profile_rule.applied";

/// How many rules one bot may hold.
pub(crate) const MAX_RULES_PER_BOT: usize = 200;

/// The class of a built-in browser or desktop tool call; `None` for every other tool.
#[must_use]
pub(crate) fn classify(tool: &str) -> Option<Capability> {
    match tool {
        "browser_open" => Some(Capability::Read),
        "browser_click" | "browser_fill" | "browser_select" | "desktop_type"
        | "desktop_set_value" => Some(Capability::Interact),
        "browser_press" | "browser_act" | "browser_eval" | "desktop_key" => {
            Some(Capability::Submit)
        }
        _ => None,
    }
}

/// What the call acts on, and whether the subject is exact enough for an `allow` rule.
/// `None`: not known (the caller decides what that means).
fn subject_of(
    tool: &str,
    input: &Value,
    page_origin: Option<&str>,
    target: Option<&ControlTarget>,
) -> Option<(Subject, bool)> {
    match tool {
        "browser_open" => {
            let host = site_of_url(input.get("url")?.as_str()?)?;
            Some((Subject::Site(host), true))
        }
        t if is_page_action(t) => {
            let host = site_of_url(page_origin?)?;
            Some((Subject::Site(host), true))
        }
        "desktop_type" | "desktop_set_value" | "desktop_key" => {
            let target = target?;
            let path = normalize_app(&target.identity.image_path)?;
            Some((Subject::App(path), target.eligible()))
        }
        _ => None,
    }
}

/// The subject of an approved call as `(kind, key, class)`, for "add from a recent
/// approval". Same derivation as the decision, so what is offered is what would match.
#[must_use]
pub(crate) fn approval_subject(
    tool: &str,
    input: &Value,
    page_origin: Option<&str>,
    target: Option<&ControlTarget>,
) -> Option<(SubjectKind, String, Capability)> {
    let class = classify(tool)?;
    match subject_of(tool, input, page_origin, target)?.0 {
        Subject::Site(h) => Some((SubjectKind::Site, h, class)),
        Subject::App(p) => Some((SubjectKind::App, p, class)),
    }
}

impl Shared {
    /// Whether the bot has any rule (so the program a desktop call reaches is worth resolving).
    pub(crate) fn has_profile_rules(&self, bot: BotId) -> bool {
        self.profile_rules.lock().iter().any(|(b, _)| *b == bot)
    }

    /// The rule of this bot that speaks for this call, if any.
    pub(crate) fn profile_verdict(
        &self,
        ctx: &RunCtx,
        name: &str,
        input: &Value,
        target: Option<&ControlTarget>,
    ) -> Option<Verdict> {
        let class = classify(name)?;
        let now = storage::now_ms();
        let rules: Vec<ProfileRule> = {
            let all = self.profile_rules.lock();
            all.iter()
                .filter(|(b, r)| *b == ctx.bot_id && r.active(now))
                .map(|(_, r)| r.clone())
                .collect()
        };
        if rules.is_empty() {
            return None;
        }
        let origin = ctx.page_origin.lock().clone();
        let known = subject_of(name, input, origin.as_deref(), target);
        if let Some((subject, trusted)) = &known {
            return evaluate(&rules, subject, class, now, *trusted);
        }
        // A browser action that cannot be placed: ask when a narrowing site rule would
        // cover it somewhere (a read of an unknown page and a program the OS could not
        // name keep the default, as the issue asks).
        if name.starts_with("browser_") && class != Capability::Read {
            return rules
                .iter()
                .find(|r| {
                    r.kind == SubjectKind::Site
                        && r.action != ProfileAction::Allow
                        && (r.capability == Capability::Any || r.capability == class)
                })
                .map(|r| Verdict {
                    action: ProfileAction::Ask,
                    rule_id: r.id.clone(),
                });
        }
        None
    }

    /// Why a call was blocked by a rule, for the model and the owner's activity feed.
    pub(crate) fn profile_block_text(
        &self,
        ctx: &RunCtx,
        name: &str,
        input: &Value,
        target: Option<&ControlTarget>,
    ) -> Option<String> {
        let v = self.profile_verdict(ctx, name, input, target)?;
        (v.action == ProfileAction::Block).then(|| {
            "Permission denied: the owner set a rule for this site or program that blocks this. \
             Do not retry it; choose another approach or tell the user."
                .to_owned()
        })
    }

    /// Record that a rule decided or narrowed a call.
    pub(crate) fn audit_profile_applied(&self, ctx: &RunCtx, tool: &str, v: &Verdict) {
        self.audit(
            ctx,
            "system",
            AUDIT_PROFILE_APPLIED,
            &json!({
                "bot_id": ctx.bot_id.to_string(),
                "tool": tool,
                "rule_id": v.rule_id,
                "action": v.action.as_str(),
            }),
        );
    }
}

/// Rebuild every bot's rules from the audit log, oldest event first.
pub(crate) fn load_profile_rules(storage: &Storage) -> Vec<(BotId, ProfileRule)> {
    let mut rules: Vec<(BotId, ProfileRule)> = Vec::new();
    for user in storage.list_users().unwrap_or_default() {
        for e in storage.list_audit(user.id).unwrap_or_default() {
            let bot = e.detail["bot_id"]
                .as_str()
                .and_then(|s| s.parse::<Uuid>().ok())
                .map(BotId);
            let Some(bot) = bot else { continue };
            match e.action.as_str() {
                AUDIT_PROFILE_SET => {
                    if let Ok(rule) =
                        serde_json::from_value::<ProfileRule>(e.detail["rule"].clone())
                    {
                        rules.retain(|(_, r)| r.id != rule.id);
                        rules.push((bot, rule));
                    }
                }
                AUDIT_PROFILE_REMOVED => {
                    if let Some(id) = e.detail["rule_id"].as_str() {
                        rules.retain(|(b, r)| !(*b == bot && r.id == id));
                    }
                }
                _ => {}
            }
        }
    }
    rules
}

/// One site or program the owner recently approved something on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalSubject {
    /// Site or program.
    pub kind: SubjectKind,
    /// Normalised host or image path.
    pub key: String,
    /// The class of the approved call.
    pub capability: Capability,
    /// When it was approved (ms since the Unix epoch).
    pub approved_at_ms: i64,
}

/// Recent approvals of one bot that named a site or program, newest first, one per
/// subject and class.
pub(crate) fn recent_subjects(
    storage: &Storage,
    user: protocol::UserId,
    bot: BotId,
    limit: usize,
) -> Vec<ApprovalSubject> {
    let since = storage::now_ms() - 30 * 24 * 3600 * 1000;
    let mut entries = storage
        .list_audit_action_since(user, "approval.approved", since)
        .unwrap_or_default();
    entries.reverse();
    let mut seen: std::collections::HashSet<(SubjectKind, String, Capability)> =
        std::collections::HashSet::new();
    let mut out = Vec::new();
    for e in entries {
        if e.detail["bot_id"].as_str() != Some(&bot.to_string()) {
            continue;
        }
        let s = &e.detail["subject"];
        let kind = match s["kind"].as_str() {
            Some("site") => SubjectKind::Site,
            Some("app") => SubjectKind::App,
            _ => continue,
        };
        let capability = match s["capability"].as_str() {
            Some("read") => Capability::Read,
            Some("interact") => Capability::Interact,
            Some("submit") => Capability::Submit,
            _ => continue,
        };
        let Some(key) = s["key"].as_str() else {
            continue;
        };
        if seen.insert((kind, key.to_owned(), capability)) {
            out.push(ApprovalSubject {
                kind,
                key: key.to_owned(),
                capability,
                approved_at_ms: e.ts_ms,
            });
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}
