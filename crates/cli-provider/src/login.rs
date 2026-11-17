//! Interactive sign-in of a coding CLI on THIS machine (the always-on host), driven from the
//! owner's desktop.
//!
//! Only the vendor's own, unmodified login command runs, from a fixed registry entry (argv
//! arrays, no shell, nothing supplied by a client). Its output stays in bounded private memory;
//! only a verified sign-in page, a one-time device code and the fact that it wants a pasted code
//! leave this module. A pasted code goes to the process' stdin and nowhere else.
//!
//! What each vendor offers headlessly (checked with `--help` only, never by signing in):
//! * `claude auth login` prints a sign-in link and, after the person signed in in ANY browser,
//!   reads the code Anthropic's page shows from stdin (paste-code flow). `claude auth status
//!   --json` confirms.
//! * `codex login --device-auth` prints a page (`https://auth.openai.com/codex/device`) and a
//!   one-time code (device-code flow); the process exits 0 when the person approved.
//!   `codex login status` confirms.
//! * `opencode auth login [target] [--method] [--answer]` is a provider/method form: it needs
//!   the provider chosen interactively, so it is not driven here (use an API key instead).
//! * `agy` has no login subcommand: the first interactive run signs in with a local browser
//!   callback. Not drivable remotely.
//! * `cursor-agent login` is not installed on the development machine, so its flow could not be
//!   checked; it is offered as the exact command to run over SSH.

use std::sync::OnceLock;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::ChildStdin;
use tokio::sync::mpsc;

use crate::detect::{ChildTree, PathEnv, ShimError, apply_sanitized_env, isolate, resolve_spawn};

/// Most output bytes kept per login (everything beyond is dropped, never forwarded).
const MAX_OUTPUT: usize = 32 * 1024;
/// A pasted code: one line, this long at most.
pub const MAX_CODE: usize = 1024;

/// How a vendor's login finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowKind {
    /// The CLI prints a page and a one-time code; the person types the code on the page.
    DeviceCode,
    /// The CLI prints a sign-in link; the person signs in and pastes the page's code back.
    PasteCode,
}

/// One entry of the fixed registry. No field is ever derived from a request.
#[derive(Debug)]
pub struct LoginSpec {
    pub engine: &'static str,
    /// Argv after the executable.
    pub args: &'static [&'static str],
    pub kind: FlowKind,
    /// Hosts the shown page may be on (exact or subdomain).
    pub hosts: &'static [&'static str],
}

pub const LOGIN_SPECS: &[LoginSpec] = &[
    LoginSpec {
        engine: "claude",
        args: &["auth", "login"],
        kind: FlowKind::PasteCode,
        hosts: &[
            "claude.com",
            "claude.ai",
            "console.anthropic.com",
            "platform.claude.com",
        ],
    },
    LoginSpec {
        engine: "codex",
        args: &["login", "--device-auth"],
        kind: FlowKind::DeviceCode,
        hosts: &["auth.openai.com"],
    },
];

#[must_use]
pub fn login_spec(engine: &str) -> Option<&'static LoginSpec> {
    LOGIN_SPECS.iter().find(|s| s.engine == engine)
}

/// What the output asks of the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// Open `url`; for a device flow type `code` there.
    Page { url: String, code: Option<String> },
}

fn strip_ansi(text: &str) -> String {
    static ANSI: OnceLock<regex::Regex> = OnceLock::new();
    let re = ANSI.get_or_init(|| {
        regex::Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07").expect("valid pattern")
    });
    re.replace_all(text, "").into_owned()
}

/// An https page on one of `hosts`, with no login, port or fragment; the query is dropped for a
/// device page (the code is shown separately) and kept for a paste flow (it carries the state).
fn trusted_page(value: &str, spec: &LoginSpec) -> Option<String> {
    let value = value.trim_end_matches([')', ',', '.', ';', '"', '\'', '>']);
    let url = url::Url::parse(value).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let trusted = spec
        .hosts
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")));
    let plain = url.scheme() == "https"
        && trusted
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.fragment().is_none();
    if !plain {
        return None;
    }
    match spec.kind {
        FlowKind::PasteCode => Some(url.to_string()),
        FlowKind::DeviceCode => {
            // The one device page; anything else a CLI prints is not shown.
            (url.path().trim_end_matches('/') == "/codex/device" && url.query().is_none())
                .then(|| "https://auth.openai.com/codex/device".to_owned())
        }
    }
}

fn device_code(line: &str) -> Option<String> {
    static CODE: OnceLock<regex::Regex> = OnceLock::new();
    let re = CODE.get_or_init(|| {
        regex::Regex::new(r"^[A-Z0-9]{4,8}-[A-Z0-9]{4,8}$").expect("valid pattern")
    });
    let line = line.trim();
    re.is_match(line).then(|| line.to_owned())
}

