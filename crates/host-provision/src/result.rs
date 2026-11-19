//! Structured step results, the final table, and log redaction.
use std::{fs, io::Write, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
            Self::Skip => "skip",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepResult {
    pub id: &'static str,
    pub title: String,
    pub status: Status,
    pub detail: String,
    pub remediation: Option<String>,
}

impl StepResult {
    pub fn new(
        id: &'static str,
        title: &str,
        status: Status,
        detail: String,
        remediation: Option<String>,
    ) -> Self {
        Self {
            id,
            title: title.to_owned(),
            status,
            detail,
            remediation,
        }
    }

    pub fn pass(id: &'static str, title: &str, detail: impl Into<String>) -> Self {
        Self::new(id, title, Status::Pass, detail.into(), None)
    }

    pub fn skip(id: &'static str, title: &str, detail: impl Into<String>) -> Self {
        Self::new(id, title, Status::Skip, detail.into(), None)
    }

    pub fn warn(id: &'static str, title: &str, detail: impl Into<String>, hint: &str) -> Self {
        Self::new(
            id,
            title,
            Status::Warn,
            detail.into(),
            Some(hint.to_owned()),
        )
    }

    pub fn fail(id: &'static str, title: &str, detail: impl Into<String>, hint: &str) -> Self {
        Self::new(
            id,
            title,
            Status::Fail,
            detail.into(),
            Some(hint.to_owned()),
        )
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub results: Vec<StepResult>,
    /// A host-key mismatch: nothing further may run, no override exists.
    pub hard_stop: bool,
}

impl Report {
    pub fn ok(&self) -> bool {
        !self.results.iter().any(|r| r.status == Status::Fail)
    }

    /// Plain-text table for the terminal.
    pub fn table(&self) -> String {
        let w = self
            .results
            .iter()
            .map(|r| r.title.chars().count())
            .max()
            .unwrap_or(0);
        let mut s = String::new();
        for r in &self.results {
            s.push_str(&format!(
                "{:<5} {:<w$}  {}\n",
                r.status.label(),
                r.title,
                r.detail
            ));
            if let (Status::Fail | Status::Warn, Some(h)) = (r.status, &r.remediation) {
                s.push_str(&format!("      -> {h}\n"));
            }
        }
        s
    }
}

/// Remove secrets and terminal control sequences from text before it is
/// logged or shown: ticket/pairing material, anything after a secret-looking
/// keyword, private key blocks, long opaque tokens, ANSI escapes.
pub fn redact(text: &str) -> String {
    const KEYWORDS: [&str; 7] = [
        "passphrase",
        "password",
        "token",
        "secret",
        "authorization",
        "ticket",
        "private key",
    ];
    let mut out = Vec::new();
    let mut in_key = false;
    for raw in text.lines() {
        let line: String = strip_ansi(raw);
        if line.contains("-----BEGIN") {
            in_key = true;
        }
        if in_key {
            if line.contains("-----END") {
                in_key = false;
                out.push("<private key block redacted>".to_owned());
            }
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let hit = KEYWORDS
            .iter()
            .filter_map(|k| lower.find(k).map(|i| i + k.len()))
            .min();
        let line = match hit {
            Some(end)
                if line.len() > end
                    && !line[end..].trim().is_empty()
                    && lower.contains(['=', ':', '"']) =>
            {
                format!("{} <redacted>", &line[..end])
            }
            _ => line,
        };
        out.push(mask_tokens(&line));
    }
    out.join("\n")
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out
}

/// Replace runs of 40+ token characters (base32/base64/hex ids, tickets).
fn mask_tokens(line: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.chars().count() >= 40 {
            out.push_str("<redacted>");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in line.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-') {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Append-only redacted log (`provision.log`).
pub struct ProvisionLog {
    path: Option<PathBuf>,
}

impl ProvisionLog {
    pub fn new(path: Option<PathBuf>) -> Self {
        if let Some(dir) = path.as_ref().and_then(|p| p.parent()) {
            let _ = fs::create_dir_all(dir);
        }
        Self { path }
    }

    pub fn line(&self, step: &str, text: &str) {
        let Some(path) = &self.path else { return };
        let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        for l in redact(text).lines() {
            let _ = writeln!(f, "{ts} [{step}] {l}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_tickets_keys_and_escapes_are_redacted() {
        let ticket = "a".repeat(64);
        let text = format!(
            "BOT_SECRETS_PASSPHRASE=hunter2\n\u{1b}[31m{{\"ticket\":\"{ticket}\"}}\n-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----\nhello"
        );
        let out = redact(&text);
        for leak in ["hunter2", &ticket, "abc", "\u{1b}"] {
            assert!(!out.contains(leak), "{leak}");
        }
        assert!(out.contains("hello"));
    }
}
