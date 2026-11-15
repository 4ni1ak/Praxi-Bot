use super::tests::{bundle, database, pack, replace_payload, unpack};
use super::*;
use memory_store::knowledge::{
    DocumentFormat, DocumentInput, KnowledgeStore, NewSource, RemoteEmbeddingConsent, Scope,
    SearchRequest, SourceKind, SourceUpdate,
};
use protocol::{BotId, UserId};
use rusqlite::Connection;
use secret_store::MemoryStore;

#[test]
fn knowledge_paths_and_sidecars_cannot_use_case_aliases() {
    assert!(
        crate::knowledge::bot_from_path("bots/00000000-0000-0000-0000-000000000002/knowledge.db")
            .is_some()
    );
    assert!(
        crate::knowledge::bot_from_path("bots/00000000-0000-0000-0000-000000000002/Knowledge.DB")
            .is_none()
    );
    assert!(crate::knowledge::excluded("bots/x/Knowledge.DB-WAL"));
    assert!(crate::knowledge::excluded("KNOWLEDGE-RUNTIME.DB-SHM"));
}

#[tokio::test]
async fn knowledge_case_alias_payload_cannot_bypass_import_validation() {
    let source = tempfile::tempdir().unwrap();
    let (_store, _) = seeded(source.path());
    let bytes = bundle(source.path()).await;
    let mut entries = unpack(&bytes);
    let original = format!("bots/{}/knowledge.db", scope().bot_id);
    let alias = format!("bots/{}/Knowledge.DB", scope().bot_id);
    entries
        .iter_mut()
        .find(|(name, _)| name == &original)
        .unwrap()
        .0
        .clone_from(&alias);
    let last = entries.last_mut().unwrap();
    let mut manifest: Manifest = serde_json::from_slice(&last.1).unwrap();
    manifest
        .files
        .iter_mut()
        .find(|record| record.path == original)
        .unwrap()
        .path = alias;
    // Legacy manifests may omit this map, so filename validation is essential.
    manifest.db_schemas = None;
    last.1 = serde_json::to_vec(&manifest).unwrap();
    let payload = pack(&entries);
    let target = tempfile::tempdir().unwrap();
    assert!(
        import(
            target.path(),
            &MemoryStore::new(),
            payload.as_slice(),
            ImportOptions::default()
        )
        .await
        .is_err()
    );
    assert!(!target.path().join("bot.db").exists());
}

fn scope() -> Scope {
    Scope {
        user_id: UserId("00000000-0000-0000-0000-000000000001".parse().unwrap()),
        bot_id: BotId("00000000-0000-0000-0000-000000000002".parse().unwrap()),
    }
}

fn seeded(path: &Path) -> (KnowledgeStore, uuid::Uuid) {
    database(path, true);
    let store = KnowledgeStore::open(path, scope()).unwrap();
    let source = store
        .add_source(NewSource::new(
            SourceKind::Note,
            "Selected knowledge",
            "note:source",
        ))
        .unwrap();
    let indexed = store
        .index_document(
            source.id,
            source.revision,
            &DocumentInput {
                locator: "one.md".into(),
                title: "Architecture".into(),
                format: DocumentFormat::Markdown,
                content: "# Decision\n\nSQLite panorama keeps lexical retrieval available.".into(),
                mtime_ms: None,
            },
        )
        .unwrap();
    store
        .update_source(
            source.id,
            indexed.source_revision,
            SourceUpdate {
                remote_embedding: Some(Some(RemoteEmbeddingConsent {
                    profile_fingerprint: "a".repeat(64),
                    granted_at_ms: 1,
                })),
                ..Default::default()
            },
        )
        .unwrap();
    (store, source.id)
}

