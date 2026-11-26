//! The client-side handler of every MCP session.
//!
//! The bridge advertises one client capability: form elicitation. Other
//! server-initiated requests keep rmcp's defaults: sampling is `method not
//! found` and roots are empty. The handler also reacts to
//! `notifications/tools/list_changed`: the owning server is told to list its
//! tools again, so a tool a server adds, removes or redefines at runtime never
//! stays stale in the registry.
//!
//! **Elicitation.** A server may ask the owner for input only while one of its
//! tool calls runs: the request belongs to that call's run. With no call or
//! several calls in flight on the server it is declined (the bridge never
//! guesses the run). A form the bridge cannot show (URL mode, a multi-select
//! list, an unknown type) is declined too. The answer goes back to the server
//! only; when the call ends first, the server gets `cancel`.

use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

use parking_lot::{Mutex, RwLock};
use protocol::event::{ElicitationField, ElicitationFieldType, ElicitationOption};
use rmcp::{
    ClientHandler, ErrorData, RoleClient,
    model::{
        ClientConfig, ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationCapability,
        ElicitationSchema, FormElicitationCapability,
    },
    service::{NotificationContext, RequestContext, RunningService},
};
use serde_json::Value;
use tool_registry::{ElicitationAnswer, ElicitationRequest, Elicitor};

/// Where the host installs its [`Elicitor`]; shared by every server of a manager.
pub(crate) type ElicitorSlot = RwLock<Option<Arc<dyn Elicitor>>>;

/// One tool call in flight on a server.
struct InFlight {
    id: u64,
    run_id: String,
    /// Closed when the call ends.
    done: tokio::sync::watch::Receiver<()>,
}

/// Signals a session raises for the server that owns it. They outlive a
/// single session: a reconnect keeps the same instance.
#[derive(Default)]
pub(crate) struct Signals {
    /// The server announced that its tool list changed.
    pub(crate) tools_changed: tokio::sync::Notify,
    /// The server's configured id, shown on the form.
    server: String,
    elicitor: Arc<ElicitorSlot>,
    calls: Mutex<Vec<InFlight>>,
    next_call: AtomicU64,
    /// Forms open right now: the call waits for the owner, not for the server.
    eliciting: AtomicUsize,
}

/// Marks a tool call in flight until dropped.
pub(crate) struct CallGuard {
    signals: Arc<Signals>,
    id: u64,
    _done: tokio::sync::watch::Sender<()>,
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        self.signals.calls.lock().retain(|c| c.id != self.id);
    }
}

struct Eliciting<'a>(&'a AtomicUsize);

impl Drop for Eliciting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Signals {
    pub(crate) fn new(server: String, elicitor: Arc<ElicitorSlot>) -> Self {
        Self {
            server,
            elicitor,
            ..Self::default()
        }
    }

    /// Record a tool call of run `run_id` until the guard is dropped.
    pub(crate) fn begin_call(self: &Arc<Self>, run_id: &str) -> CallGuard {
        let id = self.next_call.fetch_add(1, Ordering::Relaxed);
        let (tx, done) = tokio::sync::watch::channel(());
        self.calls.lock().push(InFlight {
            id,
            run_id: run_id.to_owned(),
            done,
        });
        CallGuard {
            signals: Arc::clone(self),
            id,
            _done: tx,
        }
    }

    /// A form of this server is waiting for the owner.
    pub(crate) fn is_eliciting(&self) -> bool {
        self.eliciting.load(Ordering::Acquire) > 0
    }

    async fn elicit(&self, request: ElicitRequestParams) -> ElicitResult {
        let decline = || ElicitResult::new(ElicitationAction::Decline);
        let ElicitRequestParams::FormElicitationParams {
            message,
            requested_schema,
            ..
        } = request
        else {
            return decline();
        };
        let Some(fields) = fields_of(&requested_schema) else {
            return decline();
        };
        let Some(elicitor) = self.elicitor.read().clone() else {
            return decline();
        };
        let (run_id, mut done) = match self.calls.lock().as_slice() {
            [call] => (call.run_id.clone(), call.done.clone()),
            _ => return decline(),
        };
        self.eliciting.fetch_add(1, Ordering::AcqRel);
        let _open = Eliciting(&self.eliciting);
        let request = ElicitationRequest {
            server: self.server.clone(),
            message,
            fields,
        };
        let answer = tokio::select! {
            answer = elicitor.elicit(&run_id, request) => answer,
            // The call ended (timeout, Stop): nobody waits for the answer.
            _ = done.changed() => ElicitationAnswer::Cancel,
        };
        match answer {
            ElicitationAnswer::Accept(content) => {
                ElicitResult::new(ElicitationAction::Accept).with_content(Value::Object(content))
            }
            ElicitationAnswer::Decline => decline(),
            ElicitationAnswer::Cancel => ElicitResult::new(ElicitationAction::Cancel),
        }
    }
}

