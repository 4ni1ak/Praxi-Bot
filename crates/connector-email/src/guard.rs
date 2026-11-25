//! Outgoing-mail guardrails (ADR-0035, item ML-1). All of it lives in code, outside the model.
//!
//! * a **send log** (who, when, which bot, message id) the owner can open and export;
//! * an **opt-out list** (addresses and whole domains) for people who asked not to be written to;
//! * an **opt-out line** appended in code to the body (the model never writes or removes it);
//! * a **daily count** per account and a **same-recipient wait**.
//!
//! The count, the wait and the opt-out list are *soft* guards: [`MailGuard::check`] returns
//! [`Warning`]s, never a refusal. Only the owner can send past a warning, by approving the card
//! that shows it ("Send anyway"). The tool freezes the warnings it showed
//! ([`MailGuard::freeze`]) and a later call goes through only when it matches a frozen entry
//! exactly ([`MailGuard::take_override`]). The model has no argument that can do this.
//!
//! State is a JSON settings file, a JSON opt-out file and an append-only JSONL send log under one
//! directory (or memory only, for tests).

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Default: at most this many mails per account in a rolling 24 hours before a warning.
pub const DEFAULT_DAILY_LIMIT: u32 = 20;
/// Default: warn when the same recipient got a mail from the account in the last N days.
pub const DEFAULT_SAME_RECIPIENT_DAYS: u32 = 14;
/// How long a frozen warning set stays valid after the card was shown.
const FREEZE_TTL: Duration = Duration::from_secs(30 * 60);
/// Longest subject kept in the log.
const SUBJECT_KEEP: usize = 120;
/// Longest footer text accepted from settings.
pub const MAX_FOOTER_CHARS: usize = 1000;
/// Most opt-out entries the list may hold.
pub const MAX_OPT_OUTS: usize = 20_000;

/// The sentence shown once in a warning that is about opt-out rights.
pub const LEGAL_NOTICE: &str = "Ignoring someone's opt-out on commercial e-mail can be unlawful \
(in Turkey: Law No. 6563 on electronic commerce, ETK) and can lead to fines. \
This is not legal advice.";

/// Default opt-out line (Turkish first, English second). `{sender}` is the sending address.
pub const DEFAULT_FOOTER: &str = "Bu iletiyi almak istemiyorsanız, bu e-postayı \"ÇIKMAK İSTİYORUM\" yazarak yanıtlayın; {sender} adresinden size bir daha yazılmaz.\nIf you do not want to receive e-mail from {sender}, reply with the words \"UNSUBSCRIBE\" and we will not write to you again.";

/// Owner-changeable values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuardSettings {
    /// Warn above this many sends per account in 24 hours. `0` turns the warning off.
    pub daily_limit: u32,
    /// Warn when a recipient got mail from the account within this many days. `0` = off.
    pub same_recipient_days: u32,
    /// Whether the opt-out line is added by default.
    pub opt_out_line: bool,
    /// Text of the opt-out line; `{sender}` is replaced by the sending address. Empty = default.
    pub footer: String,
}

impl Default for GuardSettings {
    fn default() -> Self {
        Self {
            daily_limit: DEFAULT_DAILY_LIMIT,
            same_recipient_days: DEFAULT_SAME_RECIPIENT_DAYS,
            opt_out_line: true,
            footer: String::new(),
        }
    }
}

impl GuardSettings {
    /// Reject values that cannot work.
    ///
    /// # Errors
    /// A short plain sentence.
    pub fn validate(&self) -> Result<(), String> {
        if self.footer.chars().count() > MAX_FOOTER_CHARS {
            return Err(format!(
                "the opt-out text is too long (at most {MAX_FOOTER_CHARS} characters)"
            ));
        }
        if self.footer.contains('\0') {
            return Err("the opt-out text contains an invalid character".into());
        }
        if self.daily_limit > 100_000 || self.same_recipient_days > 3650 {
            return Err("that limit is unreasonably large".into());
        }
        Ok(())
    }

