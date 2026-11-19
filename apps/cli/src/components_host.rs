//! Missing local components of `bot serve`: detects the launchers local apps
//! need (uv, Node.js) and installs them on request through an official
//! package manager.  Fixed allow-list, argv arrays (no shell), bounded
//! output, a time limit; nothing is downloaded or run from a URL here.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use daemon_api::components::{ComponentDto, ComponentId, ComponentInstallState};
use daemon_api::{ComponentControl, ComponentError};
use parking_lot::Mutex;

/// An installer run lasts at most this long.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

/// One allow-listed component.
struct Spec {
    id: ComponentId,
    /// Launchers whose presence means "installed" (all must be found).
    binaries: &'static [&'static str],
    winget_id: &'static str,
    brew_formula: &'static str,
    source: &'static str,
    manual: &'static str,
}

const SPECS: [Spec; 2] = [
    Spec {
        id: ComponentId::Uv,
        binaries: &["uvx"],
        winget_id: "astral-sh.uv",
        brew_formula: "uv",
        source: "astral-sh.uv (Astral, https://docs.astral.sh/uv/)",
        manual: "https://docs.astral.sh/uv/getting-started/installation/",
    },
    Spec {
        id: ComponentId::Node,
        binaries: &["npx"],
        winget_id: "OpenJS.NodeJS.LTS",
        brew_formula: "node",
        source: "OpenJS.NodeJS.LTS (OpenJS Foundation, https://nodejs.org)",
        manual: "https://nodejs.org",
    },
];

fn spec(id: ComponentId) -> &'static Spec {
    SPECS
        .iter()
        .find(|s| s.id == id)
        .expect("every id has a spec")
}

/// Which package manager is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installer {
    /// Windows Package Manager.
    Winget,
    /// Homebrew.
    Brew,
}

/// Outcome of an installer process.
pub struct RunResult {
    /// Exit code 0.
    pub ok: bool,
}

/// Runs an argv (never a shell line).
#[async_trait]
pub trait Runner: Send + Sync {
    /// Run `argv` to completion within the limit.
    async fn run(&self, argv: &[String]) -> Result<RunResult, ()>;
    /// The package manager on this machine, if any.
    fn installer(&self) -> Option<Installer>;
}

/// The real runner: `tokio::process`, output discarded (it is never shown or
/// logged), killed on timeout.
pub struct SystemRunner;

#[async_trait]
impl Runner for SystemRunner {
    async fn run(&self, argv: &[String]) -> Result<RunResult, ()> {
        let (program, rest) = argv.split_first().ok_or(())?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(rest)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn().map_err(|_| ())?;
        match tokio::time::timeout(INSTALL_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => Ok(RunResult {
                ok: status.success(),
            }),
            _ => Err(()),
        }
    }

    fn installer(&self) -> Option<Installer> {
        if cfg!(windows) && mcp_bridge::runtime::find_tool("winget", &[]).is_some() {
            Some(Installer::Winget)
        } else if cfg!(target_os = "macos") && mcp_bridge::runtime::find_tool("brew", &[]).is_some()
        {
            Some(Installer::Brew)
        } else {
            None
        }
    }
}