/// Read what to show from the output so far. Only COMPLETE lines count (output arrives in
/// chunks that can stop inside a code), and only text matching the vendor's rules is returned.
#[must_use]
pub fn parse_prompt(spec: &LoginSpec, output: &str) -> Option<Prompt> {
    let clean = strip_ansi(output);
    let mut lines: Vec<&str> = clean.split('\n').collect();
    lines.pop(); // the last, unfinished line
    let mut url = None;
    let mut code = None;
    for line in lines {
        let line = line.trim_end_matches('\r').trim();
        if let Some(found) = line
            .split_whitespace()
            .filter(|w| w.starts_with("https://"))
            .find_map(|w| trusted_page(w, spec))
        {
            url = Some(found);
        }
        if spec.kind == FlowKind::DeviceCode
            && let Some(c) = device_code(line)
        {
            code = Some(c);
        }
    }
    let url = url?;
    match spec.kind {
        FlowKind::PasteCode => Some(Prompt::Page { url, code: None }),
        FlowKind::DeviceCode => code.map(|code| Prompt::Page {
            url,
            code: Some(code),
        }),
    }
}

/// What Anthropic's page shows after sign-in: a code, sometimes with `#state`.
#[must_use]
pub fn valid_code(value: &str) -> Option<&str> {
    let code = value.trim();
    (code.len() >= 8
        && code.len() <= MAX_CODE
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'#' | b'.' | b':')))
    .then_some(code)
}

/// A running login command. Dropping it kills the process tree.
pub struct LoginRun {
    child: ChildTree,
    stdin: Option<ChildStdin>,
    rx: mpsc::Receiver<Event>,
    output: String,
    closed_streams: u8,
}

enum Event {
    Bytes(Vec<u8>),
    Closed,
}

/// Why a login could not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError {
    /// The CLI is not on the search path.
    NotInstalled,
    /// Refused by the shim rules or could not be spawned.
    SpawnFailed,
}

/// Start the registry's login command. `env` decides where the CLI finds its home: the sanitized
/// environment keeps HOME / USERPROFILE of the service user, so the credential lands where the
/// engine's turns will read it.
///
/// # Errors
/// See [`StartError`].
pub fn start(spec: &'static LoginSpec, env: &PathEnv) -> Result<LoginRun, StartError> {
    let engine = crate::engine_spec(spec.engine).ok_or(StartError::SpawnFailed)?;
    let candidate = crate::search::find_engine(engine, env).ok_or(StartError::NotInstalled)?;
    let resolved =
        resolve_spawn(&candidate, env).map_err(|ShimError::Refused| StartError::SpawnFailed)?;
    if crate::detect::is_batch_file(&resolved.program) {
        return Err(StartError::SpawnFailed);
    }
    let mut cmd = tokio::process::Command::new(&resolved.program);
    cmd.args(&resolved.args)
        .args(spec.args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    apply_sanitized_env(&mut cmd, env);
    cmd.env("NO_COLOR", "1").env("TERM", "dumb");
    #[cfg(windows)]
    let wrap = isolate(cmd, crate::detect::SharedJob::new());
    #[cfg(not(windows))]
    let wrap = isolate(cmd);
    let mut wrap = wrap;
    let mut child = ChildTree::new(wrap.spawn().map_err(|_| StartError::SpawnFailed)?);
    let stdin = child.stdin().take();
    let stdout = child.stdout().take().ok_or(StartError::SpawnFailed)?;
    let stderr = child.stderr().take().ok_or(StartError::SpawnFailed)?;
    let (tx, rx) = mpsc::channel(64);
    spawn_reader(stdout, tx.clone());
    spawn_reader(stderr, tx);
    Ok(LoginRun {
        child,
        stdin,
        rx,
        output: String::new(),
        closed_streams: 0,
    })
}

fn spawn_reader(
    mut reader: impl tokio::io::AsyncRead + Send + Unpin + 'static,
    tx: mpsc::Sender<Event>,
) {
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(Event::Bytes(buf[..n].to_vec())).await.is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(Event::Closed).await;
    });
}

