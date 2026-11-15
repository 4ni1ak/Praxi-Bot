//! Engine behaviour against a fake `Transport` (no ssh, no network).
use std::{cell::RefCell, collections::HashMap, time::Duration};

use super::*;
use crate::known_hosts::ScannedKey;

const ED: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

fn good_facts() -> String {
    "uid=1001\nos_id=ubuntu\nos_version=24.04\nos_like=debian\nos_name=Ubuntu\narch=aarch64\nsystemd=1\nsudo_n=1\nram_mb=24000\ndisk_mb=150000\ncurl=1\nhttps=1\ngh_version=\npraxi_version=\nunit_present=0\nunit_hostmode=0\nunit_active=inactive\ndata_dir=0\nport=47821\nport_busy=0\n".into()
}

fn installed_facts() -> String {
    good_facts()
        .replace("praxi_version=", "praxi_version=0.1.0")
        .replace("unit_present=0", "unit_present=1")
        .replace("unit_hostmode=0", "unit_hostmode=1")
        .replace("unit_active=inactive", "unit_active=active")
        .replace("data_dir=0", "data_dir=1")
}

struct Fake {
    keys: Vec<ScannedKey>,
    preflight: RefCell<String>,
    fail: RefCell<HashMap<&'static str, i32>>,
    desktops: RefCell<Vec<String>>,
    calls: RefCell<Vec<(RemoteCmd, String)>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            keys: vec![ScannedKey { key_type: "ssh-ed25519".into(), key_b64: ED.into() }],
            preflight: RefCell::new(good_facts()),
            fail: RefCell::new(HashMap::new()),
            desktops: RefCell::new(Vec::new()),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn ran(&self, cmd: RemoteCmd) -> bool {
        self.calls.borrow().iter().any(|(c, _)| *c == cmd)
    }
}

impl Transport for Fake {
    fn scan_host_keys(&self) -> Result<Vec<ScannedKey>, TransportError> {
        Ok(self.keys.clone())
    }

    fn run(&self, cmd: RemoteCmd, stdin: &[u8], _t: Duration) -> Result<Output, TransportError> {
        self.calls.borrow_mut().push((cmd, String::from_utf8_lossy(stdin).into_owned()));
        let name = match cmd {
            RemoteCmd::Provision(Sub::Install) => "install",
            RemoteCmd::Provision(Sub::Status) => "status",
            _ => "",
        };
        if let Some(code) = self.fail.borrow().get(name) {
            return Ok(Output { code: *code, stderr: "boom".into(), ..Output::default() });
        }
        let stdout = match cmd {
            RemoteCmd::Probe => "praxi-ok\n".to_owned(),
            RemoteCmd::Provision(Sub::Preflight) => self.preflight.borrow().clone(),
            RemoteCmd::Provision(Sub::Status) => "version=0.1.0\nunit_active=active\nunit_hostmode=1\nxvfb=active\ndesktop=active\n".into(),
            RemoteCmd::Provision(Sub::Devices) => {
                let peers: Vec<_> = self.desktops.borrow().iter().map(|k| serde_json::json!({"key": k, "role": "desktop"})).collect();
                serde_json::json!({ "peers": peers }).to_string()
            }
            RemoteCmd::Provision(Sub::Invite) => {
                // the pairing consumes the ticket: our desktop key appears on the host
                self.desktops.borrow_mut().push("deskkey1".into());
                r#"{"ticket":"TICKET","code":"123456"}"#.into()
            }
            _ => String::new(),
        };
        Ok(Output { code: 0, stdout, ..Output::default() })
    }
}

#[derive(Default)]
struct Local {
    paired: RefCell<bool>,
}

impl LocalHost for Local {
    fn pair(&self, ticket: &str) -> Result<PairedHost, String> {
        assert_eq!(ticket, "TICKET");
        *self.paired.borrow_mut() = true;
        Ok(PairedHost { id: "hostid1".into(), name: "oracle".into() })
    }
    fn knows_host(&self, id: &str) -> bool {
        *self.paired.borrow() && id == "hostid1"
    }
    fn health(&self, _id: &str) -> Result<HostHealth, String> {
        Ok(HostHealth { mode: "host".into(), version: "0.1.0".into(), has_display: true, has_browser: true, view_targets: Some(1) })
    }
    fn revoke_host(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
struct TestUi {
    accept: bool,
    asked: usize,
}

impl Ui for TestUi {
    fn progress(&mut self, _r: &StepResult) {}
    fn note(&mut self, _l: &str) {}
    fn confirm_host_key(&mut self, _h: &str, _p: u16, _fp: &str) -> bool {
        self.asked += 1;
        self.accept
    }
}

fn record() -> HostRecord {
    HostRecord { name: "oracle".into(), user: "ubuntu".into(), host: "203.0.113.7".into(), port: 22, key_path: None, fingerprint: None, paired_host_id: None, desktop_key: None, installed_version: None }
}

fn run(dir: &std::path::Path, fake: &Fake, local: &Local, ui: &mut TestUi, mode: Mode) -> Report {
    let store = Store::new(dir.join("hosts"));
    let kh = KnownHosts::new(store.known_hosts_path());
    let record = store.load("oracle").unwrap_or_else(record);
    let mut engine = Engine {
        transport: fake,
        local,
        ui,
        store: &store,
        known_hosts: &kh,
        params: Params { record, mode, app_version: "0.1.0".into(), desktop: true, retry_delay: Duration::ZERO },
    };
    engine.run()
}

fn status_of(r: &Report, id: &str) -> Status {
    r.results.iter().find(|s| s.id == id).unwrap_or_else(|| panic!("no step {id}")).status
}

#[test]
fn happy_path_installs_pairs_and_checks_health() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, local, mut ui) = (Fake::new(), Local::default(), TestUi { accept: true, asked: 0 });
    let r = run(dir.path(), &fake, &local, &mut ui, Mode::Add);
    assert!(r.ok(), "{}", r.table());
    for id in ["hostkey", "connect", "install.run", "pair", "health.version", "health.live_view"] {
        assert_eq!(status_of(&r, id), Status::Pass, "{id}");
    }
    let calls = fake.calls.borrow();
    let install = calls.iter().find(|(c, _)| *c == RemoteCmd::Provision(Sub::Install)).unwrap();
    assert_eq!(install.1, "version=v0.1.0\ndesktop=1\n", "parameters go on stdin, not in a command");
    let screen = calls.iter().find(|(c, _)| *c == RemoteCmd::Provision(Sub::Screen)).unwrap();
    assert_eq!(screen.1, "key=deskkey1\n");
    // the ticket never reaches the log file
    let log = std::fs::read_to_string(dir.path().join("hosts/oracle/provision.log")).unwrap();
    assert!(!log.contains("TICKET") && !log.contains("123456"));
}