/// The form fields of a flat schema, in the server's order. `None` when a
/// property is not a single string, number, integer, boolean or choice.
fn fields_of(schema: &ElicitationSchema) -> Option<Vec<ElicitationField>> {
    let required = schema.required.clone().unwrap_or_default();
    let order = schema
        .property_order
        .clone()
        .unwrap_or_else(|| schema.properties.keys().cloned().collect());
    order
        .into_iter()
        .map(|name| {
            let property = serde_json::to_value(schema.properties.get(&name)?).ok()?;
            let required = required.contains(&name);
            field(name, &property, required)
        })
        .collect()
}

fn field(name: String, p: &Value, required: bool) -> Option<ElicitationField> {
    let text = |key: &str| p.get(key).and_then(Value::as_str).map(str::to_owned);
    let length = |key: &str| {
        p.get(key)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    };
    let kind = match p.get("type")?.as_str()? {
        "string" => ElicitationFieldType::String,
        "number" => ElicitationFieldType::Number,
        "integer" => ElicitationFieldType::Integer,
        "boolean" => ElicitationFieldType::Boolean,
        _ => return None,
    };
    let options = options(p)?;
    if !options.is_empty() && kind != ElicitationFieldType::String {
        return None;
    }
    Some(ElicitationField {
        name,
        title: text("title"),
        description: text("description"),
        kind,
        required,
        options,
        minimum: p.get("minimum").and_then(Value::as_f64),
        maximum: p.get("maximum").and_then(Value::as_f64),
        min_length: length("minLength"),
        max_length: length("maxLength"),
        format: text("format"),
        default: p.get("default").cloned(),
    })
}

/// The choices of an `enum` (with optional `enumNames`) or a titled `oneOf`.
fn options(p: &Value) -> Option<Vec<ElicitationOption>> {
    let mut out = Vec::new();
    if let Some(values) = p.get("enum").and_then(Value::as_array) {
        let names = p.get("enumNames").and_then(Value::as_array);
        for (i, v) in values.iter().enumerate() {
            let value = v.as_str()?.to_owned();
            let label = names
                .and_then(|n| n.get(i))
                .and_then(Value::as_str)
                .map_or_else(|| value.clone(), str::to_owned);
            out.push(ElicitationOption { value, label });
        }
    } else if let Some(choices) = p.get("oneOf").and_then(Value::as_array) {
        for c in choices {
            let value = c.get("const")?.as_str()?.to_owned();
            let label = c
                .get("title")
                .and_then(Value::as_str)
                .map_or_else(|| value.clone(), str::to_owned);
            out.push(ElicitationOption { value, label });
        }
    }
    Some(out)
}

/// Handler installed on every session.
#[derive(Clone, Default)]
pub(crate) struct BridgeClient {
    pub(crate) signals: Arc<Signals>,
}

/// A running client session with the bridge's handler.
pub(crate) type Running = RunningService<RoleClient, BridgeClient>;

