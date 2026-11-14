//! Use the loop's redacted event inputs, never raw persisted call arguments.
use std::collections::HashMap;

use protocol::{Event, RunId};
use secret_store::{REDACTED, Redactor};
use serde_json::Value;
use storage::RunStep;
use uuid::Uuid;
use workflow_replay::{Param, ParamType, Recording, Workflow};

use super::{ApiError, AppState, ParameterOrigin, SkippedCall, StepProvenance};

pub(super) struct Secret {
    token: String,
    name: String,
    kind: ParamType,
    original: String,
}

pub(super) struct ReviewSteps {
    pub steps: Vec<RunStep>,
    pub skipped: Vec<SkippedCall>,
    pub secrets: Vec<Secret>,
    pub provenance: Vec<StepProvenance>,
    pub untrusted_content: bool,
}

fn map_strings(value: &mut Value, f: &impl Fn(&str) -> String) {
    match value {
        Value::String(s) => *s = f(s),
        Value::Array(items) => items.iter_mut().for_each(|v| map_strings(v, f)),
        Value::Object(fields) => fields.values_mut().for_each(|v| map_strings(v, f)),
        _ => {}
    }
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "password",
        "passwd",
        "pwd",
        "pass",
        "secret",
        "token",
        "api_key",
        "apikey",
        "authorization",
        "credential",
        "cookie",
        "bearer",
        "otp",
        "private_key",
        "header",
    ]
    .iter()
    .any(|s| key.contains(s))
}

// Structural references are preserved only for tools whose recorder needs
// them. Every other string field is ambiguous data and requires fresh input.
fn protect(input: &mut Value, tool: &str, secrets: &mut Vec<Secret>) {
    protect_field(input, tool, secrets, false);
}

fn protect_field(input: &mut Value, tool: &str, secrets: &mut Vec<Secret>, typed_secret: bool) {
    match input {
        Value::Object(fields) => {
            for (key, value) in fields {
                let structural = matches!(
                    tool,
                    "browser_click"
                        | "browser_fill"
                        | "browser_select"
                        | "browser_press"
                        | "desktop_click"
                        | "desktop_type"
                        | "desktop_set_value"
                        | "desktop_key"
                ) && matches!(
                    key.as_str(),
                    "ref" | "index" | "button" | "key" | "app" | "window_title"
                );
                if !structural || sensitive_key(key) {
                    protect_field(value, tool, secrets, typed_secret || sensitive_key(key));
                }
            }
        }
        Value::Array(items) => {
            for value in items {
                protect_field(value, tool, secrets, typed_secret);
            }
        }
        Value::String(_) | Value::Number(_) | Value::Bool(_)
            if input.is_string() || typed_secret =>
        {
            let original = input
                .as_str()
                .map_or_else(|| input.to_string(), str::to_owned);
            let kind = match input {
                Value::Number(_) => ParamType::Number,
                Value::Bool(_) => ParamType::Bool,
                _ => ParamType::String,
            };
            let name = format!("recorded_secret_{}", secrets.len() + 1);
            // Unique field placeholders are replaced only at whole fields.
            let token = format!("WFSECRET{}", Uuid::now_v7().simple());
            *input = Value::String(token.clone());
            secrets.push(Secret {
                token,
                name,
                kind,
                original,
            });
        }
        _ => {}
    }
}

fn skipped_reason(
    name: &str,
    spec: Option<&tool_registry::ToolSpec>,
    result: Option<&Value>,
) -> Option<&'static str> {
    if result.is_some_and(|row| row["is_error"] == true) {
        return Some("call failed");
    }
    if spec.is_some_and(|spec| {
        spec.effect == tool_registry::Effect::Read
            || spec.category == protocol::PermissionCategory::Meta
    }) || matches!(
        name,
        "fs_read"
            | "browser_snapshot"
            | "desktop_get_state"
            | "fs_list"
            | "fs_search"
            | "todo_write"
            | "ask_user"
    ) {
        Some("read-only or meta call")
    } else {
        None
    }
}