    fn footer_for(&self, sender: &str) -> String {
        let t = if self.footer.trim().is_empty() {
            DEFAULT_FOOTER
        } else {
            self.footer.trim()
        };
        t.replace("{sender}", sender)
    }
}

/// One address or domain the owner will not write to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptOut {
    /// `ann@example.com` or `example.com` (a whole domain), lowercase.
    pub value: String,
    /// `address` or `domain`.
    pub kind: String,
    /// Unix seconds when it was added.
    pub added_at: u64,
    /// Free note from the owner (why / where it came from).
    #[serde(default)]
    pub note: String,
}

/// One line of the send log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendLogEntry {
    /// Unix seconds.
    pub at: u64,
    /// Same moment as UTC text, for people.
    pub at_utc: String,
    /// Bot that sent (`None`: not a bot).
    pub bot: Option<String>,
    /// Run that sent.
    pub run: String,
    /// Account name.
    pub account: String,
    /// Sending address.
    pub from: String,
    /// Every recipient (to, cc and bcc), lowercase.
    pub recipients: Vec<String>,
    /// First characters of the subject.
    pub subject: String,
    /// Provider message id or the server's reply line.
    pub message_id: String,
    /// The opt-out line was in the mail.
    pub opt_out_line: bool,
    /// The owner approved past these warnings ("Send anyway"); empty = none were shown.
    pub overridden: Vec<String>,
}

/// One reason to stop and ask the owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    /// `opt_out_listed`, `daily_limit`, `same_recipient` or `no_opt_out_line`.
    pub code: String,
    /// Plain English sentence (the UI shows its own translation by `code`).
    pub text: String,
    /// Addresses it is about (opt-out hit or repeat recipient).
    pub addresses: Vec<String>,
    /// Numbers for the sentence: sends so far / limit / days.
    pub count: u32,
    /// The limit or the number of days.
    pub limit: u32,
}

/// What [`MailGuard::check`] found.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GuardReport {
    /// Warnings, possibly none.
    pub warnings: Vec<Warning>,
    /// Set when a warning is about opt-out rights: the one-time legal sentence.
    pub legal_notice: Option<String>,
    /// The opt-out line that will be appended (`None` when it is off for this mail).
    pub footer: Option<String>,
}

impl GuardReport {
    /// Sorted warning codes.
    #[must_use]
    pub fn codes(&self) -> Vec<String> {
        let mut c: Vec<String> = self.warnings.iter().map(|w| w.code.clone()).collect();
        c.sort();
        c
    }
}

/// A mail as the guard sees it.
#[derive(Debug, Clone)]
pub struct MailFacts<'a> {
    /// Bot that sends.
    pub bot: Option<&'a str>,
    /// Account name.
    pub account: &'a str,
    /// Sending address.
    pub from: &'a str,
    /// Normalized recipients (lowercase plain addresses).
    pub recipients: &'a [String],
    /// Whether the opt-out line stays on (`false` only when the model asked to drop it).
    pub opt_out_line: bool,
}

struct Frozen {
    codes: Vec<String>,
    token: String,
    at: Instant,
}

#[derive(Default)]
struct State {
    settings: GuardSettings,
    opt_outs: Vec<OptOut>,
    log: Vec<SendLogEntry>,
    frozen: HashMap<String, Frozen>,
}

/// The guard. Cheap to share behind an `Arc`.
pub struct MailGuard {
    dir: Option<PathBuf>,
    state: Mutex<State>,
}

