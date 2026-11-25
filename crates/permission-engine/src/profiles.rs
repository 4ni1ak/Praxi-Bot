//! Per-site and per-app permission profiles (issue #227): the pure rule evaluator.
//!
//! A rule names ONE subject (a site's host or a program's image path), ONE class of
//! call ([`Capability`]) and an action:
//!
//! * [`ProfileAction::Block`] and [`ProfileAction::Ask`] only ever make a call
//!   stricter (a deny, or an approval that no grant, read shortcut or autonomy mode
//!   waives).
//! * [`ProfileAction::Allow`] is an explicit owner grant: the caller treats it like a
//!   remembered "always allow", i.e. it can only turn a plain policy-level ask into
//!   an allow. It never lifts a deny, the never-list, the money floor, the
//!   untrusted-content gate or an always-ask tool.
//!
//! This module only answers "which rule, if any, speaks for this subject and class?".
//! Where the subject comes from (the live browser origin, the verified program) and
//! where the verdict sits in the decision pipeline is the caller's job.
//!
//! Matching is strict on purpose: the host is normalised by the URL parser (so
//! punycode, case, a trailing dot and `user@` tricks cannot make one site look like
//! another); a subdomain match exists only for narrowing rules, never for an allow.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// The class of a call, from the least to the most consequential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Opening / navigating to a site.
    Read,
    /// Clicking, typing and picking values.
    Interact,
    /// Key presses, page scripts and other actions that can send a form.
    Submit,
    /// Every class (only for block and ask rules).
    Any,
}

impl Capability {
    /// Stable name (audit log, API).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Interact => "interact",
            Self::Submit => "submit",
            Self::Any => "any",
        }
    }

    fn covers(self, call: Capability) -> bool {
        self == Self::Any || self == call
    }
}

/// What a rule does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAction {
    /// The call is refused.
    Block,
    /// The call always needs a fresh approval.
    Ask,
    /// The owner allows it without asking (still below every floor).
    Allow,
}

impl ProfileAction {
    /// Stable name (audit log, API).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Ask => "ask",
            Self::Allow => "allow",
        }
    }

    /// Higher is stricter.
    const fn strictness(self) -> u8 {
        match self {
            Self::Allow => 0,
            Self::Ask => 1,
            Self::Block => 2,
        }
    }
}

/// What a rule's key names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    /// A website, by host.
    Site,
    /// A program, by image path.
    App,
}

impl SubjectKind {
    /// Stable name (audit log, API).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Site => "site",
            Self::App => "app",
        }
    }
}

/// The place a call acts on, already normalised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// A normalised host (see [`normalize_host`]).
    Site(String),
    /// A normalised image path (see [`normalize_app`]).
    App(String),
}

/// One stored rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRule {
    /// Stable id (for removal and the audit log).
    pub id: String,
    /// What `key` names.
    pub kind: SubjectKind,
    /// Normalised host or image path.
    pub key: String,
    /// Also match subdomains of `key` (sites only; never together with `allow`).
    #[serde(default)]
    pub include_subdomains: bool,
    /// The class of call.
    pub capability: Capability,
    /// What to do.
    pub action: ProfileAction,
    /// When the rule was created (ms since the Unix epoch).
    pub created_at_ms: i64,
    /// After this instant the rule no longer applies (`None`: until removed).
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
}

/// Why a rule was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    /// The site or program could not be understood.
    #[error("invalid subject: {0}")]
    Subject(&'static str),
    /// The combination of fields is not allowed.
    #[error("invalid rule: {0}")]
    Combination(&'static str),
}

/// What an owner (or an approval) asks for; [`ProfileRule::new`] checks it.
#[derive(Debug, Clone)]
pub struct NewRule<'a> {
    /// What `raw_key` names.
    pub kind: SubjectKind,
    /// Host or program path as typed / recorded.
    pub raw_key: &'a str,
    /// Also match subdomains (narrowing rules on sites only).
    pub include_subdomains: bool,
    /// The class of call.
    pub capability: Capability,
    /// What to do.
    pub action: ProfileAction,
    /// Expiry (ms since the Unix epoch), `None` for none.
    pub expires_at_ms: Option<i64>,
}

