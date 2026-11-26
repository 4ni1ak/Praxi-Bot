//! MCP elicitation: an MCP server asks the owner for input while one of its
//! tool calls runs (the bridge finds the run, see `mcp-bridge`'s `client.rs`).
//!
//! The run parks like a `secret_request`: a form card (`ElicitationRequested`)
//! and a wait bounded by the approval timeout. Only a run the owner started at
//! the desktop may ask; any other run declines at once. The owner's answer is
//! checked against the form here and goes back to the server only: it is never
//! put into a tool output, an event, the journal, the audit log or a log line.
//! The model sees nothing but the server's own tool result.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use protocol::event::{ElicitationField, ElicitationFieldType, ElicitationRequested};
use protocol::{Event, InitiatorKind, RunId, ToolCallId};
use serde_json::{Map, Value, json};
use tool_registry::{ElicitationAnswer, ElicitationRequest, Elicitor};

use crate::approval::Waited;
use crate::history::truncate_utf8;
use crate::shared::{PendingKind, RunCtx, Shared, Verdict};

/// A parked form: what an answer is checked against.
#[derive(Debug, Clone)]
pub(crate) struct ElicitationInfo {
    pub fields: Vec<ElicitationField>,
}

/// Why [`crate::AgentLoop::finish_elicitation`] did not take an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElicitationRefusal {
    /// No form waits under this id (answered, expired, stopped).
    Gone,
    /// The answer does not fit the form; the form keeps waiting. The message
    /// names the field, never a value.
    Invalid(String),
}

impl Shared {
    /// Show an MCP server's form to the owner of `ctx` and wait for the answer.
    pub(crate) async fn request_elicitation(
        self: &Arc<Self>,
        ctx: &RunCtx,
        request: ElicitationRequest,
    ) -> ElicitationAnswer {
        let server = request.server;
        if ctx.initiator != InitiatorKind::Human {
            self.audit(
                ctx,
                "system",
                "elicitation.refused",
                &json!({"server": server, "reason": "not a run the owner started at the desktop"}),
            );
            return ElicitationAnswer::Decline;
        }
        let call_id = ToolCallId::new();
        let summary = format!("mcp form: {}", truncate_utf8(&server, 80));
        let (approval_id, mut guard, rx) = self.park(
            ctx,
            call_id,
            &summary,
            true,
            false,
            PendingKind::Elicitation,
        );
        self.elicitations.lock().insert(
            call_id,
            ElicitationInfo {
                fields: request.fields.clone(),
            },
        );
        let names: Vec<&str> = request.fields.iter().map(|f| f.name.as_str()).collect();
        self.audit(
            ctx,
            "system",
            "elicitation.requested",
            &json!({"server": server, "fields": names}),
        );
        self.emit(
            ctx,
            Event::ElicitationRequested(ElicitationRequested {
                run_id: ctx.run_id,
                call_id,
                server: server.clone(),
                message: self.redact(request.message),
                fields: request.fields,
                expires_in_secs: self.cfg.approval_timeout.as_secs(),
            }),
        );
        let waited = self.wait_verdict(ctx, rx, false).await;
        guard.resume();
        self.elicitations.lock().remove(&call_id);
        let (answer, outcome) = match waited {
            Waited::Verdict(Ok(Verdict::Elicited(answer))) => {
                let outcome = match &answer {
                    ElicitationAnswer::Accept(_) => "answered",
                    ElicitationAnswer::Decline => "declined",
                    ElicitationAnswer::Cancel => "cancelled",
                };
                (answer, outcome)
            }
            Waited::Paused | Waited::Parked | Waited::Expired => {
                (ElicitationAnswer::Cancel, "expired")
            }
            // Stop, a deny through another route, or a dropped card.
            Waited::Verdict(_) => (ElicitationAnswer::Cancel, "cancelled"),
        };
        let _ = self.storage.decide_approval(approval_id, outcome);
        self.audit(
            ctx,
            "user",
            "elicitation.finished",
            &json!({"server": server, "outcome": outcome}),
        );
        answer
    }
}

/// The [`Elicitor`] the daemon installs into the MCP bridge.
pub(crate) struct LoopElicitor {
    pub sh: Weak<Shared>,
}

#[async_trait]
impl Elicitor for LoopElicitor {
    async fn elicit(&self, run_id: &str, request: ElicitationRequest) -> ElicitationAnswer {
        let Some(sh) = self.sh.upgrade() else {
            return ElicitationAnswer::Decline;
        };
        let Ok(id) = run_id.parse::<uuid::Uuid>() else {
            return ElicitationAnswer::Decline;
        };
        let Some(ctx) = sh.runs.lock().get(&RunId(id)).cloned() else {
            return ElicitationAnswer::Decline;
        };
        sh.request_elicitation(&ctx, request).await
    }
}

/// Check an answer against its form: only the form's fields, every required one,
/// each of the right type, inside its bounds and choices. Empty (`null`)
/// optional fields are dropped. An error names the field, never the value.
pub(crate) fn validate(
    fields: &[ElicitationField],
    content: Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    if content.keys().any(|k| !fields.iter().any(|f| &f.name == k)) {
        return Err("the answer has a field this form does not ask for".into());
    }
    for f in fields {
        match content.get(&f.name) {
            None | Some(Value::Null) if f.required => {
                return Err(format!("`{}` is required", f.name));
            }
            None | Some(Value::Null) => {}
            Some(v) => check(f, v)?,
        }
    }
    Ok(content.into_iter().filter(|(_, v)| !v.is_null()).collect())
}

