//! A bot that owns a mailbox gets the `email_*` tools by itself.
//!
//! Bots do not get the e-mail tools by default. Making the owner also edit
//! the bot's allowed tools after binding an inbox to it is a manual step that
//! is easy to miss (the bot then says "e-mail is switched off" about an inbox
//! it owns). So when the owner binds an account to a bot, the owner-gated
//! account routes switch the tool family on for that bot, and switch it off
//! again when the last binding goes away.
//!
//! * It is the owner's own action: the account routes sit in
//!   [`crate::owner::OWNER_ROUTES`], so a bot (or any holder of only the API
//!   token) can never reach this code.
//! * It only widens what the profile's tool list shows. Sending stays gated by
//!   the identity's send mode, the guard and the approval card.
//! * It is revocable and traceable: what was added is recorded in the profile
//!   (`tools.mail_auto`), so removing the binding removes exactly that and never
//!   a tool the owner added himself. An owner who switched the tools off by
//!   hand is not overruled by a later account edit.
//! * Every change writes an audit row (bot and account names only).

use std::collections::BTreeSet;

use globset::Glob;
use protocol::BotId;
use serde_json::{Value, json};

use crate::AppState;
use crate::email::EmailAccountDto;

/// The glob that hands a bot the whole mail tool family.
pub(crate) const MAIL_GLOB: &str = "email_*";
/// A tool every mail tool set contains; its visibility stands for "mail is on".
const PROBE: &str = "email_read";
/// Key in the profile's `[tools]` table that records what this module changed.
pub(crate) const MARKER: &str = "mail_auto";