impl ClientHandler for BridgeClient {
    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        // `notify_one` keeps one permit, so a notification that arrives while
        // the watcher is busy listing is not lost and a burst collapses.
        self.signals.tools_changed.notify_one();
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        Ok(self.signals.elicit(request).await)
    }

    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
        info.capabilities.elicitation =
            Some(ElicitationCapability::new().with_form(FormElicitationCapability::new()));
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Echo;

    #[async_trait::async_trait]
    impl Elicitor for Echo {
        async fn elicit(&self, run_id: &str, request: ElicitationRequest) -> ElicitationAnswer {
            let mut content = serde_json::Map::new();
            content.insert("run".into(), Value::String(run_id.to_owned()));
            content.insert("server".into(), Value::String(request.server));
            ElicitationAnswer::Accept(content)
        }
    }

    struct Never;

    #[async_trait::async_trait]
    impl Elicitor for Never {
        async fn elicit(&self, _: &str, _: ElicitationRequest) -> ElicitationAnswer {
            std::future::pending().await
        }
    }

    fn signals(elicitor: Arc<dyn Elicitor>) -> Arc<Signals> {
        Arc::new(Signals::new(
            "srv".into(),
            Arc::new(RwLock::new(Some(elicitor))),
        ))
    }

    fn form() -> ElicitRequestParams {
        ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "Which account?".into(),
            requested_schema: ElicitationSchema::builder()
                .required_email("email")
                .build()
                .unwrap(),
        }
    }

    #[tokio::test]
    async fn exactly_one_call_in_flight_gets_the_form_for_its_run() {
        let s = signals(Arc::new(Echo));
        let _call = s.begin_call("run-1");
        let r = s.elicit(form()).await;
        assert_eq!(r.action, ElicitationAction::Accept);
        let content = r.content.unwrap();
        assert_eq!(content["run"], "run-1");
        assert_eq!(content["server"], "srv");
    }

    #[tokio::test]
    async fn no_call_in_flight_declines() {
        let s = signals(Arc::new(Echo));
        drop(s.begin_call("run-1"));
        assert_eq!(s.elicit(form()).await.action, ElicitationAction::Decline);
    }

    #[tokio::test]
    async fn several_calls_in_flight_decline() {
        let s = signals(Arc::new(Echo));
        let _a = s.begin_call("run-1");
        let _b = s.begin_call("run-2");
        assert_eq!(s.elicit(form()).await.action, ElicitationAction::Decline);
    }

    #[tokio::test]
    async fn no_elicitor_declines() {
        let s = Arc::new(Signals::default());
        let _call = s.begin_call("run-1");
        assert_eq!(s.elicit(form()).await.action, ElicitationAction::Decline);
    }

    #[tokio::test]
    async fn url_mode_declines() {
        let s = signals(Arc::new(Echo));
        let _call = s.begin_call("run-1");
        let url = ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: "open".into(),
            url: "https://example.com/".into(),
            elicitation_id: "x".into(),
        };
        assert_eq!(s.elicit(url).await.action, ElicitationAction::Decline);
    }

    #[tokio::test]
    async fn the_call_ending_cancels_an_open_form() {
        let s = signals(Arc::new(Never));
        let call = s.begin_call("run-1");
        let waiter = {
            let s = Arc::clone(&s);
            tokio::spawn(async move { s.elicit(form()).await })
        };
        while !s.is_eliciting() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(call);
        let r = waiter.await.unwrap();
        assert_eq!(r.action, ElicitationAction::Cancel);
        assert!(!s.is_eliciting());
    }

    #[test]
    fn flat_schema_becomes_fields_and_multi_select_is_refused() {
        let schema: ElicitationSchema = serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "title": "Name", "maxLength": 20},
                "age": {"type": "integer", "minimum": 0, "maximum": 150},
                "ok": {"type": "boolean", "default": true},
                "size": {"type": "string", "enum": ["s", "m"], "enumNames": ["Small", "Medium"]}
            },
            "required": ["name"]
        }))
        .unwrap();
        let fields = fields_of(&schema).unwrap();
        let by = |n: &str| fields.iter().find(|f| f.name == n).unwrap();
        assert_eq!(fields.len(), 4);
        assert!(by("name").required && !by("age").required);
        assert_eq!(by("name").max_length, Some(20));
        assert_eq!(by("age").kind, ElicitationFieldType::Integer);
        assert_eq!(by("age").maximum, Some(150.0));
        assert_eq!(by("ok").default, Some(Value::Bool(true)));
        assert_eq!(by("size").options[1].label, "Medium");

        let multi: ElicitationSchema = serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "tags": {"type": "array", "items": {"type": "string", "enum": ["a", "b"]}}
            }
        }))
        .unwrap();
        assert!(fields_of(&multi).is_none());
    }
}
