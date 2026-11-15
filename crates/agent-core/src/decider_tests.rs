//! Decider settings and construction (no network).

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use provider_catalog::{ChatChunk, ChatProvider, ChatRequest, ProviderError};

use crate::{
    AgentLoop, DeciderApi, DeciderBackendSettings, DeciderSettings, LoopConfig, Providers,
    build_decider,
};

struct Nop;

#[async_trait]
impl ChatProvider for Nop {
    fn id(&self) -> &'static str {
        "fake"
    }
    async fn stream(
        &self,
        _: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        Err(ProviderError::InvalidRequest("not used".into()))
    }
}

fn providers() -> Providers {
    Providers::single(Arc::new(Nop))
}

fn backend(provider: &str, model: &str, api: Option<DeciderApi>) -> DeciderBackendSettings {
    DeciderBackendSettings {
        provider: provider.into(),
        model: model.into(),
        api,
        base_url: None,
        rpm: 0,
        tpm: 0,
        rpd: 0,
    }
}

#[test]
fn settings_round_trip_and_validate() {
    let dir = tempfile::tempdir().unwrap();
    // No file: the defaults, disabled; the free OpenRouter backend is opt-in.
    let d = DeciderSettings::load(dir.path()).unwrap();
    assert!(!d.enabled);
    assert_eq!(d.backends.len(), 1);
    assert_eq!(d.backends[0].effective_api(), DeciderApi::GeminiDecisions);

    // Model ids are plain configuration: a renamed free model is one edit.
    let custom: DeciderSettings = toml::from_str(
        "enabled = true\nlight_model = \"groq/tiny\"\n\
         [[backends]]\nprovider = \"openrouter\"\nmodel = \"some/new-decider-v9:free\"\nrpm = 5\n\
         [[backends]]\nprovider = \"groq\"\nmodel = \"llama-small\"\n",
    )
    .unwrap();
    assert_eq!(custom.backends.len(), 2);
    assert_eq!(
        custom.backends[0].effective_api(),
        DeciderApi::OpenrouterDecisions
    );
    assert_eq!(custom.backends[1].effective_api(), DeciderApi::ChatJson);
    assert!(custom.tool_preselect, "unset switches keep their defaults");
    custom.save(dir.path()).unwrap();
    assert_eq!(DeciderSettings::load(dir.path()).unwrap(), custom);

    for bad in [
        "trivial_max = 1.5",
        "timeout_ms = 5",
        "light_model = \"cli:claude/default\"",
        "[[backends]]\nprovider = \"a/b\"\nmodel = \"m\"",
        "[[backends]]\nprovider = \"a\"\nmodel = \"has space\"",
        "[[backends]]\nprovider = \"a\"\nmodel = \"m\"\nbase_url = \"file:///etc\"",
    ] {
        let s: DeciderSettings = toml::from_str(bad).unwrap();
        assert!(s.validate().is_err(), "{bad}");
    }
    std::fs::write(DeciderSettings::path(dir.path()), "enabled = 3").unwrap();
    assert!(DeciderSettings::load(dir.path()).is_err());
}

#[test]
fn a_disabled_or_unusable_decider_is_not_built() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = provider_catalog::Catalog::default();
    let store = secret_store::MemoryStore::default();
    let warnings = std::cell::RefCell::new(Vec::new());
    let mut warn = |w: &str| warnings.borrow_mut().push(w.to_owned());

    // Off by default, even though the default chain names a backend.
    let off = DeciderSettings::default();
    assert!(build_decider(&off, dir.path(), &catalog, &store, &providers(), &mut warn).is_none());

    // Enabled, but the only backend has no key: degrade to "no decider".
    let keyless = DeciderSettings {
        enabled: true,
        backends: vec![
            backend("nokey-provider", "m", Some(DeciderApi::GeminiDecisions)),
            backend("not-configured", "m", None),
        ],
        ..DeciderSettings::default()
    };
    assert!(
        build_decider(
            &keyless,
            dir.path(),
            &catalog,
            &store,
            &providers(),
            &mut warn
        )
        .is_none()
    );
    assert_eq!(warnings.borrow().len(), 3, "{warnings:?}");

    // A chat-JSON backend over a configured provider is usable; the unusable
    // one before it is skipped, not fatal.
    let chat = DeciderSettings {
        enabled: true,
        backends: vec![
            backend("nokey-provider", "m", Some(DeciderApi::OpenrouterDecisions)),
            backend("fake", "small", None),
        ],
        ..DeciderSettings::default()
    };
    let td = build_decider(&chat, dir.path(), &catalog, &store, &providers(), &mut warn).unwrap();
    assert_eq!(td.backend_ids(), ["fake/small"]);
    assert_eq!(td.settings(), &chat);
    assert_eq!(td.metrics().decisions, 0);
}

#[test]
fn the_loop_holds_the_decider_it_is_given() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(storage::Storage::open(dir.path()).unwrap());
    let lp = AgentLoop::new(
        storage,
        tool_registry::ToolRegistry::new(),
        providers(),
        LoopConfig::default(),
    );
    assert!(lp.decider().is_none());
    let s = DeciderSettings {
        enabled: true,
        backends: vec![backend("fake", "small", None)],
        ..DeciderSettings::default()
    };
    let catalog = provider_catalog::Catalog::default();
    let store = secret_store::MemoryStore::default();
    let td = build_decider(&s, dir.path(), &catalog, &store, &providers(), &mut |_| {});
    lp.set_decider(td);
    assert_eq!(lp.decider().unwrap().backend_ids(), ["fake/small"]);
    lp.set_decider(None);
    assert!(lp.decider().is_none());
}
