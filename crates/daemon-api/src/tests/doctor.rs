//! `/v1/doctor*`: auth, read-only checks, allow-listed repairs, redaction.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::{Method, StatusCode};
use secret_store::SecretStore;

use super::{T, call, send, setup};
use crate::doctor::{Check, Status};
use crate::doctor_routes::{DoctorHost, Job, run_jobs};

fn audit_actions(t: &T) -> Vec<(String, serde_json::Value)> {
    t.state
        .storage()
        .list_audit(t.state.user_id())
        .unwrap()
        .into_iter()
        .filter(|a| a.action == "doctor.repair")
        .map(|a| (a.action, a.detail))
        .collect()
}

#[tokio::test]
async fn every_doctor_route_needs_the_token() {
    let t = setup();
    for (m, p) in [
        (Method::GET, "/v1/doctor"),
        (Method::GET, "/v1/doctor/report"),
        (Method::POST, "/v1/doctor/repair/create_data_dir"),
    ] {
        let r = send(&t.app, m.clone(), p, None, &[]).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{m} {p}");
        let r = send(
            &t.app,
            m.clone(),
            p,
            None,
            &[("authorization", "Bearer wrong")],
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{m} {p}");
    }
    assert_eq!(audit_actions(&t), []);
}

#[tokio::test]
async fn doctor_lists_typed_checks_without_side_effects() {
    let t = setup();
    let dir = t.state.data_dir();
    let before: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let r = call(&t, Method::GET, "/v1/doctor", None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    let v = r.json();
    let checks = v["checks"].as_array().unwrap();
    let ids: Vec<&str> = checks.iter().map(|c| c["id"].as_str().unwrap()).collect();
    for want in [
        "daemon",
        "host_ownership",
        "peer_link",
        "data_dir",
        "unicode_path",
        "config",
        "providers",
        "providers_loaded",
        "cli_accounts",
        "browser",
        "desktop_automation",
        "capabilities",
        "schedule_tools",
        "disk_space",
        "clock",
    ] {
        assert!(ids.contains(&want), "{want} missing from {ids:?}");
    }
    let get = |id: &str| checks.iter().find(|c| c["id"] == id).unwrap();
    assert_eq!(get("data_dir")["status"], "ok");
    assert_eq!(
        get("unicode_path")["status"],
        "ok",
        "{}",
        get("unicode_path")
    );
    // Sealed PATH in the test setup: no coding CLI installed, which is neutral.
    assert_eq!(get("cli_accounts")["status"], "skipped");
    assert_eq!(get("peer_link")["status"], "skipped");
    // No embedder host: no browser restart offered, desktop not "failed".
    assert!(get("browser")["repair"].is_null());
    assert_ne!(get("desktop_automation")["status"], "fail");
    for c in checks {
        assert!(c["title"].is_string() && c["detail"].is_string(), "{c}");
        assert!(
            ["ok", "warn", "fail", "skipped"].contains(&c["status"].as_str().unwrap()),
            "{c}"
        );
    }
    let after: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        before.len(),
        after.len(),
        "probe files left behind: {after:?}"
    );
}

#[tokio::test]
async fn slow_checks_are_cut_off_without_holding_the_request() {
    let start = tokio::time::Instant::now();
    let jobs: Vec<Job> = vec![
        (
            "fast",
            "fast",
            Box::new(|| Check::ok("fast", "fast", "done")),
        ),
        (
            "slow",
            "slow",
            Box::new(|| {
                std::thread::sleep(Duration::from_millis(800));
                Check::ok("slow", "slow", "late")
            }),
        ),
        ("panics", "panics", Box::new(|| panic!("boom"))),
    ];
    let out = run_jobs(
        jobs,
        Duration::from_millis(100),
        start + Duration::from_secs(5),
    )
    .await;
    assert!(
        start.elapsed() < Duration::from_millis(700),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(
        out.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["fast", "slow", "panics"]
    );
    assert_eq!(out[0].status, Status::Ok);
    assert_eq!(out[1].status, Status::Warn);
    assert!(out[1].detail.contains("in time"));
    assert_eq!(out[2].status, Status::Warn);
    // The overall deadline wins over a longer per-check budget.
    let start = tokio::time::Instant::now();
    let jobs: Vec<Job> = vec![(
        "slow",
        "slow",
        Box::new(|| {
            std::thread::sleep(Duration::from_millis(800));
            Check::ok("slow", "slow", "late")
        }),
    )];
    let out = run_jobs(
        jobs,
        Duration::from_secs(3),
        start + Duration::from_millis(100),
    )
    .await;
    assert_eq!(out[0].status, Status::Warn);
    assert!(start.elapsed() < Duration::from_millis(700));
}

#[tokio::test]
async fn unknown_repairs_are_rejected_and_never_run() {
    let t = setup();
    for id in [
        "regenerate_api_token",
        "CREATE_DATA_DIR",
        "shell",
        "..%2F..%2Fx",
    ] {
        let r = call(&t, Method::POST, &format!("/v1/doctor/repair/{id}"), None).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{id}: {:?}", r.json());
    }
    assert_eq!(audit_actions(&t), []);
    // GET is not a way to run a repair either.
    let r = call(&t, Method::GET, "/v1/doctor/repair/create_data_dir", None).await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn allow_listed_repairs_run_and_are_audited() {
    let t = setup();
    let r = call(&t, Method::POST, "/v1/doctor/repair/create_data_dir", None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["repair"], "create_data_dir");

    // No provider configuration in this daemon: refused, still audited.
    let r = call(&t, Method::POST, "/v1/doctor/repair/reload_providers", None).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{:?}", r.json());
    // No embedder host: the browser cannot be restarted.
    let r = call(&t, Method::POST, "/v1/doctor/repair/restart_browser", None).await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    let audit = audit_actions(&t);
    let got: Vec<(String, bool)> = audit
        .iter()
        .map(|(_, d)| {
            (
                d["repair"].as_str().unwrap().to_owned(),
                d["ok"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("create_data_dir".to_owned(), true),
            ("reload_providers".to_owned(), false),
            ("restart_browser".to_owned(), false),
        ]
    );
}

#[derive(Default)]
struct FakeHost {
    restarts: AtomicUsize,
}

#[async_trait::async_trait]
impl DoctorHost for FakeHost {
    fn desktop(&self) -> Option<desktop_api::Capabilities> {
        Some(desktop_api::Capabilities {
            platform: "windows".into(),
            accessibility_tree: true,
            screenshot: true,
            app_launch: true,
            has_display: true,
        })
    }
    fn can_restart_browser(&self) -> bool {
        true
    }
    async fn restart_browser(&self) -> Result<String, String> {
        self.restarts.fetch_add(1, Ordering::SeqCst);
        Ok("browser restarted".into())
    }
}

#[tokio::test]
async fn embedder_host_adds_desktop_facts_and_the_browser_restart() {
    let t = setup();
    let host = Arc::new(FakeHost::default());
    t.state.set_doctor_host(host.clone());
    let r = call(&t, Method::GET, "/v1/doctor", None).await;
    let v = r.json();
    let desk = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "desktop_automation")
        .unwrap()
        .clone();
    assert_eq!(desk["status"], "ok", "{desk}");
    // Reading the diagnostics never runs a repair.
    assert_eq!(host.restarts.load(Ordering::SeqCst), 0);
    let r = call(&t, Method::POST, "/v1/doctor/repair/restart_browser", None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["detail"], "browser restarted");
    assert_eq!(host.restarts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reload_repair_uses_the_provider_host() {
    let t = setup();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let data_dir = t.state.data_dir();
    t.state.set_provider_host(crate::ProviderHost {
        catalog: provider_catalog::Catalog::default(),
        reload: Box::new(move || {
            h.fetch_add(1, Ordering::SeqCst);
            agent_core::build_providers(
                &data_dir,
                &provider_catalog::Catalog::default(),
                &secret_store::MemoryStore::new(),
                &mut |_| {},
            )
        }),
    });
    let _ = call(&t, Method::POST, "/v1/doctor/repair/reload_providers", None).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(audit_actions(&t).len(), 1);
}

const PLANTED_KEY: &str = "pk.planted.7f3a";
const PLANTED_SHAPED: &str = "sk-proj-abcdefghijklmnopqrstuvwxyz012345";

#[tokio::test]
async fn report_is_markdown_and_carries_no_planted_secret_or_home_path() {
    let t = setup();
    let store = Arc::new(secret_store::MemoryStore::new());
    store.set("provider.alpha.api_key", PLANTED_KEY).unwrap();
    t.state.set_secret_store(store);
    let dir = t.state.data_dir();
    // A broken config whose parse error quotes the secrets and an address.
    std::fs::write(
        dir.join("providers.toml"),
        format!("key = {PLANTED_KEY} {PLANTED_SHAPED} owner@example.com\n"),
    )
    .unwrap();

    let r = call(&t, Method::GET, "/v1/doctor/report", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/markdown"),
        "{:?}",
        r.headers
    );
    let text = String::from_utf8(r.body.clone()).unwrap();
    assert!(text.starts_with("# Praxi diagnostics report"), "{text}");
    assert!(text.contains("providers.toml"), "{text}");
    for leaked in [
        PLANTED_KEY,
        PLANTED_SHAPED,
        "owner@example.com",
        super::TOKEN,
    ] {
        assert!(!text.contains(leaked), "{leaked} leaked:\n{text}");
    }

    // The home directory is replaced by `~` (planted: the data dir's parent).
    let home = dir.parent().unwrap().to_string_lossy().into_owned();
    let text = crate::doctor_routes::build_report(&t.state, Some(home.clone()))
        .await
        .unwrap();
    assert!(!text.contains(&home), "{home} leaked:\n{text}");
    assert!(
        !text.contains(&home.replace('\\', "/")),
        "{home} leaked:\n{text}"
    );
    assert!(text.contains("`~"), "{text}");
}
