use super::*;

fn cli(
    id: &str,
    installed: bool,
    auth: cli_provider::AuthState,
    runnable: bool,
) -> cli_provider::DetectedCli {
    cli_provider::DetectedCli {
        id: id.to_owned(),
        display_name: id.to_uppercase(),
        installed,
        version: installed.then(|| "1.0.0".to_owned()),
        auth,
        account_label: None,
        runnable,
        detail: None,
        message: None,
    }
}

#[test]
fn table_rendering_is_unchanged_for_ok_warn_fail() {
    // Snapshot of the `bot doctor` table as it printed before the checks
    // moved into this module.
    let checks = vec![
        Check::ok("data_dir", "data dir", "/tmp/x (writable)"),
        Check::warn(
            "daemon",
            "daemon",
            "not running on port 1",
            "start it with `bot serve`",
        ),
        Check::fail(
            "providers",
            "providers",
            "alpha: API key not available",
            "set the key",
        ),
        Check::ok("mcp", "mcp config", "no MCP servers configured").hint("never shown"),
    ];
    let mut buf = Vec::new();
    render_table(&checks, &mut buf).unwrap();
    let expected = concat!(
        " \u{2713}  data dir    /tmp/x (writable)\n",
        " !  daemon      not running on port 1\n",
        "                fix: start it with `bot serve`\n",
        " \u{2717}  providers   alpha: API key not available\n",
        "                fix: set the key\n",
        " \u{2713}  mcp config  no MCP servers configured\n",
        "\n",
        "1 failed, 1 warning(s), 2 ok\n",
    );
    assert_eq!(String::from_utf8(buf).unwrap(), expected);
}

#[test]
fn skipped_checks_are_counted_separately_and_never_hinted() {
    let checks = vec![
        Check::ok("a", "a", "fine"),
        Check::skipped("b", "b", "not available on this platform").hint("hidden"),
    ];
    let mut buf = Vec::new();
    render_table(&checks, &mut buf).unwrap();
    let t = String::from_utf8(buf).unwrap();
    assert!(t.contains(" -  b  not available on this platform\n"), "{t}");
    assert!(!t.contains("fix:"), "{t}");
    assert!(
        t.ends_with("0 failed, 0 warning(s), 1 ok, 1 skipped\n"),
        "{t}"
    );
    let j = to_json(&checks);
    assert_eq!(j["ok"], true);
    assert_eq!(j["checks"][1]["status"], "skipped");
    assert_eq!(j["checks"][1]["name"], "b");
}

#[test]
fn repairs_are_a_closed_allow_list() {
    for r in Repair::ALL {
        assert_eq!(Repair::parse(r.id()), Some(r));
        assert_eq!(serde_json::to_value(r).unwrap(), r.id());
    }
    for bad in [
        "",
        "regenerate_api_token",
        "CREATE_DATA_DIR",
        "rm -rf /",
        "create_data_dir ",
    ] {
        assert_eq!(Repair::parse(bad), None, "{bad}");
    }
}

#[test]
fn missing_data_dir_offers_the_create_repair() {
    let d = tempfile::tempdir().unwrap();
    let missing = d.path().join("nope");
    let c = check_data_dir(&missing);
    assert_eq!(c.status, Status::Warn);
    assert_eq!(c.repair, Some(Repair::CreateDataDir));
    let c = check_data_dir(d.path());
    assert_eq!(c.status, Status::Ok, "{}", c.detail);
    assert_eq!(c.repair, None);
}

#[test]
fn unicode_probe_passes_in_a_non_ascii_temp_dir_and_cleans_up() {
    let d = tempfile::Builder::new()
        .prefix("praxi-çalışma-ğüşİ-")
        .tempdir()
        .unwrap();
    let c = check_unicode_path(d.path());
    assert_eq!(c.status, Status::Ok, "{}", c.detail);
    assert_eq!(
        std::fs::read_dir(d.path()).unwrap().count(),
        0,
        "probe file left behind"
    );
    let c = check_unicode_path(&d.path().join("missing"));
    assert_eq!(c.status, Status::Skipped);
}