impl LoginRun {
    /// Wait up to `wait` for more output and return what to show, if it is there yet.
    /// `None` after the timeout means "nothing new"; the output is kept private.
    pub async fn next_prompt(&mut self, spec: &LoginSpec, wait: Duration) -> Option<Prompt> {
        if let Some(p) = parse_prompt(spec, &self.output) {
            return Some(p);
        }
        let deadline = tokio::time::Instant::now() + wait;
        while self.closed_streams < 2 {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some(Event::Bytes(b))) => {
                    if self.output.len() < MAX_OUTPUT {
                        self.output.push_str(&String::from_utf8_lossy(&b));
                    }
                    if let Some(p) = parse_prompt(spec, &self.output) {
                        return Some(p);
                    }
                }
                Ok(Some(Event::Closed)) => self.closed_streams += 1,
                Ok(None) => self.closed_streams = 2,
                Err(_) => return None,
            }
        }
        None
    }

    /// The captured output (private: used for failure classification, never forwarded).
    #[must_use]
    pub fn private_output(&self) -> &str {
        &self.output
    }

    /// Hand one pasted line to the process' stdin. The code is not stored.
    ///
    /// # Errors
    /// The stdin pipe is gone.
    pub async fn send_line(&mut self, code: &str) -> std::io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        stdin.write_all(code.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await
    }

    /// Drain output (bounded) until the process exits; its exit code.
    pub async fn wait_exit(&mut self, limit: Duration) -> Option<Option<i32>> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            tokio::select! {
                status = self.child.wait() => return status.ok().map(|s| s.code()),
                ev = self.rx.recv() => {
                    if let Some(Event::Bytes(b)) = ev
                        && self.output.len() < MAX_OUTPUT
                    {
                        self.output.push_str(&String::from_utf8_lossy(&b));
                    }
                }
                () = tokio::time::sleep_until(deadline) => return None,
            }
        }
    }

    /// Kill the whole process tree and reap it.
    pub async fn terminate(&mut self) {
        self.stdin = None;
        self.child.terminate().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_fixed_argv_with_no_shell_and_no_client_input() {
        for spec in LOGIN_SPECS {
            assert!(crate::engine_spec(spec.engine).is_some(), "{}", spec.engine);
            assert_ne!(spec.args.len(), 0);
            for arg in spec.args {
                assert!(
                    arg.bytes()
                        .all(|b| b.is_ascii_lowercase() || b == b'-' || b == b'_'),
                    "{arg}"
                );
            }
            assert_ne!(spec.hosts.len(), 0);
        }
        // One entry per engine.
        let mut ids: Vec<_> = LOGIN_SPECS.iter().map(|s| s.engine).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), LOGIN_SPECS.len());
    }

    #[test]
    fn a_device_flow_shows_only_the_openai_page_and_a_well_formed_code() {
        let spec = login_spec("codex").unwrap();
        let out = "Welcome\n\x1b[1mhttps://auth.openai.com/codex/device\x1b[0m\nEnter this one-time code\n  AB12-CD34\n";
        assert_eq!(
            parse_prompt(spec, out),
            Some(Prompt::Page {
                url: "https://auth.openai.com/codex/device".into(),
                code: Some("AB12-CD34".into())
            })
        );
        // A half-printed code (no newline yet) is never read; other pages are never shown.
        assert_eq!(
            parse_prompt(spec, "https://auth.openai.com/codex/device\nAB12-CD"),
            None
        );
        assert_eq!(
            parse_prompt(spec, "https://evil.example/codex/device\nAB12-CD34\n"),
            None
        );
        assert_eq!(
            parse_prompt(
                spec,
                "https://auth.openai.com/oauth?token=secret\nAB12-CD34\n"
            ),
            None
        );
    }

    #[test]
    fn a_paste_flow_shows_only_a_vendor_link_without_userinfo() {
        let spec = login_spec("claude").unwrap();
        let ok = "Opening browser\nIf the browser did not open, visit: https://claude.com/cai/oauth/authorize?code=true&state=abc\nPaste code here > ";
        match parse_prompt(spec, ok) {
            Some(Prompt::Page { url, code: None }) => {
                assert!(url.starts_with("https://claude.com/cai/oauth/authorize"));
            }
            other => panic!("{other:?}"),
        }
        for bad in [
            "visit: https://evil.example/oauth/authorize?code=true\n",
            "visit: https://user:pw@claude.com/x\n",
            "visit: http://claude.com/x\n",
            "visit: https://claude.com.evil.example/x\n",
        ] {
            assert_eq!(parse_prompt(spec, bad), None, "{bad}");
        }
    }

    #[test]
    fn a_pasted_code_is_one_bounded_line_of_plain_characters() {
        assert_eq!(valid_code("  GOODCODE#abc  "), Some("GOODCODE#abc"));
        for bad in [
            "short",
            "has space inside",
            "line\nbreak",
            "x;rm -rf",
            &"a".repeat(MAX_CODE + 1),
        ] {
            assert_eq!(valid_code(bad), None, "{bad}");
        }
    }
}