impl ProfileRule {
    /// Validate and normalise a new rule.
    ///
    /// # Errors
    /// [`RuleError`] when the key is not a plain host / absolute program path, or the
    /// combination would widen more than an explicit single-site grant may.
    pub fn new(id: String, new: &NewRule<'_>, now_ms: i64) -> Result<Self, RuleError> {
        let key = match new.kind {
            SubjectKind::Site => {
                normalize_host(new.raw_key).ok_or(RuleError::Subject("not a plain website name"))?
            }
            SubjectKind::App => normalize_app(new.raw_key)
                .ok_or(RuleError::Subject("not an absolute program path"))?,
        };
        if new.action == ProfileAction::Allow {
            if new.include_subdomains {
                return Err(RuleError::Combination(
                    "an allow rule covers exactly one host, never its subdomains",
                ));
            }
            if new.capability == Capability::Any {
                return Err(RuleError::Combination(
                    "an allow rule names one kind of action",
                ));
            }
        }
        if new.include_subdomains {
            if new.kind != SubjectKind::Site {
                return Err(RuleError::Combination("subdomains only apply to sites"));
            }
            if key.parse::<IpAddr>().is_ok() || !key.contains('.') {
                return Err(RuleError::Combination(
                    "subdomains need a domain name with at least two parts",
                ));
            }
        }
        if new.kind == SubjectKind::App && new.capability == Capability::Read {
            return Err(RuleError::Combination(
                "programs are judged on typing and key presses only",
            ));
        }
        if new.expires_at_ms.is_some_and(|e| e <= now_ms) {
            return Err(RuleError::Combination("the expiry is already in the past"));
        }
        Ok(Self {
            id,
            kind: new.kind,
            key,
            include_subdomains: new.include_subdomains,
            capability: new.capability,
            action: new.action,
            created_at_ms: now_ms,
            expires_at_ms: new.expires_at_ms,
        })
    }

    /// Whether the rule is still in force at `now_ms`.
    #[must_use]
    pub fn active(&self, now_ms: i64) -> bool {
        self.expires_at_ms.is_none_or(|e| now_ms < e)
    }

    fn speaks_for(&self, subject: &Subject) -> bool {
        match (self.kind, subject) {
            (SubjectKind::Site, Subject::Site(host)) => {
                *host == self.key
                    || (self.include_subdomains
                        && host.len() > self.key.len()
                        && host.ends_with(self.key.as_str())
                        && host.as_bytes()[host.len() - self.key.len() - 1] == b'.')
            }
            (SubjectKind::App, Subject::App(path)) => *path == self.key,
            _ => false,
        }
    }
}

/// The rule that decides a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// What to do.
    pub action: ProfileAction,
    /// The rule that said so.
    pub rule_id: String,
}

/// The strictest active rule that names `subject` for a call of class `class`
/// (`block` over `ask` over `allow`; ties keep the earlier rule).
///
/// `allow_trusted` is false when the subject could only be named loosely (a program
/// the OS could not vouch for): an allow rule then does not speak.
#[must_use]
pub fn evaluate(
    rules: &[ProfileRule],
    subject: &Subject,
    class: Capability,
    now_ms: i64,
    allow_trusted: bool,
) -> Option<Verdict> {
    let mut best: Option<&ProfileRule> = None;
    for r in rules {
        if !r.active(now_ms)
            || !r.capability.covers(class)
            || !r.speaks_for(subject)
            || (r.action == ProfileAction::Allow && !allow_trusted)
        {
            continue;
        }
        if best.is_none_or(|b| r.action.strictness() > b.action.strictness()) {
            best = Some(r);
        }
    }
    best.map(|r| Verdict {
        action: r.action,
        rule_id: r.id.clone(),
    })
}

/// Normalise a host the owner typed (a plain host name: no scheme, port, path,
/// credentials or spaces) to the form the URL parser would give it (lower case,
/// punycode, no trailing dot). `None` for anything else.
#[must_use]
pub fn normalize_host(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > 253
        || raw
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/\\@:?#[]%*".contains(c))
    {
        return None;
    }
    let url = url::Url::parse(&format!("https://{raw}/")).ok()?;
    host_of(&url)
}

