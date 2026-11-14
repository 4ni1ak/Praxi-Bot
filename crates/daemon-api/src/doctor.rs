//! Diagnostics shared by `bot doctor` and `GET /v1/doctor`.
//!
//! Every check is a plain function over injected inputs (data dir, catalog,
//! secret store, CLI detection result, ...) returning a typed [`Check`], so
//! the CLI and the daemon render the same data and tests drive the checks with
//! temp dirs and fakes.  Checks only read: the one exception is the Unicode
//! path probe, which creates, reads and deletes its own probe file.
//!
//! A [`Repair`] is a closed, allow-listed enum: never a command line.  The
//! daemon runs one only on an explicit `POST /v1/doctor/repair/{id}` (a click
//! in Settings) and audit-logs it.
//!
//! Secrets are never read into the output: only their presence is checked,
//! and support reports go through [`redact_report`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use provider_catalog::{
    Auth, Catalog, ProviderConfig, ProviderEntry, ProviderSettings, config_path,
};
use secret_store::{Redactor, SecretStore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

/// Free space below which the disk check warns.
pub const DISK_WARN_BYTES: u64 = 1 << 30;
/// Free space below which the disk check fails.
pub const DISK_FAIL_BYTES: u64 = 100 << 20;
/// A data file written further than this in the future means the clock went back.
pub const CLOCK_SKEW_TOLERANCE: Duration = Duration::from_secs(5 * 60);
/// File name of the Unicode path probe (Turkish letters on purpose: the most
/// common non-ASCII user names and folders of this project's users).
pub const UNICODE_PROBE_NAME: &str = "praxi-tanılama-ğüşiöç-İĞÜŞÖÇ.txt";
/// Earliest plausible wall-clock time (2025-01-01T00:00:00Z).
const CLOCK_FLOOR_SECS: u64 = 1_735_689_600;

/// Result class of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Fine.
    Ok,
    /// Works, but worth a look (optional part missing, low space).
    Warn,
    /// Broken; the bot will not work properly.
    Fail,
    /// Not applicable here (not configured, or not available on this
    /// platform).  Neutral, never an error.
    Skipped,
}

impl Status {
    /// Table symbol.
    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Ok => "\u{2713}",
            Self::Warn => "!",
            Self::Fail => "\u{2717}",
            Self::Skipped => "-",
        }
    }

    /// Lower-case word (`ok`, `warn`, `fail`, `skipped`).
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skipped => "skipped",
        }
    }
}

/// An allow-listed, one-click repair.  Deliberately closed: nothing here runs
/// a shell command, and regenerating the API token is not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Repair {
    /// Create the missing data directory.
    CreateDataDir,
    /// Rebuild the providers from `providers.toml` and the secret store.
    ReloadProviders,
    /// Restart the shared browser session (only when the embedder supports it).
    RestartBrowser,
}

impl Repair {
    /// Every repair.
    pub const ALL: [Self; 3] = [
        Self::CreateDataDir,
        Self::ReloadProviders,
        Self::RestartBrowser,
    ];

    /// Stable id used in `POST /v1/doctor/repair/{id}`.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::CreateDataDir => "create_data_dir",
            Self::ReloadProviders => "reload_providers",
            Self::RestartBrowser => "restart_browser",
        }
    }

    /// The repair with this exact id (`None` for anything else).
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.id() == id)
    }
}

/// One diagnostic result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Check {
    /// Stable id (`data_dir`, `providers`, ...).
    pub id: String,
    /// Short human name (`data dir`, `providers`, ...), as `bot doctor` prints it.
    pub title: String,
    /// Result class.
    pub status: Status,
    /// What was found.
    pub detail: String,
    /// How to fix it by hand (for warn / fail).
    pub hint: Option<String>,
    /// The one-click repair, when one is allowed for this finding.
    pub repair: Option<Repair>,
}