impl Inner {
    fn installed(&self, s: &Spec) -> bool {
        s.binaries.iter().all(|b| {
            if self.only_extra {
                mcp_bridge::runtime::find_in(b, self.extra_dirs.clone()).is_some()
            } else {
                mcp_bridge::runtime::find_tool(b, &self.extra_dirs).is_some()
            }
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Installing,
    Done,
    Failed(&'static str),
}

/// The component service behind `/v1/components/*`.
pub struct ComponentHost(Arc<Inner>);

struct Inner {
    runner: Arc<dyn Runner>,
    extra_dirs: Vec<PathBuf>,
    /// Search only `extra_dirs` (tests): the machine's own uv must not count.
    only_extra: bool,
    jobs: Mutex<HashMap<&'static str, Job>>,
}

fn key(id: ComponentId) -> &'static str {
    match id {
        ComponentId::Uv => "uv",
        ComponentId::Node => "node",
    }
}

/// The argv of an install: only from the table, only through a known manager.
fn argv(installer: Installer, s: &Spec) -> Vec<String> {
    let v: Vec<&str> = match installer {
        Installer::Winget => vec![
            "winget",
            "install",
            "--id",
            s.winget_id,
            "-e",
            "--accept-source-agreements",
            "--accept-package-agreements",
        ],
        Installer::Brew => vec!["brew", "install", s.brew_formula],
    };
    v.into_iter().map(str::to_owned).collect()
}

impl ComponentHost {
    /// With the real runner.
    #[must_use]
    pub fn system() -> Arc<Self> {
        Self::new(Arc::new(SystemRunner), Vec::new())
    }

    /// With a given runner and extra search folders (tests).
    #[must_use]
    pub fn new(runner: Arc<dyn Runner>, extra_dirs: Vec<PathBuf>) -> Arc<Self> {
        Arc::new(Self(Arc::new(Inner {
            runner,
            only_extra: !extra_dirs.is_empty(),
            extra_dirs,
            jobs: Mutex::new(HashMap::new()),
        })))
    }

    fn installed(&self, s: &Spec) -> bool {
        Inner::installed(&self.0, s)
    }

    fn dto(&self, id: ComponentId) -> ComponentDto {
        let s = spec(id);
        let manager = self.0.runner.installer();
        let installed = self.installed(s);
        let job = self.0.jobs.lock().get(key(id)).copied();
        let (state, error) = match job {
            Some(Job::Installing) => (ComponentInstallState::Installing, None),
            Some(Job::Done) if installed => (ComponentInstallState::Done, None),
            Some(Job::Failed(code)) => (ComponentInstallState::Failed, Some(code.to_owned())),
            _ => (ComponentInstallState::Idle, None),
        };
        ComponentDto {
            id,
            installed,
            can_install: manager.is_some(),
            command: manager.map(|i| argv(i, s).join(" ")).unwrap_or_default(),
            source: s.source.to_owned(),
            manual_command: s.manual.to_owned(),
            state,
            error,
        }
    }
}

#[async_trait]
impl ComponentControl for ComponentHost {
    fn list(&self) -> Vec<ComponentDto> {
        SPECS.iter().map(|s| self.dto(s.id)).collect()
    }

    fn get(&self, id: ComponentId) -> ComponentDto {
        self.dto(id)
    }

    async fn install(&self, id: ComponentId) -> Result<ComponentDto, ComponentError> {
        let s = spec(id);
        let installer = self
            .0
            .runner
            .installer()
            .ok_or_else(|| ComponentError::Conflict("no installer is available here".into()))?;
        if self.installed(s) {
            return Err(ComponentError::Conflict("already installed".into()));
        }
        {
            let mut jobs = self.0.jobs.lock();
            if jobs.get(key(id)) == Some(&Job::Installing) {
                return Err(ComponentError::Conflict("already installing".into()));
            }
            jobs.insert(key(id), Job::Installing);
        }
        let argv = argv(installer, s);
        let me = Arc::clone(&self.0);
        tokio::spawn(async move {
            let outcome = match me.runner.run(&argv).await {
                Err(()) => Job::Failed("timeout"),
                Ok(r) if !r.ok => Job::Failed("exit"),
                // Re-detect without a restart: PATH plus the usual install folders.
                Ok(_) if Inner::installed(&me, spec(id)) => Job::Done,
                Ok(_) => Job::Failed("not_detected"),
            };
            me.jobs.lock().insert(key(id), outcome);
        });
        Ok(self.dto(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        dir: PathBuf,
        ok: bool,
    }

    #[async_trait]
    impl Runner for Fake {
        async fn run(&self, argv: &[String]) -> Result<RunResult, ()> {
            assert_eq!(argv[0], "winget");
            assert!(argv.contains(&"astral-sh.uv".to_owned()));
            if self.ok {
                let name = if cfg!(windows) { "uvx.exe" } else { "uvx" };
                std::fs::write(self.dir.join(name), "").unwrap();
            }
            Ok(RunResult { ok: self.ok })
        }
        fn installer(&self) -> Option<Installer> {
            Some(Installer::Winget)
        }
    }

    #[tokio::test]
    async fn install_is_allow_listed_and_redetected_without_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let host = ComponentHost::new(
            Arc::new(Fake {
                dir: dir.path().to_path_buf(),
                ok: true,
            }),
            vec![dir.path().to_path_buf()],
        );
        // Only ids of the enum exist; unknown names never reach the host.
        assert!(ComponentId::parse("uv; rm -rf /").is_none());
        assert!(ComponentId::parse("astral-sh.uv").is_none());
        let before = host.get(ComponentId::Uv);
        assert!(!before.installed && before.can_install);
        assert!(
            before
                .command
                .starts_with("winget install --id astral-sh.uv -e")
        );
        let started = host.install(ComponentId::Uv).await.unwrap();
        assert_eq!(started.state, ComponentInstallState::Installing);
        for _ in 0..100 {
            if host.get(ComponentId::Uv).state != ComponentInstallState::Installing {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let after = host.get(ComponentId::Uv);
        assert!(after.installed, "found at once, no restart");
        assert_eq!(after.state, ComponentInstallState::Done);
        assert!(host.install(ComponentId::Uv).await.is_err());
    }
}