/// The host of an `http(s)` URL or origin as a rule key, `None` for any other scheme
/// or a URL without a host. Userinfo, ports, paths and case never reach the result.
#[must_use]
pub fn site_of_url(url: &str) -> Option<String> {
    let url = url::Url::parse(url.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    host_of(&url)
}

fn host_of(url: &url::Url) -> Option<String> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Normalise a program image path: `/` separators, no `.` parts, refused when relative
/// or containing `..` / NUL. A Windows drive path is lower-cased (case-insensitive).
#[must_use]
pub fn normalize_app(path: &str) -> Option<String> {
    let path = path.trim().replace('\\', "/");
    if path.is_empty() || path.len() > 1024 || path.contains('\0') {
        return None;
    }
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic();
    if !(drive && bytes[2] == b'/' || path.starts_with('/') && !path.starts_with("//")) {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p => parts.push(p),
        }
    }
    if parts.is_empty() || (drive && parts.len() < 2) {
        return None;
    }
    let joined = parts.join("/");
    Some(if drive {
        joined.to_ascii_lowercase()
    } else {
        format!("/{joined}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(
        kind: SubjectKind,
        key: &str,
        sub: bool,
        capability: Capability,
        action: ProfileAction,
    ) -> Result<ProfileRule, RuleError> {
        ProfileRule::new(
            "r".into(),
            &NewRule {
                kind,
                raw_key: key,
                include_subdomains: sub,
                capability,
                action,
                expires_at_ms: None,
            },
            1000,
        )
    }

    fn rule(key: &str, sub: bool, cap: Capability, action: ProfileAction) -> ProfileRule {
        make(SubjectKind::Site, key, sub, cap, action).unwrap()
    }

    fn site(url: &str) -> Subject {
        Subject::Site(site_of_url(url).expect("host"))
    }

    fn hit(rules: &[ProfileRule], url: &str) -> bool {
        evaluate(rules, &site(url), Capability::Read, 0, true).is_some()
    }

    #[test]
    fn host_tricks_never_match_the_wrong_site() {
        let rules = [rule(
            "example.com",
            true,
            Capability::Any,
            ProfileAction::Block,
        )];
        assert!(hit(&rules, "https://example.com/"));
        assert!(hit(&rules, "https://EXAMPLE.com./x"));
        assert!(hit(&rules, "https://www.example.com:8443/"));
        // userinfo: the host is evil.test, not example.com
        assert!(!hit(&rules, "https://example.com@evil.test/"));
        assert!(hit(&rules, "https://evil.test@example.com/"));
        // the name only appears elsewhere in the URL
        assert!(!hit(&rules, "https://evil.test/?u=example.com"));
        assert!(!hit(&rules, "https://evil.test/example.com"));
        // label confusion
        assert!(!hit(&rules, "https://example.com.evil.test/"));
        assert!(!hit(&rules, "https://notexample.com/"));
        assert!(!hit(&rules, "https://evilexample.com/"));
        // lookalike (Cyrillic a) is a different, punycode host
        assert!(!hit(&rules, "https://ex\u{430}mple.com/"));
    }

    #[test]
    fn punycode_keys_and_unicode_input_agree() {
        let k = normalize_host("b\u{fc}cher.example").unwrap();
        assert_eq!(k, "xn--bcher-kva.example");
        assert_eq!(normalize_host("XN--BCHER-KVA.example.").unwrap(), k);
        let rules = [rule(&k, false, Capability::Read, ProfileAction::Block)];
        assert!(hit(&rules, "https://b\u{fc}cher.example/"));
        assert!(!hit(&rules, "https://bucher.example/"));
    }

    #[test]
    fn typed_keys_must_be_plain_hosts() {
        for bad in [
            "https://example.com",
            "example.com/path",
            "user@example.com",
            "example.com:8080",
            "*.example.com",
            "exa mple.com",
            "",
            "example.com#x",
            "[::1]",
        ] {
            assert_eq!(normalize_host(bad), None, "{bad}");
        }
        assert_eq!(normalize_host(" Example.COM. ").unwrap(), "example.com");
    }

    #[test]
    fn strictest_rule_wins_and_expired_rules_are_ignored() {
        let mut allow = rule(
            "example.com",
            false,
            Capability::Interact,
            ProfileAction::Allow,
        );
        allow.id = "a".into();
        let mut ask = rule("example.com", true, Capability::Any, ProfileAction::Ask);
        ask.id = "k".into();
        let mut block = rule(
            "example.com",
            false,
            Capability::Interact,
            ProfileAction::Block,
        );
        block.id = "b".into();
        let s = site("https://example.com/");
        let v = |rules: &[ProfileRule]| evaluate(rules, &s, Capability::Interact, 50, true);
        assert_eq!(v(&[allow.clone()]).unwrap().action, ProfileAction::Allow);
        assert_eq!(v(&[allow.clone(), ask.clone()]).unwrap().rule_id, "k");
        assert_eq!(
            v(&[ask.clone(), block.clone(), allow.clone()])
                .unwrap()
                .rule_id,
            "b"
        );
        block.expires_at_ms = Some(40);
        assert_eq!(
            v(&[block, allow.clone()]).unwrap().action,
            ProfileAction::Allow
        );
        // an allow does not speak for another class, or when the subject is not trusted
        assert!(
            evaluate(
                std::slice::from_ref(&allow),
                &s,
                Capability::Submit,
                50,
                true
            )
            .is_none()
        );
        assert!(
            evaluate(
                std::slice::from_ref(&allow),
                &s,
                Capability::Interact,
                50,
                false
            )
            .is_none()
        );
    }

    #[test]
    fn an_allow_rule_can_never_be_wide() {
        let mk = |sub, cap, action| make(SubjectKind::Site, "example.com", sub, cap, action);
        assert!(mk(true, Capability::Read, ProfileAction::Allow).is_err());
        assert!(mk(false, Capability::Any, ProfileAction::Allow).is_err());
        assert!(mk(false, Capability::Read, ProfileAction::Allow).is_ok());
        assert!(mk(true, Capability::Any, ProfileAction::Block).is_ok());
        // a single-label or IP key cannot take subdomains
        for key in ["com", "10.0.0.1"] {
            assert!(
                make(
                    SubjectKind::Site,
                    key,
                    true,
                    Capability::Any,
                    ProfileAction::Block
                )
                .is_err()
            );
        }
        // an expiry in the past is refused
        let past = ProfileRule::new(
            "x".into(),
            &NewRule {
                kind: SubjectKind::Site,
                raw_key: "example.com",
                include_subdomains: false,
                capability: Capability::Read,
                action: ProfileAction::Ask,
                expires_at_ms: Some(10),
            },
            10,
        );
        assert!(past.is_err());
    }

    #[test]
    fn app_paths_are_normalised_and_matched_exactly() {
        assert_eq!(
            normalize_app("C:\\Windows\\System32\\Notepad.EXE").unwrap(),
            "c:/windows/system32/notepad.exe"
        );
        assert_eq!(normalize_app("/usr/bin/./foo").unwrap(), "/usr/bin/foo");
        for bad in [
            "notepad.exe",
            "C:/a/../b.exe",
            "/a/../b",
            "//server/share/x",
            "C:",
            "",
        ] {
            assert_eq!(normalize_app(bad), None, "{bad}");
        }
        let r = make(
            SubjectKind::App,
            "C:\\Windows\\notepad.exe",
            false,
            Capability::Interact,
            ProfileAction::Allow,
        )
        .unwrap();
        let on = |p: &str| {
            evaluate(
                std::slice::from_ref(&r),
                &Subject::App(normalize_app(p).unwrap()),
                Capability::Interact,
                0,
                true,
            )
            .is_some()
        };
        assert!(on("c:/windows/NOTEPAD.exe"));
        assert!(!on("c:/temp/notepad.exe"));
        // a site rule never speaks for a program of the same text
        let site_rule = rule("example.com", false, Capability::Any, ProfileAction::Block);
        assert!(
            evaluate(
                &[site_rule],
                &Subject::App("example.com".into()),
                Capability::Read,
                0,
                true
            )
            .is_none()
        );
    }
}
