//! Use the loop's redacted event inputs, never raw persisted call arguments.
use std::collections::HashMap;

use protocol::{Event, RunId};
use secret_store::{REDACTED, Redactor};
use serde_json::Value;
use storage::RunStep;
use uuid::Uuid;
use workflow_replay::{Param, ParamType, Recording, Workflow};

use super::{ApiError, AppState, SkippedCall};

pub(super) struct Secret {
    token: String,
    name: String,
}

pub(super) struct ReviewSteps {
    pub steps: Vec<RunStep>,
    pub skipped: Vec<SkippedCall>,
    pub secrets: Vec<Secret>,
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
        "secret",
        "token",
        "api_key",
        "apikey",
        "authorization",
    ]
    .iter()
    .any(|s| key.contains(s))
}

fn protect(input: &mut Value, typed: bool, secrets: &mut Vec<Secret>, literals: &mut Vec<String>) {
    match input {
        Value::Object(fields) => {
            for (key, value) in fields {
                if (sensitive_key(key) || (typed && matches!(key.as_str(), "text" | "value")))
                    && let Some(text) = value.as_str().filter(|s| !s.is_empty())
                {
                    literals.push(text.to_owned());
                    let name = format!("recorded_secret_{}", secrets.len() + 1);
                    let token = format!("WFSECRET{}", Uuid::now_v7().simple());
                    *value = Value::String(token.clone());
                    secrets.push(Secret { token, name });
                } else {
                    protect(value, typed, secrets, literals);
                }
            }
        }
        Value::Array(items) => {
            for value in items {
                protect(value, typed, secrets, literals);
            }
        }
        _ => {}
    }
}

fn unwrapped(input: &mut Value, tool: &str, secrets: &mut Vec<Secret>, literals: &mut Vec<String>) {
    if tool == "use_tool" {
        let name = input["name"].as_str().unwrap_or_default().to_owned();
        protect(&mut input["input"], is_typing(&name), secrets, literals);
    } else {
        protect(input, is_typing(tool), secrets, literals);
    }
}

fn is_typing(tool: &str) -> bool {
    matches!(tool, "browser_fill" | "desktop_type" | "desktop_set_value")
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
    let mut skipped = Vec::new();
    let mut secrets = Vec::new();
    let mut literals = Vec::new();
    let registry = s.agent().tool_registry();
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
            let spec = registry
                .get(&name)
                .or_else(|| registry.get(&name.replace("__", ".")));
            if result.is_some_and(|row| row["is_error"] == true) {
                skipped.push(SkippedCall {
                    tool: name,
                    reason: "call failed".into(),
                });
            } else if spec.as_ref().is_some_and(|t| {
                t.spec().effect == tool_registry::Effect::Read
                    || t.spec().category == protocol::PermissionCategory::Meta
            }) || matches!(
                name.as_str(),
                "fs_read"
                    | "browser_snapshot"
                    | "desktop_get_state"
                    | "fs_list"
                    | "fs_search"
                    | "todo_write"
                    | "ask_user"
            ) {
                skipped.push(SkippedCall {
                    tool: name,
                    reason: "read-only or meta call".into(),
                });
            }
            // UI trees do not always expose password metadata. Conservatively
            // require a fresh value for every typed field, never retain defaults.
            unwrapped(&mut safe, &tool, &mut secrets, &mut literals);
            call["arguments"] =
                Value::String(serde_json::to_string(&safe).map_err(ApiError::internal)?);
        }
    }
    mask_context(&mut steps, &literals, &secrets)?;
    Ok(ReviewSteps {
        steps,
        skipped,
        secrets,
    })
}

fn mask_context(
    steps: &mut [RunStep],
    literals: &[String],
    secrets: &[Secret],
) -> Result<(), ApiError> {
    // User text and tool output can echo a password. They are context data,
    // not templates: erase the literals there before the recorder reads them.
    for step in steps {
        if let Some(calls) = step.input["calls"].as_array_mut() {
            for call in calls {
                if let Some(arguments) = call["arguments"].as_str() {
                    let mut input: Value =
                        serde_json::from_str(arguments).map_err(ApiError::internal)?;
                    let data = if call["name"] == "use_tool" {
                        &mut input["input"]
                    } else {
                        &mut input
                    };
                    map_strings(data, &|text| {
                        if secrets.iter().any(|secret| secret.token == text) {
                            text.to_owned()
                        } else {
                            mask(text, literals)
                        }
                    });
                    call["arguments"] =
                        Value::String(serde_json::to_string(&input).map_err(ApiError::internal)?);
                }
            }
        }
        if step.name == "user_message" {
            map_strings(&mut step.input["text"], &|text| mask(text, literals));
        }
        if let Some(results) = step
            .output
            .as_mut()
            .and_then(|output| output["results"].as_array_mut())
        {
            // Correlation ids are structural, not display text. A short
            // password must not corrupt ids and turn successful calls into drops.
            for result in results {
                map_strings(&mut result["text"], &|text| mask(text, literals));
            }
        }
    }
    Ok(())
}

fn mask(text: &str, literals: &[String]) -> String {
    literals.iter().fold(text.to_owned(), |text, literal| {
        text.replace(literal, REDACTED)
    })
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
            text.replace(&secret.token, &format!("{{{}}}", secret.name))
        });
        required.push(secret.name.clone());
        params.push(Param {
            name: secret.name.clone(),
            ty: ParamType::String,
            default: None,
            description: "Sensitive or typed value; supply again when running".into(),
        });
    }
    recording.workflow = serde_json::from_value(value).map_err(ApiError::internal)?;
    recording.workflow.params.extend(params);
    redact_metadata(&mut recording.workflow)?;
    if !required.is_empty() {
        recording.warnings.push("Typed fields and sensitive values require fresh parameters; no recorded default is retained".into());
    }
    recording
        .workflow
        .validate()
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    Ok(required)
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
    fn typed_and_nested_credentials_are_replaced_even_without_password_metadata() {
        let mut input =
            json!({"text":"p4ss", "nested":{"api_key":"sk-abcdefghijklmnopqrstuvwxyz"}});
        let mut secrets = Vec::new();
        let mut literals = Vec::new();
        protect(&mut input, true, &mut secrets, &mut literals);
        assert!(!input.to_string().contains("p4ss"));
        assert!(!input.to_string().contains("sk-"));
        assert_eq!(mask("p4ss", &literals), REDACTED);
        let mut wf = Workflow::new("password_test");
        wf.steps
            .push(workflow_replay::Step::Tool(workflow_replay::ToolStep {
                tool: "write".into(),
                input,
            }));
        let mut recording = Recording {
            workflow: wf,
            dropped: vec![],
            warnings: vec![],
        };
        let required = finish(&mut recording, &secrets).unwrap();
        assert_eq!(required.len(), 2);
        assert!(
            recording
                .workflow
                .params
                .iter()
                .all(|p| p.default.is_none())
        );
        let toml = recording.workflow.to_toml().unwrap();
        assert!(!toml.contains("p4ss"));
        assert!(!toml.contains("sk-"));
    }
}