fn check(f: &ElicitationField, v: &Value) -> Result<(), String> {
    let name = &f.name;
    let bounded = |n: f64| {
        if f.minimum.is_some_and(|min| n < min) || f.maximum.is_some_and(|max| n > max) {
            Err(format!("`{name}` is out of range"))
        } else {
            Ok(())
        }
    };
    match f.kind {
        ElicitationFieldType::String => {
            let s = v.as_str().ok_or_else(|| format!("`{name}` must be text"))?;
            if !f.options.is_empty() && !f.options.iter().any(|o| o.value == s) {
                return Err(format!("`{name}` must be one of the offered choices"));
            }
            let len = u32::try_from(s.chars().count()).unwrap_or(u32::MAX);
            if f.min_length.is_some_and(|m| len < m) || f.max_length.is_some_and(|m| len > m) {
                return Err(format!("`{name}` has the wrong length"));
            }
            match f.format.as_deref() {
                Some(format) if !format_ok(format, s) => {
                    Err(format!("`{name}` is not a valid {format}"))
                }
                _ => Ok(()),
            }
        }
        ElicitationFieldType::Number => bounded(
            v.as_f64()
                .ok_or_else(|| format!("`{name}` must be a number"))?,
        ),
        ElicitationFieldType::Integer => {
            if !(v.is_i64() || v.is_u64()) {
                return Err(format!("`{name}` must be a whole number"));
            }
            bounded(v.as_f64().unwrap_or_default())
        }
        ElicitationFieldType::Boolean if v.is_boolean() => Ok(()),
        ElicitationFieldType::Boolean => Err(format!("`{name}` must be yes or no")),
    }
}

fn format_ok(format: &str, s: &str) -> bool {
    match format {
        "email" => s.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && !domain.contains('@')
                && !s.chars().any(char::is_whitespace)
        }),
        "uri" => url::Url::parse(s).is_ok(),
        "date" => chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok(),
        "date-time" => chrono::DateTime::parse_from_rfc3339(s).is_ok(),
        // The protocol allows only the four above; an unknown one is not checked.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::event::ElicitationOption;

    fn field(name: &str, kind: ElicitationFieldType, required: bool) -> ElicitationField {
        ElicitationField {
            name: name.into(),
            title: None,
            description: None,
            kind,
            required,
            options: Vec::new(),
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            format: None,
            default: None,
        }
    }

    fn form() -> Vec<ElicitationField> {
        let mut email = field("email", ElicitationFieldType::String, true);
        email.format = Some("email".into());
        let mut age = field("age", ElicitationFieldType::Integer, false);
        age.minimum = Some(0.0);
        age.maximum = Some(150.0);
        let mut size = field("size", ElicitationFieldType::String, false);
        size.options = vec![ElicitationOption {
            value: "s".into(),
            label: "Small".into(),
        }];
        let mut note = field("note", ElicitationFieldType::String, false);
        note.max_length = Some(3);
        vec![
            email,
            age,
            size,
            note,
            field("ratio", ElicitationFieldType::Number, false),
            field("ok", ElicitationFieldType::Boolean, false),
        ]
    }

    fn answer(v: &Value) -> Result<Map<String, Value>, String> {
        validate(&form(), v.as_object().unwrap().clone())
    }

    #[test]
    fn a_fitting_answer_is_taken_and_empty_optional_fields_dropped() {
        let ok = answer(&json!({
            "email": "a@example.com", "age": 30, "size": "s", "note": "abc",
            "ratio": 0.5, "ok": true, "unused": null
        }));
        assert!(ok.is_err(), "unknown keys are refused even when null");
        let ok = answer(&json!({"email": "a@example.com", "age": null, "ok": false})).unwrap();
        assert_eq!(ok.len(), 2);
        assert!(!ok.contains_key("age"));
    }

    #[test]
    fn answers_that_do_not_fit_are_refused_without_echoing_the_value() {
        let secret = "s3cret-value";
        for bad in [
            json!({}),
            json!({"email": null}),
            json!({"email": secret}),
            json!({"email": "a@example.com", "age": 151}),
            json!({"email": "a@example.com", "age": 1.5}),
            json!({"email": "a@example.com", "age": "3"}),
            json!({"email": "a@example.com", "size": secret}),
            json!({"email": "a@example.com", "note": secret}),
            json!({"email": "a@example.com", "ratio": "x"}),
            json!({"email": "a@example.com", "ok": "yes"}),
            json!({"email": "a@example.com", secret: 1}),
        ] {
            let err = answer(&bad).expect_err(&bad.to_string());
            assert!(!err.contains(secret), "{err}");
        }
    }

    #[test]
    fn string_formats_are_checked() {
        assert!(format_ok("email", "me@example.org"));
        assert!(!format_ok("email", "me@example"));
        assert!(!format_ok("email", "m e@example.org"));
        assert!(format_ok("uri", "https://example.com/x"));
        assert!(!format_ok("uri", "not a uri"));
        assert!(format_ok("date", "2026-11-26"));
        assert!(!format_ok("date", "2026-13-01"));
        assert!(format_ok("date-time", "2026-11-26T10:00:00Z"));
        assert!(!format_ok("date-time", "2026-11-26"));
    }
}
