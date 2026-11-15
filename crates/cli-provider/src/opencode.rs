//! Experimental ACP protocol support, deliberately disconnected from runtime.
//! `opencode acp` is verified by installed v2.2.5 help. Native-tool confinement
//! is NOT verified: no process is spawned and registry.runnable remains false.
//! Protocol evidence: `OpenMausBot` docs/opencode-go.md and
//! server/drivers/acp/{core,opencode-go}.ts (Apache-2.0; ideas only, original code).
use serde_json::{Value, json};

/// No prompt ever belongs in argv. `OpenCode` chooses models through ACP, after
/// session/new or session/load has returned its sessionId.
pub const ARGV: &[&str] = &["acp"];

#[must_use]
pub fn session_request(id: u64, cwd: &str, resume: Option<&str>) -> Value {
    match resume {
        Some(session) => {
            json!({"jsonrpc":"2.0","id":id,"method":"session/load","params":{"sessionId":session,"cwd":cwd,"mcpServers":[]}})
        }
        None => {
            json!({"jsonrpc":"2.0","id":id,"method":"session/new","params":{"cwd":cwd,"mcpServers":[]}})
        }
    }
}

#[must_use]
pub fn select_model(id: u64, session: &str, model: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"session/set_config_option","params":{"sessionId":session,"configId":"model","value":model}})
}

#[must_use]
pub fn prompt(id: u64, session: &str, text: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":text}]}})
}

#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Thought,
    Completed,
    Ignore,
}

/// Fail closed on native tool activity and unknown request/update types.
/// History from a different session is never interpreted as current output.
///
/// # Errors
/// Refuses native tool activity, protocol errors and unknown update types.
pub fn decode(value: &Value, session: &str, prompt_id: u64) -> Result<Frame, &'static str> {
    if value.get("error").is_some() {
        return Err("protocol_error");
    }
    if value.get("id").is_some() && value.get("method").is_some() {
        return Err("native_request");
    }
    if value["id"].as_u64() == Some(prompt_id) {
        return if value["result"]["stopReason"] == "end_turn" {
            Ok(Frame::Completed)
        } else {
            Err("incomplete_turn")
        };
    }
    if value["method"] != "session/update" {
        return Ok(Frame::Ignore);
    }
    let params = &value["params"];
    if params["sessionId"] != session {
        return Ok(Frame::Ignore);
    }
    let update = &params["update"];
    match update["sessionUpdate"].as_str() {
        Some("agent_message_chunk") if update["content"]["type"] == "text" => {
            update["content"]["text"]
                .as_str()
                .map(|t| Frame::Text(t.to_owned()))
                .ok_or("invalid_text")
        }
        Some("agent_thought_chunk") => Ok(Frame::Thought),
        Some("tool_call" | "tool_call_update") => Err("native_tools_open"),
        _ => Err("unknown_update"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_fixture_separates_thought_and_rejects_native_tools() {
        // Synthetic transcript based on the local reference's ACP field names;
        // not represented as a recording of a credentialed vendor session.
        let frames: Vec<Value> = include_str!("../tests/fixtures/opencode-acp.jsonl")
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(
            decode(&frames[0], "session", 4),
            Ok(Frame::Text("hello".into()))
        );
        assert_eq!(decode(&frames[1], "session", 4), Ok(Frame::Thought));
        assert_eq!(decode(&frames[2], "session", 4), Err("native_tools_open"));
        assert_eq!(decode(&frames[3], "session", 4), Err("native_request"));
        assert_eq!(decode(&frames[4], "session", 4), Ok(Frame::Completed));
        assert_eq!(decode(&frames[0], "other", 4), Ok(Frame::Ignore));
        assert_eq!(
            select_model(3, "session", "provider/model#variant")["params"]["value"],
            "provider/model#variant"
        );
        assert_eq!(
            session_request(2, "/work", Some("session"))["method"],
            "session/load"
        );
        assert_eq!(
            prompt(4, "session", "private\n%text%")["params"]["prompt"][0]["text"],
            "private\n%text%"
        );
        assert_eq!(ARGV, ["acp"]);
        assert!(!crate::engine_spec("opencode").unwrap().runnable);
    }
}