#[test]
fn clock_check_flags_impossible_and_backwards_clocks() {
    let floor = SystemTime::UNIX_EPOCH + Duration::from_secs(CLOCK_FLOOR_SECS);
    let now = floor + Duration::from_secs(365 * 24 * 3600);
    assert_eq!(check_clock(now, None).status, Status::Ok);
    assert_eq!(
        check_clock(now, Some(now - Duration::from_secs(60))).status,
        Status::Ok
    );
    assert_eq!(
        check_clock(now, Some(now + Duration::from_secs(60))).status,
        Status::Ok
    );
    let c = check_clock(now, Some(now + Duration::from_secs(3600)));
    assert_eq!(c.status, Status::Warn);
    assert!(c.detail.contains("60 min"), "{}", c.detail);
    assert_eq!(
        check_clock(floor - Duration::from_secs(1), None).status,
        Status::Fail
    );
}

#[test]
fn disk_check_thresholds() {
    assert_eq!(disk_check(Ok(DISK_WARN_BYTES)).status, Status::Ok);
    assert_eq!(disk_check(Ok(DISK_WARN_BYTES - 1)).status, Status::Warn);
    assert_eq!(disk_check(Ok(DISK_FAIL_BYTES - 1)).status, Status::Fail);
    assert_eq!(disk_check(Err("denied".into())).status, Status::Warn);
    let d = tempfile::tempdir().unwrap();
    assert_ne!(check_disk(&d.path().join("a/b")).detail, "");
}

#[test]
fn unavailable_capabilities_are_neutral_never_failures() {
    let caps = [
        CapabilityStatus {
            id: "browser".into(),
            available: true,
            reason: None,
        },
        CapabilityStatus {
            id: "display".into(),
            available: false,
            reason: Some("not available on this platform".into()),
        },
    ];
    let c = check_capabilities(&caps);
    assert_eq!(c.status, Status::Ok);
    assert!(
        c.detail
            .contains("display (not available on this platform)"),
        "{}",
        c.detail
    );
    let none = [CapabilityStatus {
        id: "display".into(),
        available: false,
        reason: None,
    }];
    assert_eq!(check_capabilities(&none).status, Status::Skipped);
}

#[test]
fn desktop_automation_is_neutral_off_windows() {
    assert_eq!(
        check_desktop_automation("linux", None).status,
        Status::Skipped
    );
    assert_eq!(
        check_desktop_automation("macos", None).detail,
        "not available on this platform"
    );
    assert_eq!(
        check_desktop_automation("windows", None).status,
        Status::Warn
    );
    let caps = desktop_api::Capabilities {
        platform: "windows".into(),
        accessibility_tree: true,
        screenshot: true,
        app_launch: false,
        has_display: true,
    };
    let c = check_desktop_automation("windows", Some(&caps));
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.detail, "windows: accessibility tree, screenshots");
    let headless = desktop_api::Capabilities {
        has_display: false,
        ..caps
    };
    assert_eq!(
        check_desktop_automation("windows", Some(&headless)).status,
        Status::Skipped
    );
}

#[test]
fn peer_link_reflects_host_toml() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(check_peer_link(d.path()).status, Status::Skipped);
    std::fs::write(d.path().join("host.toml"), "mode = \"host\"\n").unwrap();
    assert_eq!(check_peer_link(d.path()).status, Status::Ok);
    std::fs::write(
        d.path().join("host.toml"),
        "mode = \"client\"\nstate = \"standby\"\n",
    )
    .unwrap();
    assert_eq!(check_peer_link(d.path()).status, Status::Warn);
    std::fs::write(d.path().join("host.toml"), "mode = \"client\"\n").unwrap();
    assert_eq!(check_peer_link(d.path()).status, Status::Skipped);
    assert_eq!(check_host(d.path()).status, Status::Fail);
}

#[test]
fn cli_accounts_from_one_detection_result() {
    use cli_provider::AuthState::{SignedIn, SignedOut, Unknown};
    assert_eq!(check_cli_accounts(None).status, Status::Skipped);
    assert_eq!(
        check_cli_accounts(Some(&[cli("claude", false, Unknown, false)])).status,
        Status::Skipped
    );
    let c = check_cli_accounts(Some(&[
        cli("claude", true, SignedIn, true),
        cli("codex", true, Unknown, false),
    ]));
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.detail, "ready: CLAUDE; detected only: CODEX");
    let c = check_cli_accounts(Some(&[cli("claude", true, SignedOut, true)]));
    assert_eq!(c.status, Status::Warn);
    assert!(c.detail.contains("signed out: CLAUDE"));
}

fn write_providers(dir: &Path, settings: Vec<ProviderSettings>) {
    let mut cfg = ProviderConfig::default();
    for s in settings {
        cfg.upsert(s);
    }
    cfg.save(&config_path(dir)).unwrap();
}