#[test]
fn failing_preflight_stops_before_anything_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, local, mut ui) = (Fake::new(), Local::default(), TestUi { accept: true, asked: 0 });
    *fake.preflight.borrow_mut() = good_facts().replace("ram_mb=24000", "ram_mb=900");
    let r = run(dir.path(), &fake, &local, &mut ui, Mode::Add);
    assert!(!r.ok());
    assert_eq!(status_of(&r, "preflight.ram"), Status::Fail);
    assert!(!fake.ran(RemoteCmd::Provision(Sub::Install)) && !fake.ran(RemoteCmd::Stage(StageFile::Installer)));
}

#[test]
fn host_key_mismatch_is_a_hard_stop_that_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("hosts"));
    let kh = KnownHosts::new(store.known_hosts_path());
    let other = ScannedKey { key_type: "ssh-ed25519".into(), key_b64: "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into() };
    kh.pin("203.0.113.7", 22, &other).unwrap();
    let (fake, local, mut ui) = (Fake::new(), Local::default(), TestUi { accept: true, asked: 0 });
    let r = run(dir.path(), &fake, &local, &mut ui, Mode::Add);
    assert!(r.hard_stop && !r.ok());
    assert_eq!(ui.asked, 0, "a mismatch never offers to accept");
    assert!(fake.calls.borrow().iter().all(|(c, _)| *c == RemoteCmd::Cleanup), "no command ran on the server");
}

#[test]
fn plain_serve_install_is_refused_by_add_and_migrated_by_repair() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, local, mut ui) = (Fake::new(), Local::default(), TestUi { accept: true, asked: 0 });
    *fake.preflight.borrow_mut() = installed_facts().replace("unit_hostmode=1", "unit_hostmode=0");
    let r = run(dir.path(), &fake, &local, &mut ui, Mode::Add);
    assert_eq!(status_of(&r, "install"), Status::Fail);
    assert!(!fake.ran(RemoteCmd::Provision(Sub::Install)));
    let r = run(dir.path(), &fake, &local, &mut ui, Mode::Repair);
    assert!(fake.ran(RemoteCmd::Provision(Sub::Snapshot)), "data is snapshotted first");
    assert_eq!(status_of(&r, "install.run"), Status::Pass, "{}", r.table());
}

#[test]
fn resume_after_a_failed_step_skips_what_is_already_done() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, local, mut ui) = (Fake::new(), Local::default(), TestUi { accept: true, asked: 0 });
    fake.fail.borrow_mut().insert("install", 1);
    let r = run(dir.path(), &fake, &local, &mut ui, Mode::Add);
    assert_eq!(status_of(&r, "install.run"), Status::Fail);
    assert!(r.results.iter().all(|s| s.id != "pair"), "pairing waits for a good install");

    fake.fail.borrow_mut().clear();
    let r = run(dir.path(), &fake, &local, &mut ui, Mode::Add);
    assert!(r.ok(), "{}", r.table());
    assert_eq!(ui.asked, 1, "the host key was pinned once and trusted afterwards");

    // the install now exists on the server: a third run installs and pairs nothing again
    *fake.preflight.borrow_mut() = installed_facts();
    let installs = fake.calls.borrow().iter().filter(|(c, _)| *c == RemoteCmd::Provision(Sub::Install)).count();
    let r = run(dir.path(), &fake, &local, &mut ui, Mode::Add);
    assert_eq!(status_of(&r, "install"), Status::Skip);
    assert_eq!(status_of(&r, "pair"), Status::Skip);
    let again = fake.calls.borrow().iter().filter(|(c, _)| *c == RemoteCmd::Provision(Sub::Install)).count();
    assert_eq!(installs, again);
}