impl Check {
    /// A check result.
    #[must_use]
    pub fn new(id: &str, title: &str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            status,
            detail: detail.into(),
            hint: None,
            repair: None,
        }
    }

    /// `Ok`.
    #[must_use]
    pub fn ok(id: &str, title: &str, detail: impl Into<String>) -> Self {
        Self::new(id, title, Status::Ok, detail)
    }

    /// `Warn` with a hint.
    #[must_use]
    pub fn warn(id: &str, title: &str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(id, title, Status::Warn, detail).hint(hint)
    }

    /// `Fail` with a hint.
    #[must_use]
    pub fn fail(id: &str, title: &str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(id, title, Status::Fail, detail).hint(hint)
    }

    /// `Skipped` (neutral).
    #[must_use]
    pub fn skipped(id: &str, title: &str, detail: impl Into<String>) -> Self {
        Self::new(id, title, Status::Skipped, detail)
    }

    /// Attach a manual fix hint.
    #[must_use]
    pub fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }

    /// Attach a one-click repair.
    #[must_use]
    pub fn repair(mut self, r: Repair) -> Self {
        self.repair = Some(r);
        self
    }
}

// ------------------------------------------------------------------ checks

/// `host ownership`: whether `host.toml` is valid and this installation runs
/// its components.
#[must_use]
pub fn check_host(data_dir: &Path) -> Check {
    const ID: &str = "host_ownership";
    const N: &str = "host ownership";
    match crate::host::HostConfig::load(data_dir) {
        Ok(config) => {
            let detail = format!(
                "mode={:?}, state={:?}, epoch={}, leader={}",
                config.mode,
                config.state,
                config.epoch,
                config.leader.as_deref().unwrap_or("local")
            );
            if config.runs_components() {
                Check::ok(ID, N, detail)
            } else {
                Check::warn(
                    ID,
                    N,
                    detail,
                    "this installation is idle; use bot host status or explicitly activate --force for emergency local operation",
                )
            }
        }
        Err(e) => Check::fail(
            ID,
            N,
            e.to_string(),
            "fix host.toml; the daemon refuses invalid ownership state",
        ),
    }
}

/// `peer link`: the host / client pairing this installation is configured
/// for.  Skipped when it runs on its own.
#[must_use]
pub fn check_peer_link(data_dir: &Path) -> Check {
    use crate::host::{HostMode, HostState};
    const ID: &str = "peer_link";
    const N: &str = "peer link";
    let Ok(config) = crate::host::HostConfig::load(data_dir) else {
        return Check::skipped(ID, N, "host.toml is invalid; see the host ownership check");
    };
    let leader = config.leader.as_deref().map(short_key);
    match (config.mode, leader) {
        (HostMode::Standalone, None) => {
            Check::skipped(ID, N, "no peer or host link configured (optional)")
        }
        (HostMode::Standalone, Some(l)) => {
            Check::ok(ID, N, format!("standalone; last known leader {l}"))
        }
        (HostMode::Host, _) => Check::ok(
            ID,
            N,
            format!(
                "this machine is the always-on host (state {:?}, epoch {})",
                config.state, config.epoch
            ),
        ),
        (HostMode::Client, Some(l)) => {
            if config.state == HostState::Standby {
                Check::ok(
                    ID,
                    N,
                    format!("client of host {l} (this machine is standby)"),
                )
            } else {
                Check::ok(ID, N, format!("client of host {l}"))
            }
        }
        (HostMode::Client, None) => Check::warn(
            ID,
            N,
            "client mode, but no host is paired",
            "pair this device with the host from the host's Settings, or switch back to standalone",
        ),
    }
}

/// First 8 characters of a peer key, enough to tell devices apart.
fn short_key(key: &str) -> String {
    let short: String = key.chars().take(8).collect();
    if short.len() < key.len() {
        format!("{short}\u{2026}")
    } else {
        short
    }
}

