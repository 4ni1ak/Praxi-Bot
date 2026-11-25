//! Network isolation for the converters, and the syntactic path guard.
//!
//! DESIGN: vetting a hostile document can never be complete, so the control that really
//! matters is that the converter has no network. On Linux, LibreOffice and `pdftoppm` run
//! inside a network-less namespace: bubblewrap (`bwrap --unshare-net ...`, with a read-only
//! root, no home folders and a private /tmp) when present, else `unshare -rn` (unprivileged
//! user namespace) or `unshare -n` (root). One probe at first use decides which one works:
//! it runs the EXACT command line of a real conversion around a program that tries to
//! connect to a listener of this process, which must fail (see `probe_works`). The bwrap
//! command line also has `--new-session` and hides `/run` and `/var/run`, whose unix
//! sockets (Docker, D-Bus, systemd) a network namespace does not block.
//! When none does the tools FAIL CLOSED, unless the owner sets
//! `allow_unsandboxed_convert = true` (documented as unsafe for untrusted documents).
//!
//! Windows and macOS have no such sandbox in safe Rust. Conversion is OFF on Windows
//! unless the owner sets `convert_enabled_on_windows = true` (a converter there can leak
//! the daemon's NTLM hash over SMB: only enable it for trusted documents); the strict
//! document vetting applies either way. macOS is treated like "no sandbox".

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use tool_registry::ToolError;

use crate::DocToolsConfig;
use crate::common::{exec, invalid};

/// How a converter is wrapped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) enum Isolation {
    Bwrap(PathBuf),
    /// `unshare` with its arguments (`-rn` or `-n`).
    Unshare(PathBuf, &'static str),
}

pub(crate) const UNAVAILABLE: &str = "network isolation unavailable on this host: install bubblewrap (bwrap) or enable unprivileged user namespaces; the owner can set allow_unsandboxed_convert = true, which is UNSAFE for untrusted documents";

static PROBE: OnceLock<Option<Isolation>> = OnceLock::new();

/// Which isolation works here (probed once per process).
pub(crate) fn detect() -> Option<Isolation> {
    PROBE.get_or_init(probe).clone()
}

