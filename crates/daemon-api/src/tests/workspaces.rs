//! HTTP API tests for per-bot persistent workspaces, disk thresholds, cleanup, and isolation.

use std::time::{Duration, SystemTime};

use protocol::BotId;

use super::*;

async fn create_test_bot(t: &T, name: &str) -> uuid::Uuid {
    let profile_toml = format!(
        "[bot]\nid = \"{}\"\nname = \"{name}\"\n\n[models]\nexecutor = \"fake/echo\"\n",
        name.to_ascii_lowercase()
    );
    let r = call(
        t,
        Method::POST,
        "/v1/bots",
        Some(json!({ "profile_toml": profile_toml })),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "bot create failed: {:?}",
        r.json()
    );
    r.json()["id"].as_str().unwrap().parse().unwrap()
}

fn setup_workspace_app() -> (tempfile::TempDir, T) {
    let t = setup();
    let ws_root = tempfile::tempdir().unwrap();
    t.state
        .set_workspaces_root(Some(ws_root.path().to_path_buf()));
    (ws_root, t)
}

#[tokio::test]
async fn workspace_routes_are_owner_only_and_denied_to_peers() {
    let (_ws, t) = setup_workspace_app();
    let bot_id = create_test_bot(&t, "Researcher").await;

    // Missing owner key over loopback is rejected even with valid bearer token.
    let r = call_token_only(&t, Method::GET, "/v1/workspaces", None).await;
    assert_eq!(
        r.status,
        StatusCode::FORBIDDEN,
        "expected owner guard: {:?}",
        r.json()
    );

    let r = call_token_only(
        &t,
        Method::GET,
        &format!("/v1/bots/{bot_id}/workspace"),
        None,
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::FORBIDDEN,
        "expected owner guard: {:?}",
        r.json()
    );

    // A paired peer cannot access workspace overview or bot workspace files via peer_router.
    let peer = crate::peer_router(t.state.clone()).layer(axum::Extension(crate::PeerIdentity {
        key: "paired-test-peer".into(),
        role: crate::PeerRole::Desktop,
    }));
    let r = send(&peer, Method::GET, "/v1/workspaces", None, &[]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let r = send(
        &peer,
        Method::GET,
        &format!("/v1/bots/{bot_id}/workspace"),
        None,
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn workspace_browse_search_preview_rename_delete_restore_and_limits() {
    let (ws_root, t) = setup_workspace_app();
    let bot_a = create_test_bot(&t, "Alpha").await;
    let bot_b = create_test_bot(&t, "Beta").await;

    // Listing creates the workspace folder and 5 standard subfolders cleanly.
    let r = call(
        &t,
        Method::GET,
        &format!("/v1/bots/{bot_a}/workspace"),
        None,
    )
    .await;
    let list_a = r.json();
    assert_eq!(r.status, StatusCode::OK, "list_a: {list_a}");
    let names_a: Vec<&str> = list_a["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    for expected in ["downloads", "drafts", "leads", "logs", "notes"] {
        assert!(
            names_a.contains(&expected),
            "missing {expected} in {names_a:?}"
        );
    }

    // Write test files into Alpha and Beta workspaces.
    let ws_a = t.state.agent().bot_workspace_dir(BotId(bot_a)).unwrap();
    let ws_b = t.state.agent().bot_workspace_dir(BotId(bot_b)).unwrap();
    assert_ne!(ws_a, ws_b);
    std::fs::write(ws_a.join("notes/report.md"), "# Report\nhello").unwrap();
    std::fs::write(
        ws_a.join("notes/icon.svg"),
        "<svg><script>alert(1)</script></svg>",
    )
    .unwrap();
    std::fs::write(ws_b.join("notes/secret.txt"), "beta-only").unwrap();

    // Traversal attempts & missing files fail cleanly.
    let r = call(
        &t,
        Method::GET,
        &format!("/v1/bots/{bot_a}/workspace/file?path=../Beta/notes/secret.txt"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Search finds Alpha's file and never leaks Beta's file.
    let r = call(
        &t,
        Method::GET,
        &format!("/v1/bots/{bot_a}/workspace?q=report"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let search = r.json();
    let hits = search["entries"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["path"], "notes/report.md");

    let r = call(
        &t,
        Method::GET,
        &format!("/v1/bots/{bot_a}/workspace?q=secret"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.json()["entries"].as_array().unwrap().is_empty());

    // Safe inline preview serves .svg as text/plain + nosniff + CSP sandbox (no SVG XSS).
    let resp = call(
        &t,
        Method::GET,
        &format!("/v1/bots/{bot_a}/workspace/file?path=notes/icon.svg"),
        None,
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(
        resp.headers.get("content-type").unwrap().to_str().unwrap(),
        "text/plain; charset=utf-8"
    );
    assert_eq!(
        resp.headers
            .get("x-content-type-options")
            .unwrap()
            .to_str()
            .unwrap(),
        "nosniff"
    );
    assert_eq!(
        resp.headers
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap(),
        "sandbox"
    );

    // Rename, then Delete (with undo), then Restore.
    let r = call(
        &t,
        Method::POST,
        &format!("/v1/bots/{bot_a}/workspace/rename"),
        Some(json!({ "path": "notes/report.md", "new_name": "summary.md" })),
    )
    .await;
    let renamed = r.json();
    assert_eq!(r.status, StatusCode::OK, "rename: {renamed}");
    assert_eq!(renamed["path"], "notes/summary.md");
    assert!(ws_a.join("notes/summary.md").is_file());

    let r = call(
        &t,
        Method::POST,
        &format!("/v1/bots/{bot_a}/workspace/delete"),
        Some(json!({ "path": "notes/summary.md" })),
    )
    .await;
    let deleted = r.json();
    assert_eq!(r.status, StatusCode::OK, "delete: {deleted}");
    assert!(!ws_a.join("notes/summary.md").exists());
    let undo_id = deleted["undo_id"].as_str().unwrap().to_owned();

    let r = call(
        &t,
        Method::POST,
        &format!("/v1/bots/{bot_a}/workspace/restore"),
        Some(json!({ "undo_id": undo_id })),
    )
    .await;
    let restored = r.json();
    assert_eq!(r.status, StatusCode::OK, "restore: {restored}");
    assert!(ws_a.join("notes/summary.md").is_file());

    // Limits validation & persistence.
    let r = call(
        &t,
        Method::PUT,
        &format!("/v1/bots/{bot_a}/workspace/limits"),
        Some(json!({ "soft_limit_bytes": 500, "hard_limit_bytes": 100 })),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "soft > hard must fail");

    let r = call(
        &t,
        Method::PUT,
        &format!("/v1/bots/{bot_a}/workspace/limits"),
        Some(json!({ "soft_limit_bytes": 1024, "hard_limit_bytes": 2048 })),
    )
    .await;
    let limits = r.json();
    assert_eq!(r.status, StatusCode::OK, "limits: {limits}");
    assert_eq!(limits["soft_limit_bytes"], 1024);
    assert_eq!(limits["hard_limit_bytes"], 2048);

    // Overview, settings update, and age-based cleanup on downloads/ and logs/ only.
    let old = SystemTime::now() - Duration::from_secs(40 * 86_400);
    let old_dl = ws_a.join("downloads/old.zip");
    std::fs::write(&old_dl, vec![b'x'; 256]).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&old_dl)
        .unwrap()
        .set_modified(old)
        .unwrap();

    let r = call(
        &t,
        Method::PUT,
        "/v1/workspaces/settings",
        Some(json!({
            "warn_percent_1": 90,
            "warn_percent_2": 80,
            "warn_percent_3": 95,
            "min_free_bytes": 1024,
            "downloads_max_age_days": 30,
            "logs_max_age_days": 30
        })),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::BAD_REQUEST,
        "non-increasing percentages must fail"
    );

    let r = call(
        &t,
        Method::PUT,
        "/v1/workspaces/settings",
        Some(json!({
            "warn_percent_1": 75,
            "warn_percent_2": 85,
            "warn_percent_3": 95,
            "min_free_bytes": 5368709120u64,
            "downloads_max_age_days": 30,
            "logs_max_age_days": 14
        })),
    )
    .await;
    let settings = r.json();
    assert_eq!(r.status, StatusCode::OK, "settings: {settings}");
    assert_eq!(settings["warn_percent_1"], 75);

    let r = call(&t, Method::GET, "/v1/workspaces", None).await;
    let overview = r.json();
    assert_eq!(r.status, StatusCode::OK, "overview: {overview}");
    assert_eq!(overview["configured"], true);
    assert!(
        overview["root_path"]
            .as_str()
            .unwrap()
            .contains(ws_root.path().file_name().unwrap().to_str().unwrap())
    );

    let r = call(
        &t,
        Method::POST,
        "/v1/workspaces/cleanup",
        Some(json!({ "target": "old_downloads" })),
    )
    .await;
    let cleaned = r.json();
    assert_eq!(r.status, StatusCode::OK, "cleanup: {cleaned}");
    assert_eq!(cleaned["deleted_files"], 1);
    assert_eq!(cleaned["freed_bytes"], 256);
    assert!(!old_dl.exists());
    // Unrelated bot notes remain intact.
    assert!(ws_a.join("notes/summary.md").exists());
    assert!(ws_b.join("notes/secret.txt").exists());
}