pub(super) fn review_steps(s: &AppState, id: RunId) -> Result<ReviewSteps, ApiError> {
    let log = storage::read_event_log(s.storage().data_dir(), id).map_err(ApiError::internal)?;
    let mut inputs = HashMap::new();
    for envelope in log {
        if let Event::ToolCallRequested(call) = envelope.event {
            inputs.insert(call.call_id.0.to_string(), (call.tool_name, call.input));
        }
    }
    let mut steps = s.storage().list_steps(id)?;
    let user_text = steps
        .iter()
        .filter(|step| step.name == "user_message")
        .filter_map(|step| step.input["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut skipped = Vec::new();
    let mut secrets = Vec::new();
    let mut provenance = Vec::new();
    let mut untrusted_content = false;
    let run = s.run_of_user(id)?;
    let registry = s.agent().workflow_recording_tools(run.bot_id)?;
    for step in steps.iter_mut().filter(|step| step.name == "tools") {
        let results = step.output.as_ref().and_then(|o| o["results"].as_array());
        let Some(calls) = step.input["calls"].as_array_mut() else {
            continue;
        };
        for call in calls {
            let tool = call["name"].as_str().unwrap_or_default().to_owned();
            let result = results.and_then(|rows| rows.iter().find(|row| row["id"] == call["id"]));
            let key = result.and_then(|row| row["call_uuid"].as_str());
            let safe = key
                .and_then(|key| inputs.get(key))
                .filter(|(name, _)| name == &tool);
            let Some((_, safe)) = safe else {
                // Legacy runs without redacted inputs cannot safely be proposed.
                return Err(ApiError::conflict(
                    "redacted tool inputs unavailable for this run",
                ));
            };
            let mut safe = safe.clone();
            let name = if tool == "use_tool" {
                safe["name"].as_str().unwrap_or(&tool)
            } else {
                &tool
            }
            .to_owned();
            let tool_handle = registry
                .get(&name)
                .or_else(|| registry.get(&name.replace("__", ".")))
                .ok_or_else(|| ApiError::conflict("source tool is not visible to this bot"))?;
            let spec = tool_handle.spec();
            if let Some(reason) = skipped_reason(&name, Some(spec), result) {
                skipped.push(SkippedCall {
                    tool: name.clone(),
                    reason: reason.into(),
                });
            }
            let non_meta = spec.category != protocol::PermissionCategory::Meta;
            let read = spec.effect == tool_registry::Effect::Read && non_meta;
            let data = if tool == "use_tool" {
                &mut safe["input"]
            } else {
                &mut safe
            };
            if read {
                untrusted_content = true;
            }
            if !read && non_meta && !result.is_some_and(|r| r["is_error"] == true) {
                provenance.push(step_origin(
                    data,
                    untrusted_content,
                    &user_text,
                    provenance.len(),
                ));
            }
            // Any non-meta tool result can contain external/private content,
            // including MCP write responses and browser navigation output.
            if non_meta
                && result.is_some_and(|r| r["text"].as_str().is_some_and(|text| !text.is_empty()))
            {
                untrusted_content = true;
            }
            protect(data, &name, &mut secrets);
            call["arguments"] =
                Value::String(serde_json::to_string(&safe).map_err(ApiError::internal)?);
        }
    }
    // The recorder must not lift defaults from chat text or derive a
    // description containing a typed value. Tool arguments were replaced at
    // their exact field occurrences, never across unrelated steps.
    for step in &mut steps {
        if step.name == "user_message" {
            step.input["text"] = Value::String(String::new());
        }
    }
    Ok(ReviewSteps {
        steps,
        skipped,
        secrets,
        provenance,
        untrusted_content,
    })
}

fn step_origin(data: &Value, tainted: bool, user_text: &str, index: usize) -> StepProvenance {
    let mut parameters = Vec::new();
    collect_origins(data, "", &mut parameters, tainted, user_text);
    StepProvenance {
        step_index: index,
        untrusted_read_before: tainted,
        origin: if !tainted && parameters.iter().all(|p| p.origin == "user_request") {
            "user_request"
        } else {
            "run_content_or_model"
        }
        .into(),
        needs_confirmation: tainted || parameters.iter().any(|p| p.origin != "user_request"),
        parameters,
    }
}

fn collect_origins(
    value: &Value,
    pointer: &str,
    out: &mut Vec<ParameterOrigin>,
    tainted: bool,
    user_text: &str,
) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                collect_origins(
                    value,
                    &format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1")),
                    out,
                    tainted,
                    user_text,
                );
            }
        }
        Value::Array(items) => {
            for (i, value) in items.iter().enumerate() {
                collect_origins(value, &format!("{pointer}/{i}"), out, tainted, user_text);
            }
        }
        _ => out.push(ParameterOrigin {
            pointer: pointer.into(),
            origin: if !tainted
                && value
                    .as_str()
                    .is_some_and(|text| !text.is_empty() && user_text.contains(text))
            {
                "user_request"
            } else {
                "run_content_or_model"
            }
            .into(),
        }),
    }
}