fn strings(tools: &Value, key: &str) -> Vec<String> {
    tools
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn glob_matches(glob: &str, name: &str) -> bool {
    Glob::new(glob).is_ok_and(|g| g.compile_matcher().is_match(name))
}

fn matches_any(globs: &[String], name: &str) -> bool {
    globs.iter().any(|g| glob_matches(g, name))
}

/// Whether a profile's `[tools]` table shows the mail tools (an empty allow
/// list shows every tool).
pub(crate) fn mail_visible(config: &Value) -> bool {
    let tools = config.get("tools").unwrap_or(&Value::Null);
    let (allow, exclude) = (strings(tools, "allow"), strings(tools, "exclude"));
    (allow.is_empty() || matches_any(&allow, PROBE)) && !matches_any(&exclude, PROBE)
}

fn tools_mut(config: &mut Value) -> Option<&mut serde_json::Map<String, Value>> {
    let root = config.as_object_mut()?;
    root.entry("tools")
        .or_insert_with(|| json!({}))
        .as_object_mut()
}

fn set_list(tools: &mut serde_json::Map<String, Value>, key: &str, list: &[String]) {
    if list.is_empty() {
        tools.remove(key);
    } else {
        tools.insert(key.to_owned(), json!(list));
    }
}

/// Switch the mail tools on in `config` and record what was changed. Returns
/// whether anything changed: nothing happens when the tools are already shown
/// (the owner's own setting), or when this module added them before and the
/// owner has switched them off since (his decision stands).
pub(crate) fn enable(config: &mut Value) -> bool {
    if mail_visible(config) {
        return false;
    }
    let had_marker = config
        .pointer(&format!("/tools/{MARKER}"))
        .is_some_and(Value::is_object);
    if had_marker {
        return false;
    }
    let Some(tools) = tools_mut(config) else {
        return false;
    };
    let view = Value::Object(tools.clone());
    let (allow, mut exclude) = (strings(&view, "allow"), strings(&view, "exclude"));
    // Only the entries that hide the mail family leave `exclude`: an owner who
    // excluded `email_send` on purpose keeps that.
    let removed: Vec<String> = exclude
        .iter()
        .filter(|g| glob_matches(g, PROBE))
        .cloned()
        .collect();
    exclude.retain(|g| !glob_matches(g, PROBE));
    let mut added_allow = false;
    if !allow.is_empty() && !matches_any(&allow, PROBE) {
        let mut next = allow;
        next.push(MAIL_GLOB.to_owned());
        set_list(tools, "allow", &next);
        added_allow = true;
    }
    if !removed.is_empty() {
        set_list(tools, "exclude", &exclude);
    }
    tools.insert(
        MARKER.to_owned(),
        json!({ "added_allow": added_allow, "removed_exclude": removed }),
    );
    true
}

/// Undo what [`enable`] recorded, and nothing else. Returns whether the
/// profile changed (a stale marker is dropped silently).
pub(crate) fn disable_auto(config: &mut Value) -> bool {
    let Some(marker) = config
        .pointer(&format!("/tools/{MARKER}"))
        .filter(|m| m.is_object())
        .cloned()
    else {
        return false;
    };
    let Some(tools) = tools_mut(config) else {
        return false;
    };
    let view = Value::Object(tools.clone());
    if marker
        .get("added_allow")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let allow = strings(&view, "allow");
        let rest: Vec<String> = allow
            .iter()
            .filter(|g| g.as_str() != MAIL_GLOB)
            .cloned()
            .collect();
        if rest.len() != allow.len() {
            if rest.is_empty() {
                // An empty allow list would mean "every tool": keep the list
                // and hide the mail family by exclusion instead.
                let mut exclude = strings(&view, "exclude");
                if !exclude.iter().any(|g| g == MAIL_GLOB) {
                    exclude.push(MAIL_GLOB.to_owned());
                }
                set_list(tools, "exclude", &exclude);
            } else {
                set_list(tools, "allow", &rest);
            }
        }
    }
    let removed = strings(&marker, "removed_exclude");
    if !removed.is_empty() {
        let mut exclude = strings(&Value::Object(tools.clone()), "exclude");
        for g in removed {
            if !exclude.contains(&g) {
                exclude.push(g);
            }
        }
        set_list(tools, "exclude", &exclude);
    }
    tools.remove(MARKER);
    true
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Whether `account` is an identity of `bot`: bound to it, shared with it or
/// marked as its default sender.
pub(crate) fn binds(account: &EmailAccountDto, bot: &str) -> bool {
    account.bot.as_deref().is_some_and(|b| same(b, bot))
        || account.shared_with.iter().any(|b| same(b, bot))
        || account.default_for.iter().any(|b| same(b, bot))
}

/// Every bot an account is bound to.
pub(crate) fn bots_of(account: &EmailAccountDto) -> BTreeSet<String> {
    account
        .bot
        .iter()
        .chain(&account.shared_with)
        .chain(&account.default_for)
        .map(|b| b.to_ascii_lowercase())
        .collect()
}

/// Bring the mail tools of every bot in `affected` in line with `accounts`
/// (the accounts as they are after the change): a bot with an account gets the
/// tools, a bot without one loses what this module gave it. Best effort: a
/// failure never fails the account change itself.
pub(crate) fn sync(s: &AppState, accounts: &[EmailAccountDto], affected: &BTreeSet<String>) {
    for raw in affected {
        let Ok(uuid) = raw.parse() else { continue };
        let id = BotId(uuid);
        // Only the owner's own bots; an id from a list nobody checked is skipped.
        if s.bot_of_user(id).is_err() {
            continue;
        }
        let bound = accounts.iter().find(|a| binds(a, raw));
        let account = bound.map(|a| a.name.clone());
        let result = s.storage().modify_bot(id, |name, stored| {
            let mut next = stored.clone();
            let changed = if bound.is_some() {
                enable(&mut next)
            } else {
                disable_auto(&mut next)
            };
            if changed {
                Ok((name.to_owned(), next))
            } else {
                Err("unchanged".to_owned())
            }
        });
        // Unchanged, a bot that is gone, or a storage error: nothing to do.
        if let Ok(storage::BotPatch::Applied { bot, .. }) = result {
            s.refresh_profile(&bot);
            let (action, detail) = if bound.is_some() {
                (
                    "email.bot_tools.enable",
                    json!({"bot": raw, "account": account}),
                )
            } else {
                ("email.bot_tools.disable", json!({"bot": raw}))
            };
            // The change is made and the audit row is its trace; if the row
            // cannot be written the owner still sees the tools in the bot's
            // settings, so this never fails the account change.
            let _ = s
                .storage()
                .append_audit(s.user_id(), None, "user", action, &detail);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(tools: &Value) -> Value {
        json!({ "bot": {"id": "x", "name": "X"}, "tools": tools })
    }

    #[test]
    fn enabling_adds_the_glob_once_and_disabling_takes_back_only_that() {
        let mut c = cfg(&json!({ "allow": ["fs_read", "web_*"] }));
        assert!(!mail_visible(&c));
        assert!(enable(&mut c));
        assert_eq!(c["tools"]["allow"], json!(["fs_read", "web_*", "email_*"]));
        assert!(mail_visible(&c));
        // Idempotent.
        let again = c.clone();
        assert!(!enable(&mut c));
        assert_eq!(c, again);
        assert!(disable_auto(&mut c));
        assert_eq!(c["tools"]["allow"], json!(["fs_read", "web_*"]));
        assert!(c["tools"].get(MARKER).is_none());
        assert!(!disable_auto(&mut c));
    }

    #[test]
    fn tools_the_owner_added_himself_are_never_taken_away() {
        let mut c = cfg(&json!({ "allow": ["fs_read", "email_*"] }));
        assert!(!enable(&mut c), "already on: nothing to record");
        assert!(!disable_auto(&mut c));
        assert_eq!(c["tools"]["allow"], json!(["fs_read", "email_*"]));
        // An empty allow list already shows everything.
        let mut all = cfg(&json!({}));
        assert!(!enable(&mut all));
        assert!(!disable_auto(&mut all));
    }

    #[test]
    fn an_exclusion_that_hid_the_mail_tools_comes_back_on_removal() {
        let mut c = cfg(&json!({ "exclude": ["email_*", "shell_run"] }));
        assert!(enable(&mut c));
        assert_eq!(c["tools"]["exclude"], json!(["shell_run"]));
        assert!(mail_visible(&c));
        assert!(disable_auto(&mut c));
        assert_eq!(c["tools"]["exclude"], json!(["shell_run", "email_*"]));
        assert!(!mail_visible(&c));
    }

    #[test]
    fn an_owner_who_switched_the_tools_off_is_not_overruled() {
        let mut c = cfg(&json!({ "allow": ["fs_read"] }));
        assert!(enable(&mut c));
        // The owner hides them again from the bot panel.
        c["tools"]["allow"] = json!(["fs_read"]);
        c["tools"]["exclude"] = json!(["email_*"]);
        assert!(!enable(&mut c), "the marker says: already offered once");
    }

    #[test]
    fn a_specific_send_exclusion_of_the_owner_survives() {
        let mut c = cfg(&json!({ "allow": ["fs_read"], "exclude": ["email_send"] }));
        assert!(enable(&mut c));
        assert_eq!(c["tools"]["exclude"], json!(["email_send"]));
        assert!(disable_auto(&mut c));
        assert_eq!(c["tools"]["exclude"], json!(["email_send"]));
    }
}