/// `data dir`: exists and is writable.  Offers [`Repair::CreateDataDir`] when
/// it is missing.
#[must_use]
pub fn check_data_dir(dir: &Path) -> Check {
    const ID: &str = "data_dir";
    const N: &str = "data dir";
    if !dir.exists() {
        return Check::warn(
            ID,
            N,
            format!("{} does not exist yet", dir.display()),
            "it is created by `bot setup` or `bot serve`",
        )
        .repair(Repair::CreateDataDir);
    }
    let probe = dir.join(format!(".doctor-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"ok").and_then(|()| std::fs::remove_file(&probe)) {
        Ok(()) => Check::ok(ID, N, format!("{} (writable)", dir.display())),
        Err(e) => Check::fail(
            ID,
            N,
            format!("{} is not writable: {e}", dir.display()),
            "fix the folder permissions or pass another --data-dir",
        ),
    }
}

/// `unicode paths`: create, read back and delete a file whose name has
/// Turkish letters, inside the data dir (catches code pages and file systems
/// that mangle non-ASCII names).
#[must_use]
pub fn check_unicode_path(dir: &Path) -> Check {
    const ID: &str = "unicode_path";
    const N: &str = "unicode paths";
    if !dir.is_dir() {
        return Check::skipped(ID, N, "data dir does not exist yet");
    }
    let name = format!("{}-{UNICODE_PROBE_NAME}", std::process::id());
    let path = dir.join(&name);
    let payload = "Praxi tanılama: ğüşiöç İĞÜŞÖÇ";
    let result = (|| -> std::io::Result<bool> {
        std::fs::write(&path, payload)?;
        let back = std::fs::read_to_string(&path)?;
        let listed = std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_str() == Some(name.as_str()));
        std::fs::remove_file(&path)?;
        if back != payload {
            return Err(std::io::Error::other("content changed on read-back"));
        }
        Ok(listed)
    })();
    match result {
        Ok(true) => Check::ok(ID, N, "non-ASCII file names work in the data dir"),
        // Readable by name, but listed in another form (e.g. Unicode
        // normalisation of the file system): works, worth knowing.
        Ok(false) => Check::warn(
            ID,
            N,
            "non-ASCII file names work, but the folder lists them in a different form",
            "harmless for the bot; keep file names ASCII if another tool cannot find them",
        ),
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Check::fail(
                ID,
                N,
                format!("non-ASCII file name probe failed: {e}"),
                "move the data dir to a folder on an NTFS/ext4/APFS disk, or set BOT_DATA_DIR to an ASCII-only path",
            )
        }
    }
}

/// `config`: `providers.toml` parses.
#[must_use]
pub fn check_config(dir: &Path) -> Check {
    const ID: &str = "config";
    const N: &str = "config";
    let path = config_path(dir);
    if !path.exists() {
        return Check::warn(
            ID,
            N,
            "providers.toml not found",
            "run `bot setup` to add a provider",
        );
    }
    match ProviderConfig::load(&path) {
        Ok(c) => Check::ok(
            ID,
            N,
            format!("providers.toml parses ({} provider(s))", c.providers.len()),
        ),
        Err(e) => Check::fail(
            ID,
            N,
            format!("providers.toml: {e}"),
            "fix the file by hand or move it away and run `bot setup`",
        ),
    }
}

/// The opened secret store and a label of its backend, or why it failed.
pub type StoreRef<'a> = Result<(&'a dyn SecretStore, String), String>;

/// `secret backend`: the store opens and lists.
#[must_use]
pub fn check_secrets(store: &StoreRef<'_>) -> Check {
    const ID: &str = "secret_backend";
    const N: &str = "secret backend";
    match store {
        Ok((store, kind)) => match store.list_names() {
            Ok(names) => Check::ok(ID, N, format!("{kind} ({} secret(s))", names.len())),
            Err(e) => Check::fail(
                ID,
                N,
                format!("{kind}: {e}"),
                "check BOT_SECRETS_BACKEND / BOT_SECRETS_PASSPHRASE",
            ),
        },
        Err(e) => Check::fail(
            ID,
            N,
            e.clone(),
            "set BOT_SECRETS_PASSPHRASE for the encrypted file, or use BOT_SECRETS_BACKEND=keyring",
        ),
    }
}

/// Online test of one provider: the number of models it answered with.
pub type OnlineProbe<'a> =
    &'a dyn Fn(&ProviderSettings, &ProviderEntry, &dyn SecretStore) -> Result<usize, String>;

/// Everything the provider check looks at.
pub struct ProviderInputs<'a> {
    /// Data directory (`providers.toml`).
    pub data_dir: &'a Path,
    /// Loaded catalog, or why loading failed.
    pub catalog: Result<&'a Catalog, String>,
    /// Secret store.
    pub store: &'a StoreRef<'a>,
    /// Detected coding CLIs (`None` = not probed).
    pub detected: Option<&'a [cli_provider::DetectedCli]>,
    /// Test connections over the network (`bot doctor --online`).
    pub online: Option<OnlineProbe<'a>>,
}