pub(super) fn finish(
    recording: &mut Recording,
    secrets: &[Secret],
) -> Result<Vec<String>, ApiError> {
    let mut value = serde_json::to_value(&recording.workflow).map_err(ApiError::internal)?;
    let mut required = Vec::new();
    let mut params = Vec::new();
    for secret in secrets {
        // A dropped call may have produced no step, so it needs no parameter.
        if !value.to_string().contains(&secret.token) {
            continue;
        }
        map_strings(&mut value, &|text| {
            if text == secret.token {
                format!("{{{}}}", secret.name)
            } else {
                text.to_owned()
            }
        });
        required.push(secret.name.clone());
        params.push(Param {
            name: secret.name.clone(),
            ty: secret.kind,
            default: None,
            description:
                "Fresh input required; recorded values may contain credentials or destinations"
                    .into(),
        });
    }
    let structural_url_credentials = has_url_credentials(&value);
    let structural_secret_echo = contains_protected_literal(&value, secrets);
    recording.workflow = serde_json::from_value(value).map_err(ApiError::internal)?;
    recording.workflow.params.extend(params);
    // Snapshot-derived locators can contain credentials too. Refuse rather
    // than storing a redacted literal that replay would later send.
    let encoded = serde_json::to_string(&recording.workflow).map_err(ApiError::internal)?;
    if encoded.contains(REDACTED)
        || structural_url_credentials
        || Redactor::new().contains_secret(&encoded)
        || structural_secret_echo
    {
        return Err(ApiError::conflict(
            "credential in structural recording data; cannot safely record",
        ));
    }
    redact_metadata(&mut recording.workflow)?;
    if !required.is_empty() {
        recording.warnings.push("Ambiguous data, typed fields and destinations require fresh parameters; no recorded default is retained".into());
    }
    recording
        .workflow
        .validate()
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    Ok(required)
}

fn contains_protected_literal(value: &Value, secrets: &[Secret]) -> bool {
    match value {
        Value::String(text) => secrets.iter().any(|secret| {
            text == &secret.original
                || (secret.original.len() >= 6 && text.contains(&secret.original))
        }),
        Value::Array(values) => values
            .iter()
            .any(|value| contains_protected_literal(value, secrets)),
        Value::Object(fields) => fields
            .values()
            .any(|value| contains_protected_literal(value, secrets)),
        _ => false,
    }
}

// Snapshot-derived locator strings are structural, not replay payloads. If
// they contain URL credentials, refuse the recording rather than retain them.
fn has_url_credentials(value: &Value) -> bool {
    match value {
        Value::String(text) => text.split_once("://").is_some_and(|(_, rest)| {
            rest.contains('?')
                || rest
                    .split(['/', '?', '#', ' ', '\n'])
                    .next()
                    .is_some_and(|host| host.contains('@'))
        }),
        Value::Array(values) => values.iter().any(has_url_credentials),
        Value::Object(fields) => fields.values().any(has_url_credentials),
        _ => false,
    }
}

