use super::*;
use agent_core::{AgentLoop, LoopConfig, Providers};
use memory_store::knowledge::NewSource;
use protocol::BotId;
use std::sync::Arc;
use storage::{Bot, Storage};
use tool_registry::ToolRegistry;

const P: Params = Params {
    tick_ms: 1_000,
    files_per_minute: 1_000,
    page_pause: Duration::ZERO,
    backoff_cap_ms: 60_000,
    churn_threshold: 3,
};

struct Host {
    state: AppState,
    owner: Scope,
    folder: tempfile::TempDir,
    source: Uuid,
    _data: tempfile::TempDir,
}

impl Host {
    fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(data.path()).unwrap());
        let agent = AgentLoop::new(
            Arc::clone(&storage),
            ToolRegistry::new(),
            Providers::single(Arc::new(crate::fake::FakeProvider)),
            LoopConfig {
                legacy_tool_surface: true,
                ..LoopConfig::default()
            },
        )
        .with_subagents();
        let state = AppState::new(agent, storage, crate::DaemonConfig::new("test-token")).unwrap();
        let bot = BotId::new();
        state
            .storage()
            .insert_bot(&Bot {
                id: bot,
                user_id: state.user_id(),
                name: "Watch test".into(),
                config: serde_json::json!({"bot":{"id":"watch-test","name":"Watch test"},"models":{"executor":"fake/model"},"router":{"enabled":false}}),
                created_at_ms: 1,
            })
            .unwrap();
        let owner = Scope {
            user_id: state.user_id(),
            bot_id: bot,
        };
        let folder = tempfile::tempdir().unwrap();
        let source = KnowledgeStore::open(state.data_dir(), owner)
            .unwrap()
            .add_source(NewSource::new(
                SourceKind::Folder,
                "Notes",
                folder.path().display().to_string(),
            ))
            .unwrap()
            .id;
        Self {
            state,
            owner,
            folder,
            source,
            _data: data,
        }
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.folder.path().join(name), text).unwrap();
    }

    fn indexed(&self) -> Vec<String> {
        let mut names: Vec<_> = KnowledgeStore::open(self.state.data_dir(), self.owner)
            .unwrap()
            .list_documents(self.source)
            .unwrap()
            .into_iter()
            .map(|d| d.locator.trim_end_matches("#extracted-text").to_owned())
            .collect();
        names.sort();
        names
    }

    fn status(&self) -> KnowledgeWatchStatus {
        folder_statuses(&self.state, self.owner)
            .unwrap()
            .pop()
            .unwrap()
    }
}

#[tokio::test]
async fn changes_are_debounced_then_indexed_and_deletes_are_dropped() {
    let host = Host::new();
    host.write("a.md", "alpha notes");
    tick(&host.state, P, 1_000).await;
    assert_eq!(host.indexed(), ["a.md"]);

    host.write("b.md", "beta notes");
    std::fs::remove_file(host.folder.path().join("a.md")).unwrap();
    tick(&host.state, P, 2_000).await;
    // Seen, but not settled yet: nothing re-indexed, status says it is waiting.
    assert_eq!(host.indexed(), ["a.md"]);
    assert_eq!(host.status().state, KnowledgeWatchState::Waiting);

    tick(&host.state, P, 3_000).await;
    assert_eq!(host.indexed(), ["b.md"]);
    let status = host.status();
    assert_eq!(status.state, KnowledgeWatchState::Watching);
    assert_eq!(status.indexed_files, 1);
    assert_eq!(status.last_sync_ms, Some(3_000));
}

#[tokio::test]
async fn secret_looking_files_are_never_indexed() {
    let host = Host::new();
    host.write("plan.md", "visible");
    host.write(".env", "TOKEN=abc");
    host.write("credentials.json", "{}");
    tick(&host.state, P, 1_000).await;
    assert_eq!(host.indexed(), ["plan.md"]);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_out_of_the_folder_are_not_followed() {
    let host = Host::new();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("private.md"), "outside").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("private.md"),
        host.folder.path().join("link.md"),
    )
    .unwrap();
    host.write("own.md", "inside");
    tick(&host.state, P, 1_000).await;
    assert_eq!(host.indexed(), ["own.md"]);
}

#[tokio::test]
async fn constant_churn_backs_off_instead_of_indexing() {
    let host = Host::new();
    host.write("a.md", "v0");
    tick(&host.state, P, 1_000).await;
    for (i, now) in [2_000, 3_000, 4_000, 5_000].into_iter().enumerate() {
        host.write("a.md", &format!("a longer version {i}{}", "x".repeat(i * 3)));
        tick(&host.state, P, now).await;
    }
    assert_eq!(host.status().state, KnowledgeWatchState::BackingOff);
    // Still the first version: churn never settled.
    let store = KnowledgeStore::open(host.state.data_dir(), host.owner).unwrap();
    assert_eq!(store.list_documents(host.source).unwrap().len(), 1);
    assert!(host.state.0.knowledge_watch.sources.lock()[&host.source].next_check_ms > 6_000);
}

#[tokio::test]
async fn the_per_minute_file_budget_spreads_a_large_folder_over_time() {
    let host = Host::new();
    for i in 0..40 {
        host.write(&format!("f{i:02}.md"), &format!("note {i}"));
    }
    let slow = Params {
        files_per_minute: 10,
        ..P
    };
    tick(&host.state, slow, 1_000).await;
    // One page (32 files) used up the budget; the rest waits for the next minute.
    assert_eq!(host.indexed().len(), 32);
    assert_eq!(host.status().state, KnowledgeWatchState::Waiting);
    tick(&host.state, slow, 30_000).await;
    assert_eq!(host.indexed().len(), 32);
    tick(&host.state, slow, 62_000).await;
    assert_eq!(host.indexed().len(), 40);
}

#[tokio::test]
async fn paused_folder_is_left_alone_until_resumed() {
    let host = Host::new();
    host.write("a.md", "alpha");
    tick(&host.state, P, 1_000).await;
    host.state
        .0
        .knowledge_watch
        .set_paused(&host.state.data_dir(), host.source, true);
    host.write("b.md", "beta");
    tick(&host.state, P, 2_000).await;
    tick(&host.state, P, 3_000).await;
    assert_eq!(host.indexed(), ["a.md"]);
    assert_eq!(host.status().state, KnowledgeWatchState::Paused);
    // The pause survives a restart (new hub reads the file).
    assert!(
        WatchHub::default()
            .paused_set(&host.state.data_dir())
            .contains(&host.source)
    );
    host.state
        .0
        .knowledge_watch
        .set_paused(&host.state.data_dir(), host.source, false);
    tick(&host.state, P, 4_000).await;
    assert_eq!(host.indexed(), ["a.md", "b.md"]);
}

#[tokio::test]
async fn a_missing_folder_reports_a_plain_error_and_recovers() {
    let host = Host::new();
    host.write("a.md", "alpha");
    tick(&host.state, P, 1_000).await;
    let path = host.folder.path().to_path_buf();
    let moved = path.with_extension("moved");
    std::fs::rename(&path, &moved).unwrap();
    tick(&host.state, P, 10_000).await;
    assert_eq!(host.status().error.unwrap().code, "folder_missing");
    std::fs::rename(&moved, &path).unwrap();
    tick(&host.state, P, 100_000).await;
    assert!(host.status().error.is_none());
}
