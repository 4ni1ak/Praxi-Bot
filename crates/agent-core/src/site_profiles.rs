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
//!   A program that cannot be identified fails CLOSED: the strictest `block` / `ask` rule
//!   of the bot that covers the class applies (an `allow` rule never matches it).
//! * Reading a page (`browser_snapshot`, `browser_screenshot`) and `browser_back` are judged
//!   by the origin of the page that is open; after any browser call the page the browser
//!   LANDED on is checked against the `block` rules (a redirect or a click may have moved it),
//!   and a blocked page's content is withheld from the model.
//! * Rules apply on every computer the bot uses (the owner's computer and the host): a block
//!   is never weaker on another machine, and the owner sees that scope on the rules card.
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

use crate::shared::{RunCtx, Shared, is_page_action, is_page_read};

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
        "browser_open" | "browser_snapshot" | "browser_screenshot" | "browser_back" => {
            Some(Capability::Read)
        }
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
        t if is_page_action(t) || is_page_read(t) => {
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
        let origin = ctx.page_origin.lock().clone();
        self.profile_verdict_at(ctx.bot_id, name, input, origin.as_deref(), target)
    }

    /// [`Self::profile_verdict`] for an explicit page origin (a parked approval is judged by
    /// the origin it was shown for, not by whatever page is open now).
    pub(crate) fn profile_verdict_at(
        &self,
        bot_id: BotId,
        name: &str,
        input: &Value,
        origin: Option<&str>,
        target: Option<&ControlTarget>,
    ) -> Option<Verdict> {
        let class = classify(name)?;
        let now = storage::now_ms();
        let rules: Vec<ProfileRule> = {
            let all = self.profile_rules.lock();
            all.iter()
                .filter(|(b, r)| *b == bot_id && r.active(now))
                .map(|(_, r)| r.clone())
                .collect()
        };
        if rules.is_empty() {
            return None;
        }
        let known = subject_of(name, input, origin, target);
        if let Some((subject, trusted)) = &known {
            return evaluate(&rules, subject, class, now, *trusted);
        }
        let covers = |r: &ProfileRule, kind: SubjectKind| {
            r.kind == kind
                && r.action != ProfileAction::Allow
                && (r.capability == Capability::Any || r.capability == class)
        };
        // A browser action that cannot be placed: ask when a narrowing site rule would
        // cover it somewhere (a navigation to an unreadable address keeps the default).
        if name.starts_with("browser_") && (class != Capability::Read || is_page_read(name)) {
            return rules
                .iter()
                .find(|r| covers(r, SubjectKind::Site))
                .map(|r| Verdict {
                    action: ProfileAction::Ask,
                    rule_id: r.id.clone(),
                });
        }
        // A desktop input call whose program could not be identified fails closed: the
        // strictest block / ask rule on a program that covers its class speaks for it.
        if matches!(name, "desktop_type" | "desktop_set_value" | "desktop_key") {
            return rules
                .iter()
                .filter(|r| covers(r, SubjectKind::App))
                .max_by_key(|r| u8::from(r.action == ProfileAction::Block))
                .map(|r| Verdict {
                    action: r.action,
                    rule_id: r.id.clone(),
                });
        }
        None
    }

    /// After a browser call: the page the browser LANDED on (a redirect from an allowed
    /// site, a click or a back step) is checked against the block rules. `Some(text)` means
    /// the page is blocked and what the call returned must not reach the model.
    pub(crate) async fn blocked_landing(
        &self,
        ctx: &RunCtx,
        tool: &std::sync::Arc<dyn tool_registry::Tool>,
        tctx: &tool_registry::ToolContext,
    ) -> Option<String> {
        if !self.has_profile_rules(ctx.bot_id) {
            return None;
        }
        let landed = tool.landed_origin(tctx).await?;
        let host = site_of_url(&landed)?;
        let now = storage::now_ms();
        let rules: Vec<ProfileRule> = {
            let all = self.profile_rules.lock();
            all.iter()
                .filter(|(b, r)| *b == ctx.bot_id && r.active(now))
                .map(|(_, r)| r.clone())
                .collect()
        };
        let v = evaluate(&rules, &Subject::Site(host), Capability::Read, now, true)?;
        (v.action == ProfileAction::Block).then(|| {
            self.audit_profile_applied(ctx, "landing", &v);
            "Permission denied: the page this ended up on is blocked by an owner rule for              that site, so its content is withheld. Do not go back to it; choose another              approach or tell the user."
                .to_owned()
        })
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