pub(super) fn redact_metadata(wf: &mut Workflow) -> Result<(), ApiError> {
    let redactor = Redactor::new();
    let mut value = serde_json::to_value(&*wf).map_err(ApiError::internal)?;
    map_strings(&mut value, &|text| redactor.redact(text));
    *wf = serde_json::from_value(value).map_err(ApiError::internal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ambiguous_credentials_commands_urls_headers_and_typed_values_require_parameters() {
        for (tool, input) in [
            (
                "shell",
                json!({"command":"curl -u user:pw https://example.test"}),
            ),
            (
                "navigate",
                json!({"url":"https://user:pw@example.test/?token=hidden"}),
            ),
            (
                "mcp__send",
                json!({"value":"unrecognised-credential", "headers":{"X-Custom":"hidden"}, "cookie":"session=hidden"}),
            ),
            ("browser_fill", json!({"ref":"e1", "text":"1"})),
            (
                "mutate",
                json!({"pwd":"hidden", "otp":"123", "private_key":"hidden", "value":"[REDACTED]"}),
            ),
        ] {
            let mut safe = input.clone();
            let mut secrets = Vec::new();
            protect(&mut safe, tool, &mut secrets);
            assert!(!secrets.is_empty());
            let mut wf = Workflow::new("safe");
            wf.steps
                .push(workflow_replay::Step::Tool(workflow_replay::ToolStep {
                    tool: tool.into(),
                    input: safe,
                }));
            let mut recording = Recording {
                workflow: wf,
                dropped: vec![],
                warnings: vec![],
            };
            finish(&mut recording, &secrets).unwrap();
            assert!(
                recording
                    .workflow
                    .params
                    .iter()
                    .all(|p| p.default.is_none())
            );
            assert!(!recording.workflow.to_toml().unwrap().contains(REDACTED));
            let saved = recording.workflow.to_toml().unwrap();
            assert!(!saved.contains("curl -u"));
            assert!(!saved.contains("user:pw"));
            assert!(!saved.contains("unrecognised-credential"));
            assert!(!saved.contains("session=hidden"));
        }
    }

    #[test]
    fn quoted_typed_credentials_echoed_in_structural_fields_are_refused() {
        let secret = "fake\"password\nvalue";
        let mut input = serde_json::json!({"text":secret});
        let mut secrets = Vec::new();
        protect(&mut input, "browser_fill", &mut secrets);
        input["snapshot_title"] = serde_json::json!(format!("Title: {secret}"));
        let mut wf = Workflow::new("safe");
        wf.steps
            .push(workflow_replay::Step::Tool(workflow_replay::ToolStep {
                tool: "mutate".into(),
                input,
            }));
        let mut recording = Recording {
            workflow: wf,
            dropped: vec![],
            warnings: vec![],
        };
        assert!(finish(&mut recording, &secrets).is_err());
    }

    #[test]
    fn snapshot_derived_url_credentials_are_refused_instead_of_saved_as_literals() {
        for url in [
            "https://user:pw@example.test/",
            "https://example.test/?cookie=unknown-credential",
        ] {
            let mut wf = Workflow::new("safe");
            wf.steps
                .push(workflow_replay::Step::Navigate(workflow_replay::Navigate {
                    url: url.into(),
                    profile: None,
                }));
            let mut recording = Recording {
                workflow: wf,
                dropped: vec![],
                warnings: vec![],
            };
            assert!(finish(&mut recording, &[]).is_err());
        }
    }

    #[test]
    fn short_typed_value_masks_only_its_field_and_never_other_steps() {
        let mut first = json!({"ref":"e1", "text":"e"});
        let second = json!({"ref":"e1", "button":"left"});
        let mut secrets = Vec::new();
        protect(&mut first, "browser_fill", &mut secrets);
        let mut next = second.clone();
        protect(&mut next, "browser_click", &mut secrets);
        assert_eq!(next, second);
        assert_eq!(first["ref"], "e1");
        assert_ne!(first["text"], "e");
    }
}
