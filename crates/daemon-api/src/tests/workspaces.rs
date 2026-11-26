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
async fn workspace_links_are_never_browsed_or_cleaned() {
    let (_root, t) = setup_workspace_app();
    let bot = create_test_bot(&t, "Links").await;
    let ws = t.state.agent().bot_workspace_dir(BotId(bot)).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let old_file = outside.path().join("private.txt");
    std::fs::write(&old_file, "must remain outside").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&old_file)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(40 * 86_400))
        .unwrap();
    // Separate components: a `/` inside the path makes `mklink` read it as a switch.
    let link = ws.join("downloads").join("linked");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "junction creation failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::create_dir(outside.path().join("nested")).unwrap();
    assert!(
        builtin_tools::workspace::validate_workspaces_root(
            &link.join("nested"),
            &t.state.data_dir()
        )
        .is_err(),
        "an existing root below a linked ancestor must be refused"
    );
    let r = call(
        &t,
        Method::GET,
        &format!("/v1/bots/{bot}/workspace/file?path=downloads/linked/private.txt"),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = call(
        &t,
        Method::GET,
        &format!("/v1/bots/{bot}/workspace?q=private"),
        None,
    )
    .await;
    assert_eq!(r.json()["entries"], json!([]));
    let r = call(
        &t,
        Method::POST,
        "/v1/workspaces/cleanup",
        Some(json!({ "target": "old_downloads", "bot_id": bot })),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(&old_file).unwrap(),
        "must remain outside"
    );
}