/// `providers`: every enabled provider has its key (or, for a coding CLI
/// provider, a signed-in CLI).
#[must_use]
pub fn check_providers(inp: &ProviderInputs<'_>) -> Check {
    const ID: &str = "providers";
    const N: &str = "providers";
    let Ok(cfg) = ProviderConfig::load(&config_path(inp.data_dir)) else {
        return Check::warn(ID, N, "no readable provider config", "see the config check");
    };
    let enabled: Vec<_> = cfg.providers.iter().filter(|p| p.enabled).collect();
    if enabled.is_empty() {
        return Check::fail(
            ID,
            N,
            "setup required: no enabled provider",
            "run `bot setup` or `bot providers add`",
        );
    }
    let mut problems = Vec::new();
    let mut notes = Vec::new();
    for p in &enabled {
        if p.cli.is_some() {
            match cli_state(&p.id, inp.detected) {
                Ok(note) => notes.push(note),
                Err(problem) => problems.push(problem),
            }
            continue;
        }
        let entry = p.custom.clone().or_else(|| {
            inp.catalog
                .as_ref()
                .ok()
                .and_then(|c| c.get(&p.id).cloned())
        });
        let Some(entry) = entry else {
            problems.push(format!("{}: not in the catalog", p.id));
            continue;
        };
        let key_ok = match (&p.key, &entry.auth) {
            (Some(r), _) => match inp.store {
                Ok((s, _)) => s.get(r.name()).ok().flatten().is_some(),
                Err(_) => false,
            },
            (None, Auth::None) => true,
            (None, Auth::ApiKeyEnv(var)) => std::env::var(var).is_ok_and(|v| !v.is_empty()),
        };
        if !key_ok {
            problems.push(format!("{}: API key not available", p.id));
            continue;
        }
        if let Some(probe) = inp.online {
            match inp.store {
                Ok((s, _)) => match probe(p, &entry, *s) {
                    Ok(n) => notes.push(format!("{}: reachable ({n} models)", p.id)),
                    Err(e) => problems.push(format!("{}: {e}", p.id)),
                },
                Err(_) => problems.push(format!("{}: secret store unavailable", p.id)),
            }
        } else {
            notes.push(format!("{}: configured", p.id));
        }
    }
    let detail = notes
        .iter()
        .chain(problems.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if problems.is_empty() {
        let c = Check::ok(ID, N, detail);
        if inp.online.is_some() {
            c
        } else {
            c.hint("add --online to test the connections")
        }
    } else {
        Check::fail(
            ID,
            N,
            detail,
            "`bot secrets set provider.<id>.api_key` or `bot providers add <id>` to fix the key",
        )
    }
}

/// State of an enabled `cli:<engine>` provider from the detection result.
fn cli_state(
    provider_id: &str,
    detected: Option<&[cli_provider::DetectedCli]>,
) -> Result<String, String> {
    let engine = provider_id.strip_prefix("cli:").unwrap_or(provider_id);
    let Some(detected) = detected else {
        return Ok(format!("{provider_id}: configured (CLI not probed)"));
    };
    match detected.iter().find(|d| d.id == engine) {
        Some(d) if !d.installed => Err(format!("{provider_id}: CLI is not installed")),
        Some(d) if d.auth == cli_provider::AuthState::SignedOut => Err(format!(
            "{provider_id}: CLI is installed but signed out; sign in with `{}`",
            sign_in_command(engine)
        )),
        Some(d) if !d.runnable => Err(format!(
            "{provider_id}: CLI cannot run turns ({})",
            d.detail.as_deref().unwrap_or("not runnable")
        )),
        Some(_) => Ok(format!("{provider_id}: CLI ready")),
        None => Err(format!("{provider_id}: unknown coding CLI")),
    }
}

/// Terminal command that signs in to a coding CLI.
fn sign_in_command(engine: &str) -> &str {
    match engine {
        "codex" | "chatgpt-plan" => "codex login",
        other => other,
    }
}

/// `cli accounts`: coding CLIs found on this machine (subscription use, ADR
/// 0028).  Optional: nothing found is neutral.
#[must_use]
pub fn check_cli_accounts(detected: Option<&[cli_provider::DetectedCli]>) -> Check {
    const ID: &str = "cli_accounts";
    const N: &str = "cli accounts";
    let Some(detected) = detected else {
        return Check::skipped(ID, N, "coding CLI detection did not answer in time");
    };
    let installed: Vec<_> = detected.iter().filter(|d| d.installed).collect();
    if installed.is_empty() {
        return Check::skipped(ID, N, "no coding CLI installed (optional)");
    }
    let mut ready = Vec::new();
    let mut signed_out = Vec::new();
    let mut other = Vec::new();
    for d in installed {
        match d.auth {
            cli_provider::AuthState::SignedOut => signed_out.push(d.display_name.clone()),
            _ if d.runnable => ready.push(d.display_name.clone()),
            _ => other.push(d.display_name.clone()),
        }
    }
    let mut parts = Vec::new();
    if !ready.is_empty() {
        parts.push(format!("ready: {}", ready.join(", ")));
    }
    if !signed_out.is_empty() {
        parts.push(format!("signed out: {}", signed_out.join(", ")));
    }
    if !other.is_empty() {
        parts.push(format!("detected only: {}", other.join(", ")));
    }
    let detail = parts.join("; ");
    if signed_out.is_empty() {
        Check::ok(ID, N, detail)
    } else {
        Check::warn(
            ID,
            N,
            detail,
            "open a terminal and sign in to the CLI once (e.g. `claude`, `codex login`)",
        )
    }
}

/// `model catalog`: the catalog loads and knows the default model.
#[must_use]
pub fn check_catalog(data_dir: &Path, catalog: &Result<&Catalog, String>) -> Check {
    const ID: &str = "model_catalog";
    const N: &str = "model catalog";
    let catalog = match catalog {
        Ok(c) => c,
        Err(e) => {
            return Check::fail(
                ID,
                N,
                e.clone(),
                "fix or unset BOT_CATALOG_DIR to use the built-in catalog",
            );
        }
    };
    if catalog.is_empty() {
        return Check::fail(ID, N, "catalog is empty", "unset BOT_CATALOG_DIR");
    }
    let n_models: usize = catalog.all().map(|e| e.models.len()).sum();
    let detail = format!("{} provider(s), {n_models} known model(s)", catalog.len());
    if let Ok(cfg) = ProviderConfig::load(&config_path(data_dir))
        && let Some(def) = cfg.effective_default()
    {
        match &def.default_model {
            None if def.cli.is_none() => {
                return Check::warn(
                    ID,
                    N,
                    format!("{detail}; default provider {} has no model", def.id),
                    "`bot providers default <id> --model <model>`",
                );
            }
            None => {}
            Some(m) => {
                let entry = def.custom.as_ref().or_else(|| catalog.get(&def.id));
                if entry.is_some_and(|e| !e.models.is_empty() && e.model(m).is_none()) {
                    return Check::warn(
                        ID,
                        N,
                        format!("{detail}; default model {m} is not in the catalog"),
                        "prices and limits are unknown for it; check the model id",
                    );
                }
            }
        }
    }
    Check::ok(ID, N, detail)
}

/// `provider reload`: providers enabled in `providers.toml` that the running
/// daemon has not loaded (added behind its back).  Offers
/// [`Repair::ReloadProviders`].
#[must_use]
pub fn check_providers_loaded(data_dir: &Path, active: &[String]) -> Check {
    const ID: &str = "providers_loaded";
    const N: &str = "loaded providers";
    let Ok(cfg) = ProviderConfig::load(&config_path(data_dir)) else {
        return Check::skipped(ID, N, "no readable provider config");
    };
    let missing: Vec<&str> = cfg
        .providers
        .iter()
        .filter(|p| p.enabled && !active.iter().any(|a| a == &p.id))
        .map(|p| p.id.as_str())
        .collect();
    if missing.is_empty() {
        if active.is_empty() {
            Check::skipped(ID, N, "no provider configured yet")
        } else {
            Check::ok(ID, N, format!("{} provider(s) loaded", active.len()))
        }
    } else {
        Check::warn(
            ID,
            N,
            format!("enabled but not loaded: {}", missing.join(", ")),
            "reload the providers; if that fails, see the providers check",
        )
        .repair(Repair::ReloadProviders)
    }
}

/// `schedule tools`: whether `schedule.toml` is usable.  A broken one switches
/// the bots' schedule tools off (they never fall back to looser defaults).
#[must_use]
pub fn check_schedule_tools(dir: &Path) -> Check {
    const ID: &str = "schedule_tools";
    const N: &str = "schedule tools";
    match crate::load_schedule_limits(dir) {
        Ok(l) => Check::ok(
            ID,
            N,
            format!(
                "max {} per bot, minimum interval {} s",
                l.max_per_bot, l.min_interval_secs
            ),
        ),
        Err(e) => Check::fail(
            ID,
            N,
            format!("{e}; schedule tools are disabled until it is fixed"),
            format!("fix or delete {}", dir.join("schedule.toml").display()),
        ),
    }
}

/// `disk space` of the volume holding the data dir (or its nearest existing
/// parent).
#[must_use]
pub fn check_disk(dir: &Path) -> Check {
    let mut probe = dir;
    while !probe.exists() {
        match probe.parent() {
            Some(p) if !p.as_os_str().is_empty() => probe = p,
            _ => break,
        }
    }
    disk_check(fs4::available_space(probe).map_err(|e| e.to_string()))
}

/// The disk check over a measured free-space value.
#[must_use]
pub fn disk_check(free: Result<u64, String>) -> Check {
    const ID: &str = "disk_space";
    const N: &str = "disk space";
    match free {
        Ok(free) => {
            let detail = format!("{} MiB free", free >> 20);
            if free < DISK_FAIL_BYTES {
                Check::fail(
                    ID,
                    N,
                    detail,
                    "free some disk space; runs and logs cannot be saved",
                )
            } else if free < DISK_WARN_BYTES {
                Check::warn(ID, N, detail, "less than 1 GiB left; free some space")
            } else {
                Check::ok(ID, N, detail)
            }
        }
        Err(e) => Check::warn(
            ID,
            N,
            format!("cannot read free space: {e}"),
            "ignore if unsure",
        ),
    }
}

/// Newest modification time of the files directly inside `dir`.
#[must_use]
pub fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
}

