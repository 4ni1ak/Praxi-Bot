//! Review finding 6: OAuth errors and dependency tracing never carry token,
//! verifier, code or client-secret values — even when the authorization
//! server reflects them in `error_description` and whatever subscriber is
//! installed.  Its own binary: it installs a process-wide subscriber.

#[path = "support/oauth_fixture.rs"]
mod fixture;

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use fixture::*;
use mcp_bridge::oauth::{SignIn, SignInOptions};
use mcp_bridge::{McpError, McpManager};
use parking_lot::Mutex;
use secret_store::SecretStore;

const CLIENT_SECRET: &str = "reflected-client-secret-sentinel-5e0b";
const BAD_CODE: &str = "reflected-code-sentinel-c71d";

/// Everything any event or span field said, at every level.
static CAPTURED: Mutex<String> = Mutex::new(String::new());

struct Capture;

struct Fields<'a>(&'a mut String);

impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, " {}={value:?}", field.name());
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        let _ = write!(self.0, " {}={value}", field.name());
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }
    // Format first and lock only to append: formatting a value may log.
    fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let mut text = String::new();
        span.record(&mut Fields(&mut text));
        CAPTURED.lock().push_str(&text);
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let mut text = String::new();
        values.record(&mut Fields(&mut text));
        CAPTURED.lock().push_str(&text);
    }
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut text = String::from("\n");
        event.record(&mut Fields(&mut text));
        CAPTURED.lock().push_str(&text);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Values a token request carried that must never be shown anywhere.
fn submitted_secrets(seen: &Mutex<Seen>) -> Vec<String> {
    let mut out = vec![
        CLIENT_SECRET.to_owned(),
        BAD_CODE.to_owned(),
        ACCESS.to_owned(),
        REFRESH.to_owned(),
    ];
    for form in &seen.lock().token_requests {
        for key in ["code", "code_verifier", "client_secret", "refresh_token"] {
            if let Some(value) = form.get(key) {
                out.push(value.clone());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn reflected_secrets_and_dependency_logs_never_reach_errors_or_any_subscriber() {
    tracing::subscriber::set_global_default(Capture).unwrap();
    tracing::callsite::rebuild_interest_cache();
    let (base, seen, server) = fake_authorization_server(true).await;
    {
        let mut s = seen.lock();
        s.client_secret = Some(CLIENT_SECRET.into());
        s.echo_errors = true;
        s.expires_in = Some(45);
    }
    let confidential = || SignInOptions {
        client_id: Some("praxi-test-client".into()),
        client_secret: Some(CLIENT_SECRET.into()),
        expected_issuer: Some(base.clone()),
        allow_loopback_http: true,
        ..SignInOptions::default()
    };
    let mut errors = String::new();

    // 1. A successful sign-in (rmcp logs the code and the token response at
    //    debug level).
    let store = Arc::new(MemoryStore::default());
    let sign_in = SignIn::start(
        "ok",
        &format!("{base}/mcp"),
        store.clone(),
        None,
        confidential(),
    )
    .await
    .unwrap();
    finish_sign_in(sign_in, &base, &seen).await;

    // 2. A refused code exchange whose error reflects code, verifier and secret.
    let sign_in = SignIn::start(
        "bad",
        &format!("{base}/mcp"),
        store.clone(),
        None,
        confidential(),
    )
    .await
    .unwrap();
    let url = sign_in.authorization_url().to_owned();
    let params = pairs(url.split_once('?').unwrap().1);
    seen.lock().challenge = Some(params["code_challenge"].clone());
    let finishing = tokio::spawn(sign_in.finish(Duration::from_secs(20)));
    redirect(
        &url,
        &format!("code={BAD_CODE}&state={}&iss={base}", params["state"]),
    )
    .await
    .unwrap();
    let refused = finishing.await.unwrap().unwrap_err();
    errors.push_str(&refused.to_string());
    let _ = write!(errors, "{refused:?}");

    // 3. A refused refresh whose error reflects the refresh token and secret.
    store.set("mcp/client-secret", CLIENT_SECRET).unwrap();
    let manager =
        McpManager::new(tool_registry::ToolRegistry::new()).with_secret_store(store.clone());
    let mut config = http_config("refresh", format!("{base}/mcp"));
    config.oauth.as_mut().unwrap().client_secret_ref = Some("mcp/client-secret".into());
    let _ = manager.add_server(config).await;
    finish_sign_in(
        manager
            .start_sign_in("refresh", SignInOptions::default())
            .await
            .unwrap(),
        &base,
        &seen,
    )
    .await;
    seen.lock().refresh_allowed = false;
    let refreshed = manager.reenable("refresh").await;
    assert!(refreshed.is_err());
    if let Err(e) = &refreshed {
        errors.push_str(&e.to_string());
        let _ = write!(errors, "{e:?}");
        assert!(!matches!(e, McpError::Quarantined(_)));
    }
    let snapshot = manager.snapshot("refresh").unwrap();
    let _ = write!(errors, "{snapshot:?}");
    manager.shutdown().await;
    server.abort();

    // The token endpoint really reflected the values (the test is live).
    assert!(seen.lock().token_requests.len() >= 3);
    let captured = CAPTURED.lock().clone();
    for value in submitted_secrets(&seen) {
        if value.len() < 8 {
            continue;
        }
        assert!(
            !errors.contains(&value),
            "an error carries a submitted value: {errors}"
        );
        assert!(
            !captured.contains(&value),
            "a log event carries a submitted value"
        );
    }
}