/// Seconds since the Unix epoch.
#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `2026-11-24T10:00:00Z` for `secs`.
#[must_use]
pub fn utc_text(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Lowercase plain address from `Name <a@b>` or `a@b`; `None` when it has no `@`.
#[must_use]
pub fn plain_address(s: &str) -> Option<String> {
    let t = s.trim();
    let inner = match (t.rfind('<'), t.rfind('>')) {
        (Some(a), Some(b)) if a < b => &t[a + 1..b],
        _ => t,
    };
    let inner = inner.trim().to_ascii_lowercase();
    (inner.contains('@') && !inner.contains(char::is_whitespace)).then_some(inner)
}

/// Normalize an owner-typed opt-out entry (`a@b.c`, `@b.c` or `b.c`).
///
/// # Errors
/// A short plain sentence.
pub fn normalize_opt_out(raw: &str) -> Result<(String, &'static str), String> {
    let t = raw.trim().to_ascii_lowercase();
    if t.is_empty() || t.len() > 254 || t.contains(char::is_whitespace) || t.contains(['<', '>']) {
        return Err(
            "enter one e-mail address (ann@example.com) or one domain (example.com)".into(),
        );
    }
    if let Some(domain) = t.strip_prefix('@') {
        return domain_entry(domain);
    }
    if t.contains('@') {
        let (local, domain) = t.rsplit_once('@').unwrap_or(("", ""));
        if local.is_empty() || !domain.contains('.') {
            return Err("that does not look like an e-mail address".into());
        }
        return Ok((t, "address"));
    }
    domain_entry(&t)
}

fn domain_entry(d: &str) -> Result<(String, &'static str), String> {
    if d.contains('@')
        || !d.contains('.')
        || d.starts_with('.')
        || d.ends_with('.')
        || !d
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
    {
        return Err("that does not look like a domain (example.com)".into());
    }
    Ok((d.to_owned(), "domain"))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MailGuard {
    /// Memory only (tests, and the default until the host gives a directory).
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            dir: None,
            state: Mutex::new(State::default()),
        }
    }

    /// Load (or start) the files under `dir`. Unreadable files are treated as empty and the
    /// problem is logged: a damaged log must not stop the owner's mail, but it must not be
    /// silently overwritten either (the damaged line is skipped, the rest is kept).
    #[must_use]
    pub fn open(dir: PathBuf) -> Self {
        let mut st = State::default();
        if let Ok(t) = std::fs::read_to_string(dir.join("guard-settings.json")) {
            match serde_json::from_str::<GuardSettings>(&t) {
                Ok(s) if s.validate().is_ok() => st.settings = s,
                _ => log::warn!("email: guard-settings.json is invalid, using defaults"),
            }
        }
        if let Ok(t) = std::fs::read_to_string(dir.join("opt-out.json")) {
            match serde_json::from_str::<Vec<OptOut>>(&t) {
                Ok(v) => st.opt_outs = v,
                Err(_) => log::warn!("email: opt-out.json is unreadable, the list starts empty"),
            }
        }
        if let Ok(t) = std::fs::read_to_string(dir.join("send-log.jsonl")) {
            st.log = t
                .lines()
                .filter_map(|l| serde_json::from_str::<SendLogEntry>(l).ok())
                .collect();
        }
        Self {
            dir: Some(dir),
            state: Mutex::new(st),
        }
    }

    fn write_json(&self, name: &str, text: &str) -> Result<(), String> {
        let Some(dir) = &self.dir else { return Ok(()) };
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot save: {e}"))?;
        let tmp = dir.join(format!("{name}.tmp"));
        std::fs::write(&tmp, text).map_err(|e| format!("cannot save: {e}"))?;
        std::fs::rename(&tmp, dir.join(name)).map_err(|e| format!("cannot save: {e}"))
    }

    /// Current settings.
    #[must_use]
    pub fn settings(&self) -> GuardSettings {
        lock(&self.state).settings.clone()
    }

    /// Replace the settings (owner only; the daemon route classifies it).
    ///
    /// # Errors
    /// Invalid values or a failed save.
    pub fn set_settings(&self, s: GuardSettings) -> Result<(), String> {
        s.validate()?;
        let text = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?;
        self.write_json("guard-settings.json", &text)?;
        lock(&self.state).settings = s;
        Ok(())
    }

    /// The opt-out list, newest first.
    #[must_use]
    pub fn opt_outs(&self) -> Vec<OptOut> {
        let mut v = lock(&self.state).opt_outs.clone();
        v.reverse();
        v
    }

    /// Add an address or domain (idempotent).
    ///
    /// # Errors
    /// Invalid entry, list full, failed save.
    pub fn add_opt_out(&self, raw: &str, note: &str) -> Result<OptOut, String> {
        let (value, kind) = normalize_opt_out(raw)?;
        let mut st = lock(&self.state);
        if let Some(e) = st.opt_outs.iter().find(|o| o.value == value) {
            return Ok(e.clone());
        }
        if st.opt_outs.len() >= MAX_OPT_OUTS {
            return Err("the opt-out list is full".into());
        }
        let entry = OptOut {
            value,
            kind: kind.into(),
            added_at: now_secs(),
            note: note.chars().take(200).collect(),
        };
        let mut next = st.opt_outs.clone();
        next.push(entry.clone());
        let text = serde_json::to_string_pretty(&next).map_err(|e| e.to_string())?;
        self.write_json("opt-out.json", &text)?;
        st.opt_outs = next;
        Ok(entry)
    }

    /// Remove an entry; `false` when it was not there.
    ///
    /// # Errors
    /// A failed save.
    pub fn remove_opt_out(&self, raw: &str) -> Result<bool, String> {
        let want = raw.trim().trim_start_matches('@').to_ascii_lowercase();
        let mut st = lock(&self.state);
        let next: Vec<OptOut> = st
            .opt_outs
            .iter()
            .filter(|o| o.value != want)
            .cloned()
            .collect();
        if next.len() == st.opt_outs.len() {
            return Ok(false);
        }
        let text = serde_json::to_string_pretty(&next).map_err(|e| e.to_string())?;
        self.write_json("opt-out.json", &text)?;
        st.opt_outs = next;
        Ok(true)
    }

    /// The send log, newest first (at most `limit`).
    #[must_use]
    pub fn log(&self, limit: usize) -> Vec<SendLogEntry> {
        lock(&self.state)
            .log
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// The whole log as CSV (oldest first), for the owner's export. Cells that start with a
    /// formula character are prefixed so a spreadsheet never runs them.
    #[must_use]
    pub fn export_csv(&self) -> String {
        fn cell(s: &str) -> String {
            let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
                format!("'{s}")
            } else {
                s.to_owned()
            };
            format!("\"{}\"", s.replace('"', "\"\""))
        }
        let mut out = String::from(
            "time_utc,bot,run,account,from,recipients,subject,message_id,opt_out_line,send_anyway\r\n",
        );
        for e in &lock(&self.state).log {
            let row = [
                cell(&e.at_utc),
                cell(e.bot.as_deref().unwrap_or("")),
                cell(&e.run),
                cell(&e.account),
                cell(&e.from),
                cell(&e.recipients.join("; ")),
                cell(&e.subject),
                cell(&e.message_id),
                cell(if e.opt_out_line { "yes" } else { "no" }),
                cell(&e.overridden.join("; ")),
            ];
            out.push_str(&row.join(","));
            out.push_str("\r\n");
        }
        out
    }

    /// Evaluate the soft guards for `facts`. Never refuses.
    #[must_use]
    pub fn check(&self, facts: &MailFacts<'_>) -> GuardReport {
        let st = lock(&self.state);
        let now = now_secs();
        let mut warnings = Vec::new();

        let hits: Vec<String> = facts
            .recipients
            .iter()
            .filter(|r| {
                let domain = r.rsplit_once('@').map_or("", |(_, d)| d);
                st.opt_outs
                    .iter()
                    .any(|o| o.value == **r || (o.kind == "domain" && o.value == domain))
            })
            .cloned()
            .collect();
        let mut legal = false;
        if !hits.is_empty() {
            legal = true;
            warnings.push(Warning {
                code: "opt_out_listed".into(),
                text: format!(
                    "These recipients asked not to be written to (opt-out list): {}.",
                    hits.join(", ")
                ),
                addresses: hits,
                count: 0,
                limit: 0,
            });
        }

        let day_ago = now.saturating_sub(86_400);
        let sent_today: usize = st
            .log
            .iter()
            .filter(|e| e.at >= day_ago && e.account == facts.account)
            .map(|e| e.recipients.len().max(1))
            .sum();
        let limit = st.settings.daily_limit;
        let planned = facts.recipients.len();
        if limit > 0 && sent_today + planned > limit as usize {
            warnings.push(Warning {
                code: "daily_limit".into(),
                text: format!(
                    "This would make {} e-mails from this account in 24 hours; your limit is {limit}.",
                    sent_today + planned
                ),
                addresses: Vec::new(),
                count: u32::try_from(sent_today + planned).unwrap_or(u32::MAX),
                limit,
            });
        }

        let days = st.settings.same_recipient_days;
        if days > 0 {
            let since = now.saturating_sub(u64::from(days) * 86_400);
            let repeat: Vec<String> = facts
                .recipients
                .iter()
                .filter(|r| {
                    st.log
                        .iter()
                        .any(|e| e.at >= since && e.recipients.iter().any(|x| x == *r))
                })
                .cloned()
                .collect();
            if !repeat.is_empty() {
                warnings.push(Warning {
                    code: "same_recipient".into(),
                    text: format!(
                        "You already wrote to these recipients in the last {days} days: {}.",
                        repeat.join(", ")
                    ),
                    addresses: repeat,
                    count: 0,
                    limit: days,
                });
            }
        }

        if !facts.opt_out_line {
            legal = true;
            warnings.push(Warning {
                code: "no_opt_out_line".into(),
                text: "This mail would go out without the opt-out line.".into(),
                addresses: Vec::new(),
                count: 0,
                limit: 0,
            });
        }

        GuardReport {
            warnings,
            legal_notice: legal.then(|| LEGAL_NOTICE.to_owned()),
            footer: facts
                .opt_out_line
                .then(|| st.settings.footer_for(facts.from)),
        }
    }

    /// Whether the opt-out line is on by default (owner setting).
    #[must_use]
    pub fn opt_out_line_default(&self) -> bool {
        lock(&self.state).settings.opt_out_line
    }

    /// Remember that the owner is being shown exactly these warnings for the mail with `key`.
    pub fn freeze(&self, key: &str, report: &GuardReport, token: &str) {
        let mut st = lock(&self.state);
        st.frozen.retain(|_, f| f.at.elapsed() < FREEZE_TTL);
        if report.warnings.is_empty() {
            st.frozen.remove(key);
            return;
        }
        st.frozen.insert(
            key.to_owned(),
            Frozen {
                codes: report.codes(),
                token: token.to_owned(),
                at: Instant::now(),
            },
        );
    }

    /// Drop what [`Self::freeze`] kept for `token` (the approval ended without a send).
    pub fn discard(&self, token: &str) {
        lock(&self.state).frozen.retain(|_, f| f.token != token);
    }

    /// One-shot: `true` when the owner was shown every warning in `report` for this very mail.
    /// The entry is consumed either way a match is found.
    pub fn take_override(&self, key: &str, report: &GuardReport) -> bool {
        let mut st = lock(&self.state);
        let Some(f) = st.frozen.remove(key) else {
            return false;
        };
        let current = report.codes();
        f.at.elapsed() < FREEZE_TTL && current.iter().all(|c| f.codes.contains(c))
    }

    /// Append a sent mail to the log (memory first, then the file).
    pub fn record(&self, mut entry: SendLogEntry) {
        entry.subject = entry.subject.chars().take(SUBJECT_KEEP).collect();
        entry.at_utc = utc_text(entry.at);
        let line = serde_json::to_string(&entry).unwrap_or_default();
        lock(&self.state).log.push(entry);
        if let Some(dir) = &self.dir {
            let res = std::fs::create_dir_all(dir).and_then(|()| {
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("send-log.jsonl"))?;
                writeln!(f, "{line}")
            });
            if let Err(e) = res {
                log::warn!("email: could not write the send log: {e}");
            }
        }
    }
}