/// `clock`: the wall clock is plausible and did not jump back behind data
/// this installation already wrote.  Local only: it is not compared with a
/// network time source (no outbound request).
#[must_use]
pub fn check_clock(now: SystemTime, newest_data: Option<SystemTime>) -> Check {
    const ID: &str = "clock";
    const N: &str = "clock";
    let hint = "turn on automatic time sync in the operating system settings";
    let floor = SystemTime::UNIX_EPOCH + Duration::from_secs(CLOCK_FLOOR_SECS);
    if now < floor {
        return Check::fail(
            ID,
            N,
            "system clock is set before 2025; schedules and TLS will misbehave",
            hint,
        );
    }
    if let Some(newest) = newest_data
        && let Ok(ahead) = newest.duration_since(now)
        && ahead > CLOCK_SKEW_TOLERANCE
    {
        return Check::warn(
            ID,
            N,
            format!(
                "data files are {} min in the future: the clock went back",
                ahead.as_secs() / 60
            ),
            hint,
        );
    }
    Check::ok(
        ID,
        N,
        "plausible (checked locally, not against network time)",
    )
}

/// One capability the daemon may lack, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CapabilityStatus {
    /// Capability id (`display`, `browser`, `desktop_automation`, ...).
    pub id: String,
    /// Whether it can be used now.
    pub available: bool,
    /// Why not, when unavailable.
    pub reason: Option<String>,
}

