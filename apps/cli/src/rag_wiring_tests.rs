//! Regression: first-class knowledge must be registered in the shipped host,
//! not only usable from memory-store's isolated library tests.

#[test]
fn shipped_agent_registers_knowledge_search_without_any_embedding_provider() {
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(storage::Storage::open(dir.path()).unwrap());
    let agent = crate::serve::build_agent(
        &storage,
        agent_core::Providers::unconfigured(),
        dir.path(),
        None,
    );
    assert!(
        agent
            .tool_names()
            .iter()
            .any(|name| name == "knowledge_search")
    );
}
