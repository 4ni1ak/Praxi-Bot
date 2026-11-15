use memory_store::knowledge::{KnowledgeStore, NewSource, Scope, SourceKind};
use protocol::{BotId, UserId};

fn fixture() -> (tempfile::TempDir, Scope, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let scope = Scope {
        user_id: UserId::new(),
        bot_id: BotId::new(),
    };
    let store = KnowledgeStore::open(dir.path(), scope).unwrap();
    store
        .add_source(NewSource::new(SourceKind::Note, "Note", "note.md"))
        .unwrap();
    drop(store);
    let path = dir
        .path()
        .join("bots")
        .join(scope.bot_id.to_string())
        .join("knowledge.db");
    (dir, scope, path)
}

#[test]
fn import_rejects_trigger_that_restores_source_authority() {
    let (_dir, scope, path) = fixture();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER restore_authority AFTER UPDATE ON sources WHEN new.enabled=0 BEGIN
             UPDATE sources SET enabled=1,remote_embedding_json=old.remote_embedding_json WHERE id=new.id;
         END",
    )
    .unwrap();
    drop(conn);
    assert!(
        KnowledgeStore::prepare_import(&path, scope).is_err(),
        "import accepted a trigger that restores disabled sources"
    );
}

#[test]
fn import_rejects_trigger_that_suppresses_source_deletion() {
    let (_dir, _scope, path) = fixture();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER prevent_delete BEFORE DELETE ON sources BEGIN SELECT RAISE(IGNORE); END",
    )
    .unwrap();
    drop(conn);
    assert!(
        KnowledgeStore::validate_and_migrate(&path).is_err(),
        "import accepted a trigger that silently suppresses deletion"
    );
}

#[test]
fn import_rejects_unexpected_schema_objects() {
    for sql in [
        "CREATE TABLE retained_content(body TEXT)",
        "CREATE VIEW source_alias AS SELECT * FROM sources",
        "CREATE UNIQUE INDEX arbitrary_source_limit ON sources(enabled)",
    ] {
        let (_dir, _scope, path) = fixture();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(sql).unwrap();
        drop(conn);
        assert!(
            KnowledgeStore::validate_and_migrate(&path).is_err(),
            "accepted unexpected schema object: {sql}"
        );
    }
}

#[test]
fn canonical_schema_accepts_data_and_page_layout_changes() {
    let (dir, scope, path) = fixture();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("VACUUM").unwrap();
    drop(conn);
    KnowledgeStore::validate_and_migrate(&path).unwrap();
    KnowledgeStore::prepare_import(&path, scope).unwrap();
    let reopened = KnowledgeStore::open(dir.path(), scope).unwrap();
    let sources = reopened.list_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert!(!sources[0].enabled);
    assert_eq!(sources[0].revision, 2);
}