/// `capabilities`: one neutral line per unavailable capability, never an
/// error ("not available on this platform" is a fact, not a failure).
#[must_use]
pub fn check_capabilities(caps: &[CapabilityStatus]) -> Check {
    const ID: &str = "capabilities";
    const N: &str = "capabilities";
    let available: Vec<&str> = caps
        .iter()
        .filter(|c| c.available)
        .map(|c| c.id.as_str())
        .collect();
    let missing: Vec<String> = caps
        .iter()
        .filter(|c| !c.available)
        .map(|c| match &c.reason {
            Some(r) => format!("{} ({r})", c.id),
            None => c.id.clone(),
        })
        .collect();
    let mut detail = if available.is_empty() {
        "none available".to_owned()
    } else {
        format!("available: {}", available.join(", "))
    };
    if !missing.is_empty() {
        detail.push_str("; not available here: ");
        detail.push_str(&missing.join(", "));
    }
    if available.is_empty() && !caps.is_empty() {
        Check::skipped(ID, N, detail)
    } else {
        Check::ok(ID, N, detail)
    }
}

/// `desktop automation`: the OS automation backend (UI Automation on
/// Windows).  Unsupported platforms are neutral.
#[must_use]
pub fn check_desktop_automation(os: &str, caps: Option<&desktop_api::Capabilities>) -> Check {
    const ID: &str = "desktop_automation";
    const N: &str = "desktop automation";
    match caps {
        Some(c) if !c.has_display => Check::skipped(
            ID,
            N,
            format!("{} backend loaded, but no display is available", c.platform),
        ),
        Some(c) => {
            let mut parts = Vec::new();
            if c.accessibility_tree {
                parts.push("accessibility tree");
            }
            if c.screenshot {
                parts.push("screenshots");
            }
            if c.app_launch {
                parts.push("app launch");
            }
            Check::ok(ID, N, format!("{}: {}", c.platform, parts.join(", ")))
        }
        None if os == "windows" => Check::warn(
            ID,
            N,
            "the Windows automation backend is not running",
            "unset BOT_DESKTOP=off and restart the daemon; see the daemon log for the reason",
        ),
        None => Check::skipped(ID, N, "not available on this platform"),
    }
}

