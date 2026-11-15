//! `bot host add|repair|upgrade|remove`: provision your own server over SSH.
//!
//! Runs in the owner's terminal only; there is no daemon route for it. The
//! engine is the `host-provision` crate. This file is the terminal side: it
//! reads flags, asks about a new host key, and talks to the LOCAL daemon's
//! host-link API (`/v1/hosts`) to pair and check health.
//!
//! SSH uses key files or `ssh-agent` only (`BatchMode`). Passwords are not
//! supported in this version.
use std::{
    io::{BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use clap::{Args, Subcommand};
use host_provision::{
    Engine, HostHealth, HostRecord, KnownHosts, LocalHost, Mode, OpenSshTransport, PairedHost,
    Params, RemoveLevel, SshPaths, SshTarget, StepResult, Store, Ui, validate,
};
use reqwest::Method;
use serde_json::{Value, json};

use crate::{
    Result,
    client::{Client, exit, fail},
};

#[derive(Debug, Args)]
pub struct TargetArgs {
    /// `user@server` (for example ubuntu@203.0.113.7), or the name of a saved server.
    pub target: String,
    /// SSH port (default 22). Not the daemon's global `--port`.
    #[arg(long = "ssh-port")]
    pub ssh_port: Option<u32>,
    /// Private key file. Without it the ssh-agent is used. Passwords are not supported.
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// Local name for this server (default: derived from the address).
    #[arg(long)]
    pub name: Option<String>,
    /// Accept a NEW host key without asking (compare the fingerprint first).
    /// A changed key is never accepted.
    #[arg(long)]
    pub yes: bool,
    /// Skip the virtual display and desktop session.
    #[arg(long)]
    pub no_desktop: bool,
}

#[derive(Debug, Subcommand)]
pub enum ProvisionCmd {
    /// Install Praxi on your own Ubuntu/Debian server over SSH and pair it
    /// with this computer (idempotent: run it again to resume).
    Add(TargetArgs),
    /// Re-run the install on a server, migrating a plain `bot serve` install
    /// to host mode in place (its data is kept and snapshotted first).
    Repair(TargetArgs),
    /// Install this app's version on a server that runs an older one.
    Upgrade(TargetArgs),
    /// Forget a server and revoke this computer on it.
    Remove {
        /// Saved server name or `user@server`.
        name: String,
        /// Also uninstall Praxi on the server (keeps its data).
        #[arg(long)]
        uninstall: bool,
        /// Uninstall AND delete the server's data, config and praxi user.
        #[arg(long, conflicts_with = "uninstall")]
        purge: bool,
        /// Skip the typed confirmation for --purge.
        #[arg(long)]
        yes: bool,
        #[arg(long = "ssh-port")]
        ssh_port: Option<u32>,
        #[arg(long)]
        key: Option<PathBuf>,
    },
}

struct TerminalUi {
    accept: bool,
}

impl Ui for TerminalUi {
    fn progress(&mut self, r: &StepResult) {
        println!("[{:<4}] {}: {}", r.status.label(), r.title, r.detail);
        if let Some(h) = &r.remediation
            && r.status != host_provision::Status::Pass
        {
            println!("       -> {h}");
        }
    }

    fn note(&mut self, line: &str) {
        println!("... {line}");
    }

    fn confirm_host_key(&mut self, host: &str, port: u16, fingerprint: &str) -> bool {
        println!("\nFirst connection to {host} (port {port}).");
        println!("The server's SSH host key fingerprint is:\n  {fingerprint}");
        println!("Compare it with the value in your provider's console if it shows one.");
        if self.accept {
            println!("Accepting it (--yes).");
            return true;
        }
        if !std::io::stdin().is_terminal() {
            println!("Not a terminal: re-run with --yes to accept this key.");
            return false;
        }
        print!("Trust this server and continue? Type yes: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).is_ok() && line.trim() == "yes"
    }
}

/// This computer's daemon (`/v1/hosts`), reached over loopback.
struct LocalDaemon<'a> {
    rt: tokio::runtime::Runtime,
    client: &'a dyn Fn() -> Result<Client>,
}

impl LocalDaemon<'_> {
    fn call(
        &self,
        c: &Client,
        m: Method,
        path: &str,
        body: Option<Value>,
    ) -> std::result::Result<Value, String> {
        self.rt
            .block_on(c.request(m, path, body))
            .map_err(|e| e.to_string())
    }

    fn local(&self) -> std::result::Result<Client, String> {
        (self.client)().map_err(|e| e.to_string())
    }
}

impl LocalHost for LocalDaemon<'_> {
    fn pair(&self, ticket: &str) -> std::result::Result<PairedHost, String> {
        let c = self.local()?;
        let v = self.call(
            &c,
            Method::POST,
            "/v1/hosts/pair",
            Some(json!({ "ticket": ticket })),
        )?;
        Ok(PairedHost {
            id: v["id"]
                .as_str()
                .ok_or("unexpected pairing answer")?
                .to_owned(),
            name: v["name"].as_str().unwrap_or("host").to_owned(),
        })
    }

    fn knows_host(&self, id: &str) -> bool {
        let Ok(c) = self.local() else { return false };
        self.call(&c, Method::GET, "/v1/hosts", None)
            .is_ok_and(|v| {
                v["hosts"]
                    .as_array()
                    .is_some_and(|h| h.iter().any(|x| x["id"] == id))
            })
    }

    fn health(&self, id: &str) -> std::result::Result<HostHealth, String> {
        let c = self.local()?;
        let link = self.call(&c, Method::POST, &format!("/v1/hosts/{id}/connect"), None)?;
        let base = link["base_url"].as_str().ok_or("no channel")?;
        let token = link["token"].as_str().ok_or("no channel")?;
        let host = Client::new(base, token.to_owned());
        let status = self.call(&host, Method::GET, "/v1/host", None);
        let targets = self.call(&host, Method::GET, "/v1/view/targets", None);
        let _ = self.call(&c, Method::DELETE, &format!("/v1/hosts/{id}/connect"), None);
        let s = status?;
        Ok(HostHealth {
            mode: s["mode"].as_str().unwrap_or_default().to_owned(),
            version: s["version"].as_str().unwrap_or_default().to_owned(),
            has_display: s["has_display"].as_bool().unwrap_or(false),
            has_browser: s["has_browser"].as_bool().unwrap_or(false),
            view_targets: targets.ok().and_then(|t| t.as_array().map(Vec::len)),
        })
    }

    fn host_ids(&self) -> Vec<String> {
        let Ok(c) = self.local() else {
            return Vec::new();
        };
        self.call(&c, Method::GET, "/v1/hosts", None)
            .ok()
            .and_then(|v| {
                v["hosts"].as_array().map(|h| {
                    h.iter()
                        .filter_map(|x| x["id"].as_str().map(str::to_owned))
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    fn forget_host(&self, id: &str) -> std::result::Result<(), String> {
        let c = self.local()?;
        self.call(&c, Method::DELETE, &format!("/v1/hosts/{id}"), None)
            .map(|_| ())
    }

    fn revoke_host(&self, id: &str) -> std::result::Result<(), String> {
        let c = self.local()?;
        let key = peer_network::key_from_string(id).map_err(|e| e.to_string())?;
        self.call(&c, Method::DELETE, &format!("/v1/host/devices/{key}"), None)
            .map(|_| ())
    }
}

fn resolve(store: &Store, args: &TargetArgs) -> Result<HostRecord> {
    resolve_parts(
        store,
        &args.target,
        args.ssh_port,
        args.key.as_deref(),
        args.name.as_deref(),
    )
}

pub(crate) fn resolve_parts(
    store: &Store,
    target: &str,
    port: Option<u32>,
    key: Option<&Path>,
    name: Option<&str>,
) -> Result<HostRecord> {
    let mut rec = if target.contains('@') {
        let (user, host) = validate::target(target)?;
        store
            .find(&format!("{user}@{host}"))
            .unwrap_or_else(|| HostRecord {
                name: validate::default_name(&host),
                user,
                host,
                port: 22,
                key_path: None,
                fingerprint: None,
                paired_host_id: None,
                desktop_key: None,
                installed_version: None,
            })
    } else {
        store
            .load(&validate::name(target)?)
            .ok_or_else(|| format!("no saved server named {target:?}; use user@server"))?
    };
    if let Some(n) = name {
        rec.name = validate::name(n)?;
    }
    if let Some(p) = port {
        rec.port = validate::port(p)?;
    }
    if let Some(k) = key {
        rec.key_path = Some(validate::key_path(k)?);
    }
    Ok(rec)
}

pub(crate) fn transport(store: &Store, rec: &HostRecord) -> Result<OpenSshTransport> {
    let paths = SshPaths {
        ssh: host_provision::ssh::resolve_binary("ssh")?,
        known_hosts: store.known_hosts_path(),
        ssh_config: store.ssh_config_path(),
    };
    OpenSshTransport::prepare(&paths)?;
    Ok(OpenSshTransport::new(
        paths,
        SshTarget {
            user: rec.user.clone(),
            host: rec.host.clone(),
            port: rec.port,
            key: rec.key_path.clone(),
        },
    ))
}

pub fn run(cmd: &ProvisionCmd, data_dir: &Path, client: &dyn Fn() -> Result<Client>) -> Result<()> {
    let store = Store::from_data_dir(data_dir);
    let known_hosts = KnownHosts::new(store.known_hosts_path());
    let local = LocalDaemon {
        rt: tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
        client,
    };
    let (args, mode) = match cmd {
        ProvisionCmd::Add(a) => (a, Mode::Add),
        ProvisionCmd::Repair(a) => (a, Mode::Repair),
        ProvisionCmd::Upgrade(a) => (a, Mode::Upgrade),
        ProvisionCmd::Remove {
            name,
            uninstall,
            purge,
            yes,
            ssh_port,
            key,
        } => {
            return remove(
                &store,
                &known_hosts,
                &local,
                name,
                *uninstall,
                *purge,
                *yes,
                *ssh_port,
                key.as_deref(),
            );
        }
    };
    let record = resolve(&store, args)?;
    store.save(&record)?;
    let transport = transport(&store, &record)?;
    let mut ui = TerminalUi { accept: args.yes };
    println!(
        "Provisioning {}@{}:{} as \"{}\" ({mode:?}). SSH uses a key file or ssh-agent; passwords are not supported.\n",
        record.user, record.host, record.port, record.name
    );
    let log = store.log_path(&record.name);
    let mut engine = Engine {
        transport: &transport,
        local: &local,
        ui: &mut ui,
        store: &store,
        known_hosts: &known_hosts,
        params: Params {
            record,
            mode,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            desktop: !args.no_desktop,
            retry_delay: Duration::from_secs(5),
            allow_install: true,
        },
    };
    let report = engine.run();
    println!("\n{}", report.table());
    println!("Log (redacted): {}", log.display());
    if report.hard_stop {
        return Err(fail(
            exit::AUTH,
            "host key mismatch: this is not the server you set up; nothing was changed",
        ));
    }
    if !report.ok() {
        return Err(fail(
            exit::GENERIC,
            "provisioning stopped at a failed step; fix it and run the same command again to resume",
        ));
    }
    println!(
        "Done. The server is a paired Praxi host. It still needs a model key (`bot setup` on the host, or add one from this computer when that flow lands)."
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)] // flat CLI flags
fn remove(
    store: &Store,
    known_hosts: &KnownHosts,
    local: &LocalDaemon<'_>,
    name: &str,
    uninstall: bool,
    purge: bool,
    yes: bool,
    port: Option<u32>,
    key: Option<&Path>,
) -> Result<()> {
    let record = resolve_parts(store, name, port, key, None)?;
    if purge && !yes {
        if !std::io::stdin().is_terminal() {
            return Err(
                "--purge deletes the server's data; run it in a terminal or pass --yes".into(),
            );
        }
        print!(
            "This DELETES all Praxi data (bots, memory, keys) on {}. Type the server name to confirm: ",
            record.name
        );
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        if line.trim() != record.name {
            return Err("not confirmed; nothing was changed".into());
        }
    }
    let level = if purge {
        RemoveLevel::Purge
    } else if uninstall {
        RemoveLevel::Uninstall
    } else {
        RemoveLevel::Disconnect
    };
    let transport = transport(store, &record)?;
    let mut ui = TerminalUi { accept: false };
    let mut engine = Engine {
        transport: &transport,
        local,
        ui: &mut ui,
        store,
        known_hosts,
        params: Params {
            record,
            mode: Mode::Repair,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            desktop: true,
            retry_delay: Duration::ZERO,
            allow_install: true,
        },
    };
    let report = engine.remove(level);
    println!("\n{}", report.table());
    if report.ok() {
        Ok(())
    } else {
        Err(fail(
            exit::GENERIC,
            "remove did not finish; the local record was kept",
        ))
    }
}