#[cfg(target_os = "linux")]
fn probe() -> Option<Isolation> {
    // Absolute, fixed locations only: a `PATH` lookup would trust whatever is writable.
    for dir in ["/usr/bin", "/bin", "/usr/local/bin"] {
        let bwrap = Path::new(dir).join("bwrap");
        if bwrap.is_file() {
            let iso = Isolation::Bwrap(bwrap);
            if probe_works(&iso) {
                return Some(iso);
            }
        }
    }
    for dir in ["/usr/bin", "/bin"] {
        let unshare = Path::new(dir).join("unshare");
        if unshare.is_file() {
            for flag in ["-rn", "-n"] {
                let iso = Isolation::Unshare(unshare.clone(), flag);
                if probe_works(&iso) {
                    return Some(iso);
                }
            }
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn probe() -> Option<Isolation> {
    None
}

/// The bubblewrap arguments of EVERY real conversion (and of the probe, which must
/// test exactly this): no network, own pid / ipc namespaces, a new session (no
/// controlling terminal, so no `TIOCSTI` injection), a read-only root, a private `/tmp`,
/// no home folders and no `/run` or `/var/run` (so the unix sockets of the Docker, D-Bus
/// or systemd daemons are unreachable: a unix socket is not blocked by a network
/// namespace). `work` is the only writable folder. The caller appends the converter.
pub(crate) fn bwrap_args(work: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = [
        "--unshare-net",
        "--unshare-pid",
        "--unshare-ipc",
        "--new-session",
        "--die-with-parent",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/home",
        "--tmpfs",
        "/root",
        "--tmpfs",
        "/run",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    // Usually `/var/run` is a link to `/run` and is hidden with it; a real folder there
    // (older systems) needs its own mask.
    if fs::symlink_metadata("/var/run").is_ok_and(|m| m.is_dir()) {
        args.extend(["--tmpfs".into(), "/var/run".into()]);
    }
    args.extend([
        "--bind".into(),
        work.into(),
        work.into(),
        "--chdir".into(),
        work.into(),
        "--".into(),
    ]);
    args
}

/// The command prefix for `isolation`; the caller appends the converter's arguments.
/// `work` is the only writable folder.
fn wrapped(isolation: &Isolation, exe: &Path, work: &Path) -> Command {
    match isolation {
        Isolation::Bwrap(bwrap) => {
            let mut c = Command::new(bwrap);
            c.args(bwrap_args(work)).arg(exe);
            c
        }
        Isolation::Unshare(unshare, flag) => {
            let mut c = Command::new(unshare);
            c.arg(flag).arg("--").arg(exe);
            c
        }
    }
}

// ---------------------------------------------------------------- probe

/// What the probe program prints when its connect attempt failed / succeeded.
#[cfg(target_os = "linux")]
const NO_CONNECT: &str = "PRAXI-NOCONNECT";
#[cfg(target_os = "linux")]
const CONNECTED: &str = "PRAXI-CONNECTED";

/// Programs that can attempt a TCP connect, as (absolute path, flag, script). Each
/// prints [`CONNECTED`] or [`NO_CONNECT`].
#[cfg(target_os = "linux")]
fn connect_probes(port: u16) -> Vec<(PathBuf, &'static str, String)> {
    let mut v = Vec::new();
    for dir in ["/usr/bin", "/bin", "/usr/local/bin"] {
        let d = Path::new(dir);
        v.push((
            d.join("python3"),
            "-c",
            format!(
                "import socket\ntry:\n s=socket.create_connection(('127.0.0.1',{port}),3)\n print('{CONNECTED}')\nexcept OSError:\n print('{NO_CONNECT}')"
            ),
        ));
        v.push((
            d.join("perl"),
            "-e",
            format!(
                "use IO::Socket::INET; my $s=IO::Socket::INET->new(PeerAddr=>'127.0.0.1:{port}',Timeout=>3); print($s ? '{CONNECTED}' : '{NO_CONNECT}');"
            ),
        ));
        v.push((
            d.join("bash"),
            "-c",
            format!(
                "if (exec 3<>/dev/tcp/127.0.0.1/{port}) 2>/dev/null; then echo {CONNECTED}; else echo {NO_CONNECT}; fi"
            ),
        ));
    }
    v.retain(|(p, _, _)| p.is_file());
    v
}

/// Run `cmd` (stdin closed, stderr dropped) for at most 20 s; its stdout, or `None`
/// when it could not start, failed, timed out or was killed.
#[cfg(target_os = "linux")]
fn run_capture(mut cmd: Command) -> Option<String> {
    use std::io::Read as _;
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return status.success().then_some(out);
            }
            Ok(None) if started.elapsed() < std::time::Duration::from_secs(20) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Does `isolation` really cut the network? It runs the EXACT command line of a real
/// conversion (`wrapped`, the same flags) around a probe program that tries to open a
/// TCP connection to a listener THIS process owns on 127.0.0.1. Control run first:
/// unsandboxed, the program must connect (so a failure inside the sandbox means "no
/// network", not "broken probe"). Inside the sandbox the connect must fail AND the
/// listener must never see a connection. Anything else (no probe program, a timeout,
/// a connection reaching us) means "not isolated".
#[cfg(target_os = "linux")]
pub(crate) fn probe_works(isolation: &Isolation) -> bool {
    use std::net::TcpListener;
    let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) else {
        return false;
    };
    let Ok(port) = listener.local_addr().map(|a| a.port()) else {
        return false;
    };
    if listener.set_nonblocking(true).is_err() {
        return false;
    }
    let Ok(work) = tempfile::Builder::new().prefix("praxi-probe-").tempdir() else {
        return false;
    };
    let accepted = |l: &TcpListener| {
        let mut n = 0;
        while l.accept().is_ok() {
            n += 1;
        }
        n
    };
    for (exe, flag, script) in connect_probes(port) {
        // Control: no sandbox, the connection must arrive.
        let mut plain = Command::new(&exe);
        plain.arg(flag).arg(&script);
        let control = run_capture(plain).is_some_and(|o| o.trim() == CONNECTED);
        if !control || accepted(&listener) == 0 {
            continue; // this program cannot tell us anything on this host
        }
        let mut boxed = wrapped(isolation, &exe, work.path());
        boxed.arg(flag).arg(&script);
        let out = run_capture(boxed);
        let leaked = accepted(&listener);
        return leaked == 0 && out.is_some_and(|o| o.trim() == NO_CONNECT);
    }
    false
}

/// A command that runs `exe` with no network, or the plain command when the owner
/// explicitly allowed unsandboxed conversion. Fails closed otherwise.
pub(crate) fn command(cfg: &DocToolsConfig, exe: &Path, work: &Path) -> Result<Command, ToolError> {
    #[cfg(test)]
    if FORCE_NONE.with(std::cell::Cell::get) {
        return command_with(cfg, None, exe, work);
    }
    command_with(cfg, detect(), exe, work)
}

#[cfg(test)]
thread_local! {
    /// Tests of the converter plumbing run unsandboxed (their fakes live in /tmp); the
    /// sandbox tests switch the real probe back on for their own thread.
    static FORCE_NONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn force_no_sandbox(on: bool) {
    FORCE_NONE.with(|c| c.set(on));
}

pub(crate) fn command_with(
    cfg: &DocToolsConfig,
    isolation: Option<Isolation>,
    exe: &Path,
    work: &Path,
) -> Result<Command, ToolError> {
    // Windows: the owner switch comes first; there is no sandbox to probe.
    if cfg!(windows) {
        return Ok(Command::new(exe));
    }
    match isolation {
        Some(i) => Ok(wrapped(&i, exe, work)),
        None if cfg.convert.allow_unsandboxed_convert => Ok(Command::new(exe)),
        None => Err(exec(UNAVAILABLE)),
    }
}

/// Windows is opt-in: refuse unless the owner enabled it.
pub(crate) fn platform_gate(cfg: &DocToolsConfig) -> Result<(), ToolError> {
    if cfg!(windows) && !cfg.convert.convert_enabled_on_windows {
        return Err(exec(
            "document conversion is switched off on Windows (there is no network sandbox here); the owner can set convert_enabled_on_windows = true for trusted documents only",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- path guard

/// Reject, by text alone (NO filesystem call, so nothing touches the network), any path
/// that could name a remote or device location. RULE: a UNC path (`\\host\share`,
/// `//host/share`), a device / extended path (`\\?\`, `\\.\`, `\??\`), an alternate data
/// stream or drive-relative form (a `:` anywhere but right after a drive letter) and
/// control characters. Stops the SMB authentication / NTLM leak that a `stat` on a UNC
/// path would trigger before any other guard runs.
pub(crate) fn unsafe_path_reason(path: &str, windows: bool) -> Option<&'static str> {
    let b = path.as_bytes();
    let slash = |c: u8| c == b'\\' || c == b'/';
    if path.trim().is_empty() {
        return Some("the path is empty");
    }
    if path.chars().any(char::is_control) {
        return Some("the path contains control characters");
    }
    if b.len() >= 2 && slash(b[0]) && slash(b[1]) {
        return Some("network (UNC) paths are not allowed");
    }
    let norm = path.replace('/', "\\");
    if norm.contains("\\\\?\\") || norm.contains("\\\\.\\") || norm.contains("\\??\\") {
        return Some("device paths are not allowed");
    }
    if windows {
        let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
        if drive && !(b.len() >= 3 && slash(b[2])) {
            return Some("give the full path including the folder after the drive");
        }
        let rest = if drive { &path[2..] } else { path };
        if rest.contains(':') {
            return Some("paths with ':' (streams, devices) are not allowed");
        }
        if !drive && slash(b[0]) {
            return Some("give a full local path starting with a drive letter");
        }
        if path.split(['\\', '/']).any(reserved_device_name) {
            return Some("Windows device names (CON, NUL, COM1 ...) are not allowed");
        }
    }
    None
}

/// Whether one path component names a Windows DOS device: `CON`, `PRN`, `AUX`, `NUL`,
/// `COM0`-`COM9` / `LPT0`-`LPT9` (also the superscript digits Windows maps to them),
/// `CONIN$`, `CONOUT$`. Windows ignores the extension (`CON.docx`, `nul.tar.gz`), the
/// case and trailing spaces before the dot (`NUL .txt`), so all of those match.
pub(crate) fn reserved_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '\t']);
    let up = stem.to_uppercase();
    if matches!(
        up.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let mut chars = up.chars();
    let head: String = chars.by_ref().take(3).collect();
    matches!(head.as_str(), "COM" | "LPT")
        && chars
            .next()
            .is_some_and(|d| matches!(d, '0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'))
        && chars.next().is_none()
}

/// Apply [`unsafe_path_reason`] for this platform.
pub(crate) fn guard_path(path: &str) -> Result<(), ToolError> {
    match unsafe_path_reason(path, cfg!(windows)) {
        Some(why) => Err(invalid(format!("{why}: {path}"))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_no_unc_device_or_stream_paths_by_text() {
        for bad in [
            r"\\attacker\share\a.docx",
            "//attacker/share/a.docx",
            r"\\?\UNC\attacker\share\a.docx",
            r"\\.\pipe\x.docx",
            r"C:\x\\?\a.docx",
            r"\??\UNC\a\b.docx",
            r"C:\docs\a.docx:evil",
            r"C:\docs\CON:a.docx",
            "C:a.docx",
            r"\rooted\a.docx",
            "",
        ] {
            assert!(unsafe_path_reason(bad, true).is_some(), "windows: {bad:?}");
        }
        // These are refused on every platform, before any filesystem call.
        for bad in [r"\\h\s\a.docx", "//h/s/a.docx", r"/x/\\?\y.docx"] {
            assert!(unsafe_path_reason(bad, false).is_some(), "unix: {bad:?}");
        }
        for ok in [r"C:\Users\me\a.docx", "C:/Users/me/a.docx", "offer.docx"] {
            assert!(unsafe_path_reason(ok, true).is_none(), "{ok}");
        }
        for ok in ["/home/me/a.docx", "/tmp/a:b.docx", "docs/a.docx"] {
            assert!(unsafe_path_reason(ok, false).is_none(), "{ok}");
        }
    }

    #[test]
    fn rule_windows_reserved_device_names_are_refused_with_or_without_extension() {
        for bad in [
            r"C:\docs\CON.docx",
            r"C:\docs\nul",
            r"C:\docs\Com1.docx",
            r"C:\docs\LPT9.tar.gz",
            r"C:\docs\aux .docx",
            "C:\\docs\\COM\u{b9}.docx",
            "C:/docs/PRN.docx",
            r"C:\docs\CON\a.docx",
            r"C:\docs\CONOUT$.docx",
        ] {
            assert!(
                unsafe_path_reason(bad, true).is_some_and(|why| why.contains("device names")),
                "reserved: {bad:?}"
            );
        }
        for ok in [
            r"C:\docs\console.docx",
            r"C:\docs\com10.docx",
            r"C:\docs\nullable.docx",
            r"C:\docs\a.con",
            r"C:\docs\my.nul.docx",
            r"C:\docs\COM.docx",
        ] {
            assert!(unsafe_path_reason(ok, true).is_none(), "{ok}");
        }
        // The rule belongs to Windows paths: a Unix host may have a file called con.docx.
        assert!(unsafe_path_reason("/home/me/con.docx", false).is_none());
    }

    #[test]
    fn rule_the_sandbox_has_a_new_session_and_no_run_folder() {
        let args: Vec<String> = bwrap_args(Path::new("/work"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let pos = |flag: &str| args.iter().position(|a| a == flag);
        assert!(pos("--new-session").is_some(), "{args:?}");
        assert!(pos("--unshare-net").is_some(), "{args:?}");
        let run = args.windows(2).any(|w| w[0] == "--tmpfs" && w[1] == "/run");
        assert!(run, "/run must be hidden: {args:?}");
        // The writable folder comes after the masks, and the command follows `--`.
        assert!(pos("--bind") > pos("--tmpfs"), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some("--"));
    }

    /// The probe runs the same command line as a real conversion and a real connect
    /// attempt: a "sandbox" that does not cut the network must be rejected. (No probe
    /// program on the host: nothing to prove, the test skips.)
    #[cfg(target_os = "linux")]
    #[test]
    fn rule_the_probe_rejects_an_isolation_that_leaves_the_network_open() {
        if !["/usr/bin", "/bin"].iter().any(|d| {
            ["python3", "perl", "bash"]
                .iter()
                .any(|p| Path::new(d).join(p).is_file())
        }) || !Path::new("/usr/bin/env").is_file()
        {
            eprintln!("skipped: no probe program or env on this host");
            return;
        }
        // `env -i -- <exe> ...` runs the probe program with no isolation at all.
        let open = Isolation::Unshare(PathBuf::from("/usr/bin/env"), "-i");
        assert!(!probe_works(&open), "an open network passed the probe");
        // A missing wrapper can never pass.
        let missing = Isolation::Bwrap(PathBuf::from("/nonexistent/bwrap"));
        assert!(!probe_works(&missing));
    }

    /// With a working sandbox: the exact conversion flags leave `/run` empty and the
    /// network down. Skips when this host has no usable sandbox.
    #[cfg(target_os = "linux")]
    #[test]
    fn rule_a_working_sandbox_hides_run_and_passes_its_own_probe() {
        let Some(iso @ Isolation::Bwrap(_)) = detect() else {
            eprintln!("skipped: no bubblewrap sandbox on this host");
            return;
        };
        assert!(probe_works(&iso));
        let work = tempfile::tempdir().unwrap();
        let mut cmd = wrapped(&iso, Path::new("/bin/ls"), work.path());
        cmd.args(["-A", "/run"]);
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(out.stdout.is_empty(), "/run is not empty: {out:?}");
    }

    #[cfg(not(windows))]
    #[test]
    fn rule_fail_closed_without_a_sandbox_unless_the_owner_allows_it() {
        let mut cfg = DocToolsConfig::default();
        let err = command_with(&cfg, None, Path::new("/bin/true"), Path::new("/tmp")).unwrap_err();
        assert!(
            err.to_string().contains("network isolation unavailable"),
            "{err}"
        );
        cfg.convert.allow_unsandboxed_convert = true;
        assert!(command_with(&cfg, None, Path::new("/bin/true"), Path::new("/tmp")).is_ok());
        // With a sandbox available it is used, whatever the flag says.
        let iso = Isolation::Unshare(PathBuf::from("/usr/bin/unshare"), "-rn");
        let c = command_with(&cfg, Some(iso), Path::new("/bin/true"), Path::new("/tmp")).unwrap();
        assert!(c.get_program().to_string_lossy().ends_with("unshare"));
    }

    #[cfg(windows)]
    #[test]
    fn rule_windows_conversion_is_off_unless_the_owner_enables_it() {
        let mut cfg = DocToolsConfig::default();
        let err = platform_gate(&cfg).unwrap_err();
        assert!(
            err.to_string().contains("convert_enabled_on_windows"),
            "{err}"
        );
        cfg.convert.convert_enabled_on_windows = true;
        assert!(platform_gate(&cfg).is_ok());
    }
}
