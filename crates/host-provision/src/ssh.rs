//! The real transport: the system OpenSSH client with a fixed argv.
//!
//! Key file or ssh-agent only (BatchMode); passwords and passphrase prompts
//! are out of scope for this PR. No user text reaches a shell: ssh gets one
//! argv element per value and the remote command is a constant.
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use crate::{
    Error,
    known_hosts::{ScannedKey, parse_scan},
    transport::{Output, RemoteCmd, Transport, TransportError, sanitize},
};

const STDOUT_CAP: usize = 256 * 1024;
const STDERR_CAP: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct SshTarget {
    pub user: String,
    pub host: String,
    pub port: u16,
    pub key: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SshPaths {
    pub ssh: PathBuf,
    pub keyscan: PathBuf,
    /// Praxi's own known_hosts.
    pub known_hosts: PathBuf,
    /// An (empty) ssh_config so `~/.ssh/config` is never read.
    pub ssh_config: PathBuf,
}

/// Find `ssh` / `ssh-keyscan` as absolute paths (Windows: System32\OpenSSH
/// first, then PATH; elsewhere PATH).
pub fn resolve_binary(name: &str) -> Result<PathBuf, Error> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        dirs.push(Path::new(&root).join("System32").join("OpenSSH"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter()
        .map(|d| d.join(&exe))
        .find(|p| p.is_absolute() && p.is_file())
        .ok_or_else(|| {
            Error::Failed(format!(
                "the OpenSSH client ({name}) was not found; install OpenSSH (Windows: Settings > Optional features > OpenSSH Client)"
            ))
        })
}

/// ssh option values with whitespace need quoting; paths are validated to
/// contain no quote characters.
fn path_opt(key: &str, p: &Path) -> OsString {
    let text = p.to_string_lossy().replace('\\', "/");
    if text.contains(char::is_whitespace) {
        format!("{key}=\"{text}\"").into()
    } else {
        format!("{key}={text}").into()
    }
}

fn common_options(paths: &SshPaths) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["-F".into(), paths.ssh_config.as_os_str().to_owned()];
    for o in [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "GlobalKnownHostsFile=none",
        "HashKnownHosts=no",
        "UpdateHostKeys=no",
        "VerifyHostKeyDNS=no",
        "CheckHostIP=no",
        "PasswordAuthentication=no",
        "KbdInteractiveAuthentication=no",
        "ForwardAgent=no",
        "ForwardX11=no",
        "ClearAllForwardings=yes",
        "ProxyCommand=none",
        "PermitLocalCommand=no",
        "ConnectTimeout=10",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
        "LogLevel=ERROR",
    ] {
        a.push("-o".into());
        a.push(o.into());
    }
    a.push("-o".into());
    a.push(path_opt("UserKnownHostsFile", &paths.known_hosts));
    a
}

/// Full argv (without the program) for one remote command.
pub fn ssh_argv(paths: &SshPaths, target: &SshTarget, remote: &str) -> Vec<OsString> {
    let mut a = common_options(paths);
    if let Some(key) = &target.key {
        a.push("-o".into());
        a.push("IdentitiesOnly=yes".into());
        a.push("-i".into());
        a.push(key.as_os_str().to_owned());
    }
    a.extend(["-T".into(), "-l".into(), target.user.clone().into()]);
    a.extend(["-p".into(), target.port.to_string().into()]);
    a.push("--".into());
    a.push(target.host.clone().into());
    a.push(remote.into());
    a
}

pub fn keyscan_argv(target: &SshTarget) -> Vec<OsString> {
    vec![
        "-T".into(),
        "10".into(),
        "-t".into(),
        "ed25519,ecdsa,rsa".into(),
        "-p".into(),
        target.port.to_string().into(),
        "--".into(),
        target.host.clone().into(),
    ]
}

fn drain(mut r: impl Read + Send + 'static, cap: usize) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    std::thread::spawn(move || {
        let (mut kept, mut over) = (Vec::new(), false);
        let mut buf = [0u8; 8192];
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            let room = cap.saturating_sub(kept.len());
            kept.extend_from_slice(&buf[..n.min(room)]);
            over |= n > room;
        }
        (kept, over)
    })
}

/// Spawn with an argv array (never a shell), feed stdin, cap output, enforce
/// a deadline. `code` is -1 when the child was killed or has no exit code.
pub fn run_capped(
    program: &Path,
    args: &[OsString],
    stdin: &[u8],
    timeout: Duration,
) -> Result<Output, TransportError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| TransportError::Other(format!("cannot start {}: {e}", program.display())))?;
    let mut pipe = child.stdin.take();
    let input = stdin.to_vec();
    let writer = std::thread::spawn(move || {
        if let Some(p) = pipe.as_mut() {
            let _ = p.write_all(&input);
        }
        drop(pipe);
    });
    let out = drain(child.stdout.take().expect("piped"), STDOUT_CAP);
    let err = drain(child.stderr.take().expect("piped"), STDERR_CAP);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(TransportError::Other(e.to_string())),
        }
    };
    let _ = writer.join();
    let (stdout, o1) = out.join().unwrap_or_default();
    let (stderr, o2) = err.join().unwrap_or_default();
    let Some(status) = status else {
        return Err(TransportError::Timeout);
    };
    Ok(Output {
        code: status.code().unwrap_or(-1),
        stdout: sanitize(&String::from_utf8_lossy(&stdout), STDOUT_CAP),
        stderr: sanitize(&String::from_utf8_lossy(&stderr), STDERR_CAP),
        truncated: o1 || o2,
    })
}