#[tokio::test]
async fn workspace_restore_honors_expiry_and_hard_limit() {
    let (_root, t) = setup_workspace_app();
    let bot = create_test_bot(&t, "Undo").await;
    let ws = t.state.agent().bot_workspace_dir(BotId(bot)).unwrap();
    std::fs::write(ws.join("notes/undo.txt"), "restore me").unwrap();
    let r = call(
        &t,
        Method::POST,
        &format!("/v1/bots/{bot}/workspace/delete"),
        Some(json!({ "path": "notes/undo.txt" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let undo = r.json()["undo_id"].as_str().unwrap().to_owned();
    call(
        &t,
        Method::PUT,
        &format!("/v1/bots/{bot}/workspace/limits"),
        Some(json!({ "hard_limit_bytes": 1 })),
    )
    .await;
    let restore = format!("/v1/bots/{bot}/workspace/restore");
    let r = call(&t, Method::POST, &restore, Some(json!({ "undo_id": undo }))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(!ws.join("notes/undo.txt").exists());
    call(
        &t,
        Method::PUT,
        &format!("/v1/bots/{bot}/workspace/limits"),
        Some(json!({ "hard_limit_bytes": null })),
    )
    .await;
    let meta_path = t
        .state
        .data_dir()
        .join("workspace-trash")
        .join(bot.to_string())
        .join(&undo)
        .join("meta.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    meta["deleted_at_ms"] = json!(storage::now_ms() - 11 * 60 * 1000);
    std::fs::write(meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let r = call(&t, Method::POST, &restore, Some(json!({ "undo_id": undo }))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(!ws.join("notes/undo.txt").exists());
}

#[tokio::test]
async fn workspace_name_stays_reserved_after_bot_deletion() {
    let (_root, t) = setup_workspace_app();
    let first = create_test_bot(&t, "Reserved").await;
    let first_dir = t.state.agent().bot_workspace_dir(BotId(first)).unwrap();
    std::fs::write(first_dir.join("notes/previous.txt"), "previous bot's files").unwrap();
    let deleted = call(&t, Method::DELETE, &format!("/v1/bots/{first}"), None).await;
    assert!(deleted.status.is_success(), "delete: {:?}", deleted.status);
    let next = create_test_bot(&t, "Reserved").await;
    let next_dir = t.state.agent().bot_workspace_dir(BotId(next)).unwrap();
    assert_ne!(first_dir, next_dir);
    assert!(!next_dir.join("notes/previous.txt").exists());
    assert!(first_dir.join("notes/previous.txt").exists());
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

    // axum serves HEAD for the GET handlers: a bearer token alone must not get an
    // existence / Content-Length oracle for the files of any bot's workspace.
    let ws = t.state.agent().bot_workspace_dir(BotId(bot_id)).unwrap();
    std::fs::write(ws.join("notes").join("client-list.txt"), "private").unwrap();
    for path in [
        "/v1/workspaces".to_owned(),
        format!("/v1/bots/{bot_id}/workspace"),
        format!("/v1/bots/{bot_id}/workspace/file?path=notes/client-list.txt"),
        format!("/v1/bots/{bot_id}/workspace/file?path=notes/missing.txt"),
    ] {
        let r = call_token_only(&t, Method::HEAD, &path, None).await;
        assert_eq!(
            r.status,
            StatusCode::FORBIDDEN,
            "HEAD {path} with a token only"
        );
        let owner = call(&t, Method::HEAD, &path, None).await;
        assert_ne!(
            owner.status,
            StatusCode::FORBIDDEN,
            "HEAD {path} with the owner key"
        );
    }

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
#[allow(clippy::too_many_lines)] // One complete owner file lifecycle, including cleanup safety.
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
    assert_eq!(r.json()["entries"], json!([]));

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
            "min_free_bytes": 5_368_709_120_u64,
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

#[tokio::test]
async fn workspace_warning_event_carries_no_bot_folder_or_file_names() {
    let (_root, t) = setup_workspace_app();
    let bot = create_test_bot(&t, "Acme Merger Bot").await;
    let ws = t.state.agent().bot_workspace_dir(BotId(bot)).unwrap();
    let secret_file = "acme-takeover-draft.pdf";
    std::fs::write(ws.join("notes").join(secret_file), vec![0u8; 4096]).unwrap();
    let folder = ws.file_name().unwrap().to_string_lossy().into_owned();
    let overview = call(&t, Method::GET, "/v1/workspaces", None).await.json();
    assert!(
        overview.to_string().contains(secret_file),
        "the owner-gated overview is where names belong"
    );

    for kind in [
        builtin_tools::workspace::WriteWarningKind::DiskPercent {
            threshold_percent: 90,
            used_percent: 91,
            free_after_bytes: 1_000,
        },
        builtin_tools::workspace::WriteWarningKind::LowFreeSpace {
            free_after_bytes: 10,
            min_free_bytes: 5_000,
            used_percent: 99,
        },
        builtin_tools::workspace::WriteWarningKind::BotSoftLimit {
            used_after_bytes: 9,
            soft_limit_bytes: 1,
        },
    ] {
        let event = agent_core::WorkspaceWarningEvent {
            bot_id: Some(BotId(bot)),
            kind,
            workspace_path: ws.clone(),
        };
        let dto = crate::workspace_routes::warning_event_dto(&t.state, &event);
        let json = serde_json::to_value(&dto).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "available_bytes",
                "below_min_free",
                "disk_used_percent",
                "min_free_bytes",
                "severity",
                "soft_limit_bots",
                "threshold_percent"
            ],
            "{json}"
        );
        let text = json.to_string();
        for leak in [secret_file, "Acme", "acme", folder.as_str(), "notes"] {
            assert!(!text.contains(leak), "event leaks {leak:?}: {text}");
        }
    }
    // The soft-limit event names the bot by id only.
    let soft = crate::workspace_routes::warning_event_dto(
        &t.state,
        &agent_core::WorkspaceWarningEvent {
            bot_id: Some(BotId(bot)),
            kind: builtin_tools::workspace::WriteWarningKind::BotSoftLimit {
                used_after_bytes: 9,
                soft_limit_bytes: 1,
            },
            workspace_path: ws,
        },
    );
    assert_eq!(soft.soft_limit_bots, vec![bot]);
}

/// Make `link` a directory link to `target` (symlink on Unix, junction on Windows).
fn link_dir(link: &std::path::Path, target: &std::path::Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(output.status.success(), "junction creation failed");
    }
}

#[test]
fn workspace_file_read_refuses_a_folder_swapped_for_a_link_after_the_check() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(ws.path().join("notes")).unwrap();
    std::fs::write(ws.path().join("notes").join("a.txt"), "inside").unwrap();
    std::fs::write(outside.path().join("a.txt"), "OUTSIDE SECRET").unwrap();
    let segments = vec!["notes".to_owned(), "a.txt".to_owned()];
    let swap = || {
        std::fs::rename(ws.path().join("notes"), ws.path().join("notes-real")).unwrap();
        link_dir(&ws.path().join("notes"), outside.path());
    };
    let err = crate::workspace_routes::read_workspace_file_checked(
        ws.path(),
        &segments,
        1024,
        "preview",
        &swap,
    )
    .expect_err("a read that follows a link swapped in after the check must be refused");
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
}

#[cfg(unix)]
#[test]
fn workspace_file_read_refuses_a_file_swapped_for_a_symlink_after_the_check() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("a.txt"), "inside").unwrap();
    std::fs::write(outside.path().join("secret.txt"), "OUTSIDE SECRET").unwrap();
    let segments = vec!["a.txt".to_owned()];
    let swap = || {
        std::fs::remove_file(ws.path().join("a.txt")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), ws.path().join("a.txt"))
            .unwrap();
    };
    let err = crate::workspace_routes::read_workspace_file_checked(
        ws.path(),
        &segments,
        1024,
        "preview",
        &swap,
    )
    .expect_err("a file swapped for a symlink after the check must be refused");
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
}

#[test]
fn workspace_file_read_still_serves_a_plain_file() {
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("a.txt"), "inside").unwrap();
    let (_, bytes) = crate::workspace_routes::read_workspace_file_checked(
        ws.path(),
        &["a.txt".to_owned()],
        1024,
        "preview",
        &|| {},
    )
    .unwrap();
    assert_eq!(bytes, b"inside");
}

/// Like `link_dir`, but reports instead of panicking when links cannot be made
/// (a junction normally needs no privilege, a locked-down machine may refuse).
fn try_link_dir(link: &std::path::Path, target: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .creation_flags(0x0800_0000)
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

fn force_mtime(path: &std::path::Path, to: std::time::SystemTime) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(to)
        .unwrap();
}

/// The race: the path is swapped to a link, the OUTSIDE file is opened, and the
/// path is swapped back before the second check.  The decoy inside the
/// workspace has the same size and write time as the outside file (both
/// forgeable by a bot), so only a real file identity or the final path of the
/// opened handle can catch it.
#[test]
fn workspace_file_read_refuses_outside_file_opened_then_path_restored_with_forged_size_and_mtime() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(ws.path().join("notes")).unwrap();
    let decoy = ws.path().join("notes").join("a.txt");
    let secret = outside.path().join("a.txt");
    std::fs::write(&decoy, "DECOY-0123456789").unwrap();
    std::fs::write(&secret, "OUTSIDE-SECRET!!").unwrap();
    force_mtime(
        &decoy,
        std::fs::metadata(&secret).unwrap().modified().unwrap(),
    );
    let probe = ws.path().join("probe-link");
    if !try_link_dir(&probe, outside.path()) {
        eprintln!("SKIPPED: this machine does not allow creating directory links/junctions");
        return;
    }
    std::fs::remove_dir(&probe).unwrap();
    let before = || {
        std::fs::rename(ws.path().join("notes"), ws.path().join("notes-real")).unwrap();
        assert!(try_link_dir(&ws.path().join("notes"), outside.path()));
    };
    let after = || {
        std::fs::remove_dir(ws.path().join("notes")).unwrap();
        std::fs::rename(ws.path().join("notes-real"), ws.path().join("notes")).unwrap();
    };
    let err = crate::workspace_routes::read_workspace_file_checked_with_hooks(
        ws.path(),
        &["notes".to_owned(), "a.txt".to_owned()],
        1024,
        "preview",
        &before,
        &after,
    )
    .expect_err("the outside file must never be served");
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
}

#[cfg(windows)]
#[test]
fn windows_file_identity_is_not_fooled_by_equal_size_and_mtime() {
    use crate::workspace_routes::win_identity::{final_path, same_open_file};
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "same-length-1").unwrap();
    std::fs::write(&b, "same-length-2").unwrap();
    force_mtime(&b, std::fs::metadata(&a).unwrap().modified().unwrap());
    let (ma, mb) = (
        std::fs::metadata(&a).unwrap(),
        std::fs::metadata(&b).unwrap(),
    );
    // What the old check compared: identical, yet two different files.
    assert_eq!(ma.len(), mb.len());
    assert_eq!(ma.modified().unwrap(), mb.modified().unwrap());
    let (fa, fb) = (
        std::fs::File::open(&a).unwrap(),
        std::fs::File::open(&b).unwrap(),
    );
    assert!(!same_open_file(&fa, &fb));
    assert!(same_open_file(&fa, &std::fs::File::open(&a).unwrap()));
    let fp = final_path(&fa).unwrap().to_string_lossy().to_lowercase();
    assert!(fp.ends_with("a.txt"), "final path was {fp}");
}