/// Append the opt-out line to `body` (a separator the line cannot be confused with the text of).
#[must_use]
pub fn with_footer(body: &str, footer: &str) -> String {
    format!("{}\n\n-- \n{footer}\n", body.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(rcpt: &[String], line: bool) -> MailFacts<'_> {
        MailFacts {
            bot: Some("b1"),
            account: "work",
            from: "me@example.org",
            recipients: rcpt,
            opt_out_line: line,
        }
    }

    fn entry(rcpt: &[&str], at: u64) -> SendLogEntry {
        SendLogEntry {
            at,
            at_utc: String::new(),
            bot: None,
            run: "r".into(),
            account: "work".into(),
            from: "me@example.org".into(),
            recipients: rcpt.iter().map(|s| (*s).to_owned()).collect(),
            subject: "=cmd".into(),
            message_id: "m1".into(),
            opt_out_line: true,
            overridden: vec![],
        }
    }

    #[test]
    fn utc_text_is_right() {
        assert_eq!(utc_text(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_text(1_795_000_000), "2026-11-18T11:06:40Z");
    }

    #[test]
    fn opt_out_hits_address_and_domain_and_warns_with_legal_note() {
        let g = MailGuard::in_memory();
        g.add_opt_out("Ann@Example.com", "").unwrap();
        g.add_opt_out("@blocked.org", "").unwrap();
        let r = vec!["ann@example.com".to_owned(), "x@blocked.org".to_owned()];
        let rep = g.check(&facts(&r, true));
        assert_eq!(rep.codes(), vec!["opt_out_listed"]);
        assert_eq!(rep.warnings[0].addresses.len(), 2);
        assert!(rep.legal_notice.is_some());
        let ok = vec!["bob@example.com".to_owned()];
        assert_eq!(g.check(&facts(&ok, true)).codes(), Vec::<String>::new());
    }

    #[test]
    fn daily_limit_and_same_recipient_are_warnings_not_blocks() {
        let g = MailGuard::in_memory();
        let s = GuardSettings {
            daily_limit: 2,
            ..GuardSettings::default()
        };
        g.set_settings(s).unwrap();
        g.record(entry(&["a@x.io", "b@x.io"], now_secs()));
        let r = vec!["a@x.io".to_owned()];
        let rep = g.check(&facts(&r, true));
        assert_eq!(rep.codes(), vec!["daily_limit", "same_recipient"]);
        assert!(rep.legal_notice.is_none());
    }

    #[test]
    fn override_needs_a_matching_frozen_card_and_is_one_shot() {
        let g = MailGuard::in_memory();
        g.add_opt_out("a@x.io", "").unwrap();
        let r = vec!["a@x.io".to_owned()];
        let rep = g.check(&facts(&r, true));
        assert!(
            !g.take_override("k", &rep),
            "nothing was shown to the owner"
        );
        g.freeze("k", &rep, "t1");
        assert!(g.take_override("k", &rep));
        assert!(!g.take_override("k", &rep), "consumed");
        g.freeze("k", &rep, "t2");
        g.discard("t2");
        assert!(!g.take_override("k", &rep), "approval ended");
        // A new warning that the owner was not shown does not ride on an old card.
        let none = g.check(&facts(&["z@x.io".to_owned()], true));
        g.freeze("k", &none, "t3");
        assert!(!g.take_override("k", &rep));
    }

    #[test]
    fn export_neutralises_formulas_and_files_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let g = MailGuard::open(dir.path().to_path_buf());
        g.record(entry(&["a@x.io"], 100));
        g.add_opt_out("b@x.io", "asked").unwrap();
        assert!(g.export_csv().contains("\"'=cmd\""));
        let again = MailGuard::open(dir.path().to_path_buf());
        assert_eq!(again.log(10).len(), 1);
        assert_eq!(again.opt_outs().len(), 1);
        assert!(again.remove_opt_out("b@x.io").unwrap());
    }
}