/// `browser` (discovery only, no launch): which browser the tools would
/// start, or why none is usable.
#[must_use]
pub fn check_browser_found(found: Result<&Path, &str>) -> Check {
    const ID: &str = "browser";
    const N: &str = "browser";
    match found {
        Ok(p) if browser_cdp::launch::is_snap_browser(p) => Check::fail(
            ID,
            N,
            "snap browser is unsupported under the hardened service",
            "install non-snap Chrome/Chromium or set BOT_BROWSER_PATH",
        ),
        Ok(p) => Check::ok(ID, N, format!("{} (found; not launched)", p.display())),
        Err(e) => Check::warn(
            ID,
            N,
            e.to_owned(),
            "install a non-snap Chrome/Chromium/Edge or set BOT_BROWSER_PATH",
        ),
    }
}

// ---------------------------------------------------------------- rendering

/// Render the checks as the `bot doctor` table.
///
/// # Errors
/// Write errors of `out`.
pub fn render_table(checks: &[Check], out: &mut dyn Write) -> std::io::Result<()> {
    let w = checks.iter().map(|c| c.title.len()).max().unwrap_or(0);
    for c in checks {
        writeln!(out, " {}  {:w$}  {}", c.status.symbol(), c.title, c.detail)?;
        if let (Some(h), Status::Warn | Status::Fail) = (&c.hint, c.status) {
            writeln!(out, "    {:w$}  fix: {h}", "")?;
        }
    }
    let count = |s: Status| checks.iter().filter(|c| c.status == s).count();
    let (fails, warns, skipped) = (
        count(Status::Fail),
        count(Status::Warn),
        count(Status::Skipped),
    );
    let ok = checks.len() - fails - warns - skipped;
    if skipped == 0 {
        writeln!(out, "\n{fails} failed, {warns} warning(s), {ok} ok")
    } else {
        writeln!(
            out,
            "\n{fails} failed, {warns} warning(s), {ok} ok, {skipped} skipped"
        )
    }
}