fn cli_settings() -> ProviderSettings {
    let mut s = ProviderSettings::new("cli:claude", 1);
    s.cli = Some(provider_catalog::config::CliSettings::Cli {
        engine: provider_catalog::config::CliEngine::Claude,
        command: "claude".into(),
    });
    s
}

#[test]
fn cli_provider_needs_a_signed_in_cli_not_an_api_key() {
    use cli_provider::AuthState::{SignedIn, SignedOut};
    let d = tempfile::tempdir().unwrap();
    write_providers(d.path(), vec![cli_settings()]);
    let cat = Catalog::from_entries([]);
    let store = secret_store::MemoryStore::new();
    let store_ref: StoreRef<'_> = Ok((&store, "Memory".into()));
    let run = |detected: Option<&[cli_provider::DetectedCli]>| {
        check_providers(&ProviderInputs {
            data_dir: d.path(),
            catalog: Ok(&cat),
            store: &store_ref,
            detected,
            online: None,
        })
    };
    let c = run(Some(&[cli("claude", true, SignedIn, true)]));
    assert_eq!(c.status, Status::Ok, "{}", c.detail);
    let c = run(Some(&[cli("claude", true, SignedOut, true)]));
    assert_eq!(c.status, Status::Fail);
    assert!(c.detail.contains("signed out"), "{}", c.detail);
    let c = run(Some(&[cli("claude", false, SignedOut, false)]));
    assert!(c.detail.contains("not installed"), "{}", c.detail);
    assert_eq!(run(None).status, Status::Ok);
}

#[test]
fn providers_on_disk_but_not_loaded_offer_a_reload() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        check_providers_loaded(d.path(), &[]).status,
        Status::Skipped
    );
    write_providers(d.path(), vec![cli_settings()]);
    let c = check_providers_loaded(d.path(), &[]);
    assert_eq!(c.status, Status::Warn);
    assert_eq!(c.repair, Some(Repair::ReloadProviders));
    let c = check_providers_loaded(d.path(), &["cli:claude".to_owned()]);
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.repair, None);
}

#[test]
fn redaction_masks_home_secrets_emails_and_tokens() {
    let mut r = Redactor::new();
    r.add_secret("hunter2-correct-horse");
    let home = r"C:\Users\Emre Öcel";
    let text = format!(
        "path C:\\Users\\Emre Öcel\\AppData\\praxi and c:/users/emre öcel/x; \
         key sk-0123456789abcdef0123456789; secret hunter2-correct-horse; \
         mail emre.ocel+bot@example.co.uk. token {} short-id",
        "z".repeat(40)
    );
    let out = redact_report(&text, Some(&r), Some(home));
    assert!(out.contains("path ~\\AppData\\praxi"), "{out}");
    for leaked in [
        "Emre",
        "Öcel",
        "öcel",
        "emre ",
        "0123456789abcdef",
        "hunter2",
        "example.co",
        "emre.ocel",
        "zzzz",
    ] {
        assert!(!out.contains(leaked), "{leaked} leaked: {out}");
    }
    assert!(out.contains("[email]."), "{out}");
    assert!(out.contains("short-id"), "{out}");
    assert_eq!(mask_emails("a@b"), "a@b");
    assert_eq!(mask_emails("@x.org and x@.org"), "@x.org and x@.org");
}

#[test]
fn report_lists_checks_and_config_names_only() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("providers.toml"), "secret = \"inside\"").unwrap();
    std::fs::write(d.path().join("api-token"), "t").unwrap();
    let info = ReportInfo {
        version: "1.2.3".into(),
        api_version: "1.0".into(),
        platform: "linux/x86_64".into(),
        data_dir: d.path().to_path_buf(),
        config_files: present_config_files(d.path()),
    };
    let md = report_markdown(
        &info,
        &[
            Check::fail("x", "x | y", "bad\nline", "fix it"),
            Check::skipped("s", "s", "n/a"),
        ],
    );
    assert!(md.contains("- Config files: providers.toml\n"), "{md}");
    assert!(!md.contains("inside") && !md.contains("api-token"), "{md}");
    assert!(
        md.contains("| fail | x \\| y | bad line (fix: fix it) |"),
        "{md}"
    );
    assert!(
        md.contains("1 failed, 0 warning(s), 0 ok, 1 skipped"),
        "{md}"
    );
}