/// Turn ssh's own failure (exit 255) into a class the engine can act on.
pub fn classify_ssh_failure(stderr: &str) -> TransportError {
    let s = stderr.to_ascii_lowercase();
    if s.contains("host key verification failed")
        || s.contains("remote host identification has changed")
        || s.contains("host key for")
    {
        TransportError::HostKeyChanged
    } else if s.contains("permission denied") || s.contains("no supported authentication") {
        TransportError::Auth
    } else if s.contains("connection refused")
        || s.contains("timed out")
        || s.contains("no route to host")
        || s.contains("could not resolve")
        || s.contains("network is unreachable")
        || s.contains("connection closed")
        || s.contains("connection reset")
    {
        TransportError::Network(first_line(stderr))
    } else {
        TransportError::Other(first_line(stderr))
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("ssh failed").trim().chars().take(200).collect()
}

pub struct OpenSshTransport {
    paths: SshPaths,
    target: SshTarget,
}

impl OpenSshTransport {
    pub fn new(paths: SshPaths, target: SshTarget) -> Self {
        Self { paths, target }
    }

    /// Create the empty `ssh_config` next to known_hosts if missing.
    pub fn prepare(paths: &SshPaths) -> Result<(), Error> {
        if let Some(dir) = paths.ssh_config.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if !paths.ssh_config.exists() {
            std::fs::write(
                &paths.ssh_config,
                "# Praxi provisioning: intentionally empty; your ~/.ssh/config is not used.\n",
            )?;
        }
        Ok(())
    }
}

impl Transport for OpenSshTransport {
    fn scan_host_keys(&self) -> Result<Vec<ScannedKey>, TransportError> {
        let out = run_capped(
            &self.paths.keyscan,
            &keyscan_argv(&self.target),
            b"",
            Duration::from_secs(25),
        )?;
        let keys = parse_scan(&out.stdout);
        if keys.is_empty() {
            return Err(TransportError::Network(
                "no SSH host key received (wrong address or port, or no SSH server)".into(),
            ));
        }
        Ok(keys)
    }

    fn run(&self, cmd: RemoteCmd, stdin: &[u8], timeout: Duration) -> Result<Output, TransportError> {
        let args = ssh_argv(&self.paths, &self.target, cmd.command_line());
        let out = run_capped(&self.paths.ssh, &args, stdin, timeout)?;
        if out.code == 255 {
            return Err(classify_ssh_failure(&out.stderr));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> SshPaths {
        SshPaths {
            ssh: "/usr/bin/ssh".into(),
            keyscan: "/usr/bin/ssh-keyscan".into(),
            known_hosts: "/data/hosts/known_hosts".into(),
            ssh_config: "/data/hosts/ssh_config".into(),
        }
    }

    #[test]
    fn argv_is_fixed_and_values_are_single_elements() {
        let target = SshTarget {
            user: "ubuntu".into(),
            host: "203.0.113.7".into(),
            port: 2222,
            key: Some("/home/me/.ssh/id key".into()),
        };
        let argv: Vec<String> = ssh_argv(&paths(), &target, RemoteCmd::Probe.command_line())
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let dashdash = argv.iter().position(|a| a == "--").unwrap();
        // after `--`: exactly the host and the constant command
        assert_eq!(&argv[dashdash + 1..], ["203.0.113.7", "sh -c 'echo praxi-ok'"]);
        assert!(argv.windows(2).any(|w| w == ["-l", "ubuntu"]));
        assert!(argv.windows(2).any(|w| w == ["-p", "2222"]));
        // the key path with a space is one argv element, not split
        assert!(argv.windows(2).any(|w| w == ["-i", "/home/me/.ssh/id key"]));
        for must in [
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "ForwardAgent=no",
            "ClearAllForwardings=yes",
            "ProxyCommand=none",
            "GlobalKnownHostsFile=none",
        ] {
            assert!(argv.iter().any(|a| a == must), "missing {must}");
        }
        assert!(argv.iter().any(|a| a == "UserKnownHostsFile=/data/hosts/known_hosts"));
        assert!(argv.windows(2).any(|w| w == ["-F", "/data/hosts/ssh_config"]));
    }

    #[test]
    fn host_key_failures_are_a_hard_class() {
        assert_eq!(
            classify_ssh_failure("@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@"),
            TransportError::HostKeyChanged
        );
        assert_eq!(
            classify_ssh_failure("ubuntu@h: Permission denied (publickey)."),
            TransportError::Auth
        );
    }
}
