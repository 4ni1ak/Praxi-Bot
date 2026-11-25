//! Deterministic `SKILL.md` draft from the steps of a finished run.
//!
//! The draft is built only from tool names and argument *names*. An argument
//! value becomes a parameter; its value is shown as an example only when it is
//! short, passes the caller's redactor unchanged and does not look like a
//! secret or a personal address. Everything else is withheld, so a canary
//! secret typed into a chat can never be copied into the saved skill. The
//! result is a proposal: the owner edits it, the scanner runs on it and it is
//! installed only after an explicit confirmation (see `daemon-api`).

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::skill::{MAX_DESCRIPTION_LEN, MAX_NAME_LEN, validate_name};

/// Most steps copied into one draft.
pub const MAX_DRAFT_STEPS: usize = 40;
/// Longest example value shown next to a parameter.
const MAX_EXAMPLE_CHARS: usize = 60;
/// Most parameters in one draft.
const MAX_PARAMS: usize = 24;

/// One successful tool call of the run.
#[derive(Debug, Clone)]
pub struct DraftCall {
    /// Tool name.
    pub tool: String,
    /// The raw JSON arguments exactly as the model sent them.
    pub arguments: String,
    /// `read`, `write` or `destructive`; `None` when the tool is not registered.
    pub effect: Option<String>,
}

/// Everything the draft is built from.
#[derive(Debug, Clone)]
pub struct DraftInput {
    /// Chat title (redacted by the builder before use).
    pub title: String,
    /// Successful calls, oldest first.
    pub calls: Vec<DraftCall>,
}

/// A value the owner supplies each time the skill runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DraftParam {
    /// Placeholder name used in the steps as `{{name}}`.
    pub name: String,
    /// A harmless example, or `None` when the real value was withheld.
    pub example: Option<String>,
    /// True when the value looked like a secret or private data.
    pub withheld: bool,
}

/// A tool the skill relies on and what it can do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DraftTool {
    /// Tool name.
    pub name: String,
    /// `read`, `write`, `destructive` or `unknown`.
    pub effect: String,
}

/// The proposal shown in the preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillDraft {
    /// Suggested skill name.
    pub name: String,
    /// One-line description.
    pub description: String,
    /// Trigger keywords.
    pub keywords: Vec<String>,
    /// Tools the skill needs.
    pub tools: Vec<DraftTool>,
    /// Parameters.
    pub parameters: Vec<DraftParam>,
    /// Plain-language steps.
    pub steps: Vec<String>,
    /// How many values were withheld.
    pub withheld: usize,
    /// The complete SKILL.md text.
    pub markdown: String,
}

const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "authorization",
    "cookie",
    "credential",
    "bearer",
    "private",
    "auth",
    "key",
    "pin",
    "otp",
];

fn slug(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
        if out.len() >= max {
            break;
        }
    }
    out.trim_end_matches('-').to_owned()
}

fn param_name(key: &str) -> String {
    let s = slug(key, 32).replace('-', "_");
    if s.is_empty() { "value".into() } else { s }
}

fn secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    SECRET_KEYS.iter().any(|s| {
        if s.len() <= 3 {
            k.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| w == *s)
        } else {
            k.contains(s)
        }
    })
}

/// A value is safe to show only when nothing about it could be private.
fn safe_example(value: &str, redact: &dyn Fn(&str) -> String) -> bool {
    if value.is_empty()
        || value.chars().count() > MAX_EXAMPLE_CHARS
        || value.chars().any(char::is_control)
        || value.contains('@')
        || value.contains('\\')
        || redact(value) != value
    {
        return false;
    }
    // A long unbroken run of token characters is treated as a credential.
    let mut run = 0usize;
    for c in value.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '=') {
            run += 1;
            if run >= 20 {
                return false;
            }
        } else {
            run = 0;
        }
    }
    // Query strings and credentials inside URLs frequently carry tokens.
    (!value.contains('?') && !value.contains("://")) || public_url(value)
}

fn public_url(value: &str) -> bool {
    value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .is_some_and(|rest| !rest.contains(['?', '#', '@', ' ']))
}

fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Build the draft. `redact` is the host's secret redactor.
#[must_use]
pub fn build(input: &DraftInput, redact: &dyn Fn(&str) -> String) -> SkillDraft {
    let title = input.title.trim();
    let title_ok = !title.is_empty() && redact(title) == title;
    let title = if title_ok { title } else { "" };

    let mut name = slug(title, MAX_NAME_LEN - 4);
    if name.len() < 3 || validate_name(&name).is_err() {
        name = "saved-skill".into();
    }

    let mut tools: Vec<DraftTool> = Vec::new();
    let mut params: Vec<DraftParam> = Vec::new();
    let mut steps = Vec::new();
    let mut withheld = 0usize;
    for call in input.calls.iter().take(MAX_DRAFT_STEPS) {
        if !tools.iter().any(|t| t.name == call.tool) {
            tools.push(DraftTool {
                name: call.tool.clone(),
                effect: call.effect.clone().unwrap_or_else(|| "unknown".into()),
            });
        }
        let mut used: Vec<String> = Vec::new();
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&call.arguments) {
            for (key, value) in &map {
                if params.len() >= MAX_PARAMS {
                    break;
                }
                let text = scalar_text(value);
                let secret = secret_key(key);
                let show = !secret && text.as_deref().is_some_and(|t| safe_example(t, redact));
                let mut pname = param_name(key);
                // Same name seen again with a different value: a new parameter.
                let mut n = 1;
                let new_example = if show { text.clone() } else { None };
                while params
                    .iter()
                    .any(|p| p.name == pname && p.example != new_example)
                {
                    n += 1;
                    pname = format!("{}_{n}", param_name(key));
                }
                if !params.iter().any(|p| p.name == pname) {
                    if !show {
                        withheld += 1;
                    }
                    params.push(DraftParam {
                        name: pname.clone(),
                        example: new_example,
                        withheld: !show,
                    });
                }
                used.push(pname);
            }
        }
        let with = if used.is_empty() {
            String::new()
        } else {
            let list: Vec<String> = used.iter().map(|p| format!("{{{{{p}}}}}")).collect();
            format!(" with {}", list.join(", "))
        };
        steps.push(format!("Use the `{}` tool{with}.", call.tool));
    }

    let mut keywords: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    for w in title.split(|c: char| !c.is_alphabetic()) {
        let w = w.to_lowercase();
        if w.chars().count() >= 4 && seen.insert(w.clone()) && keywords.len() < 5 {
            keywords.push(w);
        }
    }

    let description = if title.is_empty() {
        "Repeats a task saved from a chat.".to_owned()
    } else {
        let d = format!(
            "Repeats the task from the chat \"{}\".",
            title.replace('"', "")
        );
        d.chars().take(MAX_DESCRIPTION_LEN).collect()
    };
    let description: String = description.chars().filter(|c| !c.is_control()).collect();

    let markdown = render(&name, &description, &keywords, &tools, &params, &steps);
    SkillDraft {
        name,
        description,
        keywords,
        tools,
        parameters: params,
        steps,
        withheld,
        markdown,
    }
}

fn render(
    name: &str,
    description: &str,
    keywords: &[String],
    tools: &[DraftTool],
    params: &[DraftParam],
    steps: &[String],
) -> String {
    use std::fmt::Write as _;
    let desc = serde_json::to_string(description).unwrap_or_else(|_| "\"\"".into());
    let mut s = format!("---\nname: {name}\ndescription: {desc}\n");
    if !keywords.is_empty() {
        let _ = writeln!(s, "when:\n  keywords: [{}]", keywords.join(", "));
    }
    s.push_str("---\n# What this does\nRepeat the task below. Ask the user for any value you do not have.\n\n");
    s.push_str("## Tools this skill needs\n");
    for t in tools {
        let _ = writeln!(s, "- `{}` ({})", t.name, t.effect);
    }
    if !params.is_empty() {
        s.push_str("\n## Parameters\n");
        for p in params {
            match (&p.example, p.withheld) {
                (Some(e), _) => {
                    let _ = writeln!(s, "- `{{{{{}}}}}`: example `{e}`", p.name);
                }
                (None, _) => {
                    let _ = writeln!(
                        s,
                        "- `{{{{{}}}}}`: ask the user each time (the value was not saved)",
                        p.name
                    );
                }
            }
        }
    }
    s.push_str("\n## Steps\n");
    for (i, step) in steps.iter().enumerate() {
        let _ = writeln!(s, "{}. {step}", i + 1);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Source, parse_skill};

    const CANARY: &str = "sk-live-CANARY9f3a77c2d1e84b6a90";

    fn call(tool: &str, args: &str) -> DraftCall {
        DraftCall {
            tool: tool.into(),
            arguments: args.into(),
            effect: Some("read".into()),
        }
    }

    fn redactor(text: &str) -> String {
        text.replace("hunter2-private", "[REDACTED]")
    }

    #[test]
    fn secrets_and_private_values_are_never_copied() {
        let args = serde_json::json!({
            "url": "https://example.com/docs",
            "api_key": CANARY,
            "note": "call me at me@home.example",
            "path": r"C:\Users\emre\secret.txt",
            "pw": "hunter2-private",
            "q": CANARY,
        })
        .to_string();
        let d = build(
            &DraftInput {
                title: "Check the docs".into(),
                calls: vec![call("web_fetch", &args)],
            },
            &redactor,
        );
        for forbidden in [CANARY, "me@home.example", "secret.txt", "hunter2-private"] {
            assert!(!d.markdown.contains(forbidden), "{forbidden} leaked");
        }
        assert!(
            d.markdown.contains("https://example.com/docs"),
            "{}",
            d.markdown
        );
        assert!(d.withheld >= 4);
        assert!(d.markdown.contains("{{api_key}}"));
    }

    #[test]
    fn a_secret_in_the_title_falls_back_to_a_generic_name() {
        let d = build(
            &DraftInput {
                title: "hunter2-private notes".into(),
                calls: vec![call("web_fetch", "{}")],
            },
            &redactor,
        );
        assert_eq!(d.name, "saved-skill");
        assert!(!d.markdown.contains("hunter2"));
    }

    #[test]
    fn the_draft_is_a_valid_skill_with_keywords_tools_and_steps() {
        let d = build(
            &DraftInput {
                title: "Weekly report from Notion".into(),
                calls: vec![
                    call("web_fetch", r#"{"url":"https://notion.so/team"}"#),
                    DraftCall {
                        tool: "fs_write".into(),
                        arguments: r#"{"path":"report.md"}"#.into(),
                        effect: Some("write".into()),
                    },
                ],
            },
            &redactor,
        );
        let skill = parse_skill(&d.markdown, Source::User, None).unwrap();
        assert_eq!(skill.name, "weekly-report-from-notion");
        assert!(skill.when.keywords.contains(&"report".to_owned()));
        assert_eq!(d.tools.len(), 2);
        assert!(d.markdown.contains("`fs_write` (write)"));
        assert_eq!(d.steps.len(), 2);
    }
}