/// The checks as the `bot doctor --json` document.
#[must_use]
pub fn to_json(checks: &[Check]) -> Value {
    json!({
        "ok": checks.iter().all(|c| c.status != Status::Fail),
        "checks": checks.iter().map(|c| json!({
            "name": c.title,
            "status": c.status.word(),
            "detail": c.detail,
            "hint": c.hint,
        })).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------- redaction

/// The user's home directory as text (`USERPROFILE`, else `HOME`).
#[must_use]
pub fn home_dir() -> Option<String> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|h| h.to_string_lossy().into_owned())
        .filter(|h| h.len() > 3)
}

/// Length in bytes of the prefix of `hay` that equals `needle` ignoring
/// letter case (Unicode), if it does.
fn ci_prefix(hay: &str, needle: &str) -> Option<usize> {
    let mut h = hay.char_indices();
    for n in needle.chars() {
        let (_, c) = h.next()?;
        if c != n && !c.to_lowercase().eq(n.to_lowercase()) {
            return None;
        }
    }
    Some(h.next().map_or(hay.len(), |(i, _)| i))
}

/// Replace every case-insensitive occurrence of `needle` in `hay`.
fn replace_ci(hay: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return hay.to_owned();
    }
    let mut out = String::with_capacity(hay.len());
    let mut rest = hay;
    while let Some(c) = rest.chars().next() {
        if let Some(len) = ci_prefix(rest, needle) {
            out.push_str(with);
            rest = &rest[len..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// Whether `c` can be part of an e-mail local part or domain.
fn email_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '%')
}

/// Mask e-mail addresses (`[email]`).
#[must_use]
pub fn mask_emails(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '@' {
            let mut start = out.len();
            // Walk back over the local part already copied to `out`.
            for (idx, c) in out.char_indices().rev() {
                if email_char(c) {
                    start = idx;
                } else {
                    break;
                }
            }
            let mut end = i + 1;
            while end < chars.len() && email_char(chars[end]) {
                end += 1;
            }
            let domain: String = chars[i + 1..end].iter().collect();
            let domain = domain.trim_end_matches('.');
            if start < out.len() && domain.contains('.') && !domain.starts_with('.') {
                out.truncate(start);
                out.push_str("[email]");
                i += 1 + domain.chars().count();
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Mask runs of 24+ `[A-Za-z0-9_-]` characters (keys, tokens) as `[redacted]`.
#[must_use]
pub fn mask_long_tokens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= 24 {
            out.push_str("[redacted]");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            run.push(ch);
        } else {
            flush(&mut run, &mut out);
            out.push(ch);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Redact text that leaves the machine: the home directory becomes `~`
/// (both slash styles, any letter case), then the secret [`Redactor`] runs
/// (known secret values and key shapes), then e-mail addresses and any
/// remaining token-like run are masked.
#[must_use]
pub fn redact_report(text: &str, redactor: Option<&Redactor>, home: Option<&str>) -> String {
    let mut s = text.to_owned();
    if let Some(home) = home.filter(|h| h.len() > 3) {
        let home = home.trim_end_matches(['/', '\\']);
        s = replace_ci(&s, home, "~");
        s = replace_ci(&s, &home.replace('\\', "/"), "~");
        s = replace_ci(&s, &home.replace('/', "\\"), "~");
    }
    if let Some(r) = redactor {
        s = r.redact(&s);
    }
    mask_long_tokens(&mask_emails(&s))
}

// ---------------------------------------------------------------- report

/// Config files the report may name (names only, never contents).
pub const CONFIG_FILES: &[&str] = &[
    "providers.toml",
    "host.toml",
    "mcp.toml",
    "web.toml",
    "email.toml",
    "channels.toml",
    "schedule.toml",
    "policy.json",
];

/// Which of [`CONFIG_FILES`] exist in `dir`.
#[must_use]
pub fn present_config_files(dir: &Path) -> Vec<String> {
    CONFIG_FILES
        .iter()
        .filter(|n| dir.join(n).is_file())
        .map(|n| (*n).to_owned())
        .collect()
}

/// What the support report says besides the checks.
#[derive(Debug, Clone)]
pub struct ReportInfo {
    /// Daemon / CLI version.
    pub version: String,
    /// HTTP API version.
    pub api_version: String,
    /// `os/arch`.
    pub platform: String,
    /// Data directory (redacted on output).
    pub data_dir: PathBuf,
    /// Config file names present in the data dir.
    pub config_files: Vec<String>,
}

/// A Markdown support report.  Run [`redact_report`] over the result before
/// it leaves the process (the daemon route does).
#[must_use]
pub fn report_markdown(info: &ReportInfo, checks: &[Check]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "# Praxi diagnostics report\n");
    let _ = writeln!(s, "- Version: {}", info.version);
    let _ = writeln!(s, "- API: {}", info.api_version);
    let _ = writeln!(s, "- Platform: {}", info.platform);
    let _ = writeln!(s, "- Data dir: `{}`", info.data_dir.display());
    let files = if info.config_files.is_empty() {
        "(none)".to_owned()
    } else {
        info.config_files.join(", ")
    };
    let _ = writeln!(s, "- Config files: {files}");
    let count = |st: Status| checks.iter().filter(|c| c.status == st).count();
    let _ = writeln!(
        s,
        "- Summary: {} failed, {} warning(s), {} ok, {} skipped\n",
        count(Status::Fail),
        count(Status::Warn),
        count(Status::Ok),
        count(Status::Skipped)
    );
    let _ = writeln!(s, "## Checks\n");
    let _ = writeln!(s, "| Status | Check | Detail |");
    let _ = writeln!(s, "| --- | --- | --- |");
    let cell = |t: &str| t.replace('|', "\\|").replace(['\n', '\r'], " ");
    for c in checks {
        let mut detail = cell(&c.detail);
        if let (Some(h), Status::Warn | Status::Fail) = (&c.hint, c.status) {
            let _ = write!(detail, " (fix: {})", cell(h));
        }
        let _ = writeln!(s, "| {} | {} | {detail} |", c.status.word(), cell(&c.title));
    }
    s
}

#[cfg(test)]
mod tests;