#[tokio::test]
async fn knowledge_live_wal_and_brain_roundtrip_require_reauthorization() {
    let source = tempfile::tempdir().unwrap();
    let (live, id) = seeded(source.path());
    let db_path = format!("bots/{}/knowledge.db", scope().bot_id);
    assert!(source.path().join(format!("{db_path}-wal")).exists());
    std::fs::create_dir(source.path().join("brain")).unwrap();
    std::fs::write(
        source.path().join("brain/decision.md"),
        "# Human note\nSource: reviewed decision\n",
    )
    .unwrap();
    std::fs::write(
        source.path().join("knowledge-runtime.db"),
        "source quota ledger",
    )
    .unwrap();
    let config = serde_json::json!({"knowledge":{"auto_retrieve":true,"embedding":{"profile":{"provider_id":"example"},"query_consent":{"profile_fingerprint":"a".repeat(64),"granted_at_ms":1}}}});
    Connection::open(source.path().join("bot.db"))
        .unwrap()
        .execute("UPDATE bots SET config_json=?1", [config.to_string()])
        .unwrap();
    let bytes = bundle(source.path()).await;
    let entries = unpack(&bytes);
    assert!(entries.iter().any(|(path, _)| path == &db_path));
    assert!(
        !entries
            .iter()
            .any(|(path, _)| path.contains("knowledge-runtime")
                || path.ends_with("knowledge.db-wal")
                || path.ends_with("knowledge.db-shm"))
    );
    let target = tempfile::tempdir().unwrap();
    std::fs::write(
        target.path().join("knowledge-runtime.db"),
        "target quota history",
    )
    .unwrap();
    let outcome = import(
        target.path(),
        &MemoryStore::new(),
        bytes.as_slice(),
        ImportOptions::default(),
    )
    .await
    .unwrap();
    assert!(
        outcome
            .manifest
            .db_schemas
            .as_ref()
            .unwrap()
            .contains_key(&db_path)
    );
    assert_eq!(
        std::fs::read_to_string(target.path().join("knowledge-runtime.db")).unwrap(),
        "target quota history"
    );
    assert_eq!(
        std::fs::read_to_string(target.path().join("brain/decision.md")).unwrap(),
        "# Human note\nSource: reviewed decision\n"
    );
    let restored = KnowledgeStore::open(target.path(), scope()).unwrap();
    let source = restored.source(id).unwrap().unwrap();
    assert!(!source.enabled);
    assert!(source.remote_embedding.is_none());
    assert_eq!(restored.list_documents(id).unwrap().len(), 1);
    assert_eq!(
        restored
            .search(&SearchRequest::new("panorama"))
            .unwrap()
            .hits
            .len(),
        0
    );
    let bot = storage::Storage::open(target.path())
        .unwrap()
        .get_bot(scope().bot_id)
        .unwrap()
        .unwrap();
    assert_eq!(bot.config["knowledge"]["auto_retrieve"], false);
    assert!(bot.config["knowledge"]["embedding"]["query_consent"].is_null());
    restored
        .update_source(
            id,
            source.revision,
            SourceUpdate {
                enabled: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert_ne!(
        restored
            .search(&SearchRequest::new("panorama"))
            .unwrap()
            .hits
            .len(),
        0
    );
    drop(live);
}

#[tokio::test]
async fn knowledge_import_refuses_foreign_owner_and_future_schema() {
    let source = tempfile::tempdir().unwrap();
    let (store, _) = seeded(source.path());
    drop(store);
    let path = format!("bots/{}/knowledge.db", scope().bot_id);
    let bytes = bundle(source.path()).await;
    let wrong = tempfile::tempdir().unwrap();
    let wrong_scope = Scope {
        user_id: UserId::new(),
        bot_id: scope().bot_id,
    };
    drop(KnowledgeStore::open(wrong.path(), wrong_scope).unwrap());
    let tampered = replace_payload(
        &bytes,
        &path,
        std::fs::read(wrong.path().join(&path)).unwrap(),
    );
    let target = tempfile::tempdir().unwrap();
    assert!(
        import(
            target.path(),
            &MemoryStore::new(),
            tampered.as_slice(),
            ImportOptions::default()
        )
        .await
        .is_err()
    );
    assert!(!target.path().join("bot.db").exists());
    let future = source.path().join(&path);
    Connection::open(&future)
        .unwrap()
        .execute_batch("PRAGMA user_version=999999")
        .unwrap();
    let tampered = replace_payload(&bytes, &path, std::fs::read(future).unwrap());
    assert!(
        import(
            target.path(),
            &MemoryStore::new(),
            tampered.as_slice(),
            ImportOptions::default()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn knowledge_import_clears_local_native_artifact_authority() {
    let source = tempfile::tempdir().unwrap();
    let (_store, _) = seeded(source.path());
    let config = serde_json::json!({"knowledge":{"auto_retrieve":true,"embedding":{"local_artifacts":{"runtime":{"path":"/tmp/untrusted-library.so","sha256":"a".repeat(64)}}}}});
    Connection::open(source.path().join("bot.db"))
        .unwrap()
        .execute("UPDATE bots SET config_json=?1", [config.to_string()])
        .unwrap();
    let bytes = bundle(source.path()).await;
    let target = tempfile::tempdir().unwrap();
    import(
        target.path(),
        &MemoryStore::new(),
        bytes.as_slice(),
        ImportOptions::default(),
    )
    .await
    .unwrap();
    let bot = storage::Storage::open(target.path())
        .unwrap()
        .get_bot(scope().bot_id)
        .unwrap()
        .unwrap();
    assert!(bot.config["knowledge"]["embedding"].is_null());
    assert_eq!(bot.config["knowledge"]["auto_retrieve"], false);
}

#[tokio::test]
async fn knowledge_import_rejects_bot_trigger_that_restores_native_artifact_authority() {
    let source = tempfile::tempdir().unwrap();
    let (_store, _) = seeded(source.path());
    let config = serde_json::json!({"knowledge":{"auto_retrieve":true,"embedding":{"local_artifacts":{"runtime":{"path":"/tmp/untrusted-library.so","sha256":"a".repeat(64)}},"query_consent":{"profile_fingerprint":"b".repeat(64)}}}});
    let connection = Connection::open(source.path().join("bot.db")).unwrap();
    connection
        .execute("UPDATE bots SET config_json=?1", [config.to_string()])
        .unwrap();
    connection.execute_batch("CREATE TRIGGER restore_knowledge AFTER UPDATE OF config_json ON bots WHEN json_extract(NEW.config_json,'$.knowledge.embedding') IS NULL BEGIN UPDATE bots SET config_json=OLD.config_json WHERE id=NEW.id; END;").unwrap();
    drop(connection);
    let bytes = bundle(source.path()).await;
    let target = tempfile::tempdir().unwrap();
    let outcome = import(
        target.path(),
        &MemoryStore::new(),
        bytes.as_slice(),
        ImportOptions::default(),
    )
    .await;
    assert!(
        outcome.is_err(),
        "an imported trigger must not survive to undo reauthorization"
    );
    assert!(!target.path().join("bot.db").exists());
}

#[tokio::test]
async fn knowledge_import_cannot_choose_a_fresh_remote_quota_group() {
    let source = tempfile::tempdir().unwrap();
    let (_store, _) = seeded(source.path());
    let config = serde_json::json!({"knowledge":{"auto_retrieve":true,"embedding":{"profile":{"provider_id":"same-provider","endpoint_identity":"https://embedding.example.invalid/v1","model":"embed-v1"},"account_group":"fresh-imported-group","query_consent":{"profile_fingerprint":"a".repeat(64)}}}});
    Connection::open(source.path().join("bot.db"))
        .unwrap()
        .execute("UPDATE bots SET config_json=?1", [config.to_string()])
        .unwrap();
    let bytes = bundle(source.path()).await;
    let target = tempfile::tempdir().unwrap();
    std::fs::write(
        target.path().join("knowledge-runtime.db"),
        "existing consumed quota",
    )
    .unwrap();
    import(
        target.path(),
        &MemoryStore::new(),
        bytes.as_slice(),
        ImportOptions::default(),
    )
    .await
    .unwrap();
    let bot = storage::Storage::open(target.path())
        .unwrap()
        .get_bot(scope().bot_id)
        .unwrap()
        .unwrap();
    assert!(
        bot.config["knowledge"]["embedding"].is_null(),
        "the owner must reselect the provider through the settings endpoint"
    );
    assert_eq!(
        std::fs::read_to_string(target.path().join("knowledge-runtime.db")).unwrap(),
        "existing consumed quota"
    );
}
