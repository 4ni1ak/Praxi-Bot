use memory_store::knowledge::{
    DocumentFormat, DocumentInput, KnowledgeStore, NewSource, Scope, SourceKind,
};
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

#[test]
fn version_two_import_requires_its_embedding_objects() {
    for sql in [
        "DROP TABLE embedding_cache",
        "DROP TABLE embedding_jobs",
        "DROP TABLE knowledge_embeddings",
        "DROP INDEX chunks_identity_source",
    ] {
        let (_dir, _scope, path) = fixture();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(sql).unwrap();
        drop(conn);
        assert!(
            KnowledgeStore::validate_and_migrate(&path).is_err(),
            "accepted a partial embedding schema after: {sql}"
        );
    }
}

#[test]
fn version_one_index_migrates_to_the_canonical_embedding_schema() {
    let (dir, scope, path) = fixture();
    let conn = rusqlite::Connection::open(&path).unwrap();
    // Restore the exact previous release's DDL while retaining its source data.
    conn.execute_batch(
        "DROP TRIGGER knowledge_embedding_grant_changed;
         DROP TABLE knowledge_embeddings;
         DROP TABLE embedding_cache;
         DROP TABLE embedding_jobs;
         DROP INDEX chunks_identity_source;
         PRAGMA user_version=1;",
    )
    .unwrap();
    drop(conn);
    assert_eq!(KnowledgeStore::validate_and_migrate(&path).unwrap(), 2);
    let reopened = KnowledgeStore::open(dir.path(), scope).unwrap();
    let sources = reopened.list_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert!(sources[0].enabled);
    assert_eq!(sources[0].revision, 1);
}

fn assert_row_mutations_rejected(mutations: &[&str]) {
    let mut accepted = Vec::new();
    for &sql in mutations {
        let (dir, scope, path) = fixture();
        let store = KnowledgeStore::open(dir.path(), scope).unwrap();
        let source = store.list_sources().unwrap().remove(0);
        store
            .index_document(
                source.id,
                source.revision,
                &DocumentInput {
                    locator: "note.md".into(),
                    title: "Note".into(),
                    format: DocumentFormat::Markdown,
                    content: "# Note\nimportrowcanary".into(),
                    mtime_ms: Some(1),
                },
            )
            .unwrap();
        drop(store);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "foreign_keys", false).unwrap();
        conn.execute_batch(sql).unwrap();
        drop(conn);
        // Keep checking every mutation so RED evidence identifies every row
        // category that a structurally valid portable database could hide.
        let validation = KnowledgeStore::validate_and_migrate(&path);
        let import = KnowledgeStore::prepare_import(&path, scope);
        if validation.is_ok() || import.is_ok() {
            accepted.push(sql);
        }
    }
    assert!(accepted.is_empty(), "accepted malformed rows: {accepted:?}");
}

#[test]
fn import_rejects_unknown_source_kind_and_document_format() {
    assert_row_mutations_rejected(&[
        r#"UPDATE sources SET kind='"unknown_kind"'"#,
        r#"UPDATE documents SET format='"unknown_format"'"#,
    ]);
}

#[test]
fn import_rejects_malformed_identifiers_even_when_foreign_keys_match() {
    assert_row_mutations_rejected(&[
        "UPDATE sources SET id='not-a-uuid';
         UPDATE documents SET source_id='not-a-uuid';
         UPDATE chunks SET source_id='not-a-uuid';",
        "UPDATE documents SET id='not-a-uuid'; UPDATE chunks SET document_id='not-a-uuid';",
        "UPDATE chunks SET id='not-a-uuid'",
        "UPDATE sources SET id='00000000-0000-0000-0000-00000000000A';
         UPDATE documents SET source_id='00000000-0000-0000-0000-00000000000A';
         UPDATE chunks SET source_id='00000000-0000-0000-0000-00000000000A';",
    ]);
}

#[test]
fn import_rejects_malformed_metadata_in_canonical_tables() {
    assert_row_mutations_rejected(&[
        "UPDATE sources SET created_ms='not-a-time'",
        "UPDATE sources SET label='Note'||char(10)",
        "UPDATE documents SET mtime_ms='not-a-time'",
        "UPDATE documents SET title='Note'||char(10); UPDATE chunks_fts SET title='Note'||char(10);",
        "UPDATE chunks SET ordinal=-1",
        "UPDATE chunks SET body=X'ff'; UPDATE chunks_fts SET body=X'ff';",
        "UPDATE documents SET content_hash=printf('%064d',0)||''; UPDATE documents SET content_hash=replace(content_hash,'0','z');",
    ]);
}

#[test]
fn import_rejects_malformed_source_consent_before_suspending_it() {
    assert_row_mutations_rejected(&[
        "UPDATE sources SET remote_embedding_json='{}'",
        r#"UPDATE sources SET remote_embedding_json='{"profile_fingerprint":"bad","granted_at_ms":1}'"#,
        r#"UPDATE sources SET remote_embedding_json='{"profile_fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","granted_at_ms":-1}'"#,
    ]);
}
