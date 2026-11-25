//! Capability packs: tools a server needs before the bot can do office, document, media or
//! developer work (LibreOffice, pandoc, ffmpeg, git ...).
//!
//! Only the OWNER's SSH login installs software: the host's service user is unprivileged, so a bot
//! cannot `apt install` anything and must ask the owner. What can run is a fixed, closed set:
//!
//! * the catalog (`scripts/packs.txt`) is compiled into the app; a pack is addressed by its catalog
//!   id only, never by free text or a package name;
//! * the app uploads the catalog next to `provision.sh` and sends its SHA-256 on stdin; the server
//!   script refuses a staged copy that differs, so nothing but the app's own catalog runs as root;
//! * packages come from the server's own signed apt sources (no third-party repository, no
//!   download-and-run); the script installs with `--no-install-recommends` and never purges;
//! * a dry-run plan ([`Packs::plan`]) changes nothing and is what the owner approves first.
//!
//! Every operation is idempotent: installing an installed pack, or removing a missing one, is a
//! success that changes nothing.
use std::{fmt, fmt::Write as _, time::Duration};

use sha2::{Digest, Sha256};

use crate::{
    remote, result,
    transport::{Output, RemoteCmd, StageFile, Sub, Transport, TransportError, sanitize},
};

/// Catalog id of the web scraping add-on (the one pack with extra files to stage).
pub const SCRAPE: &str = "scrape";

const SHORT: Duration = Duration::from_secs(60);
/// A full office pack download can be slow on a small server.
const LONG: Duration = Duration::from_secs(1800);

/// Licence facts of one pack (`scripts/pack-licenses.txt`), shown for every pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct License {
    /// SPDX expression of the software the pack installs.
    pub spdx: String,
    /// GPL/AGPL family.
    pub copyleft: bool,
    /// Installed from the server's own distribution (Praxi ships nothing and downloads nothing).
    pub distro: bool,
    pub source_url: String,
    pub notice_url: String,
    /// Pinned upstream version (empty for distribution packs).
    pub version: String,
}

/// Parse `pack-licenses.txt`: `id|spdx|copyleft|distro|source_url|notice_url|version`.
pub fn parse_licenses(text: &str) -> Vec<(String, License)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            if f.len() != 7 || !valid_id(f[0]) {
                return None;
            }
            Some((
                f[0].to_owned(),
                License {
                    spdx: f[1].to_owned(),
                    copyleft: f[2] == "1",
                    distro: f[3] == "1",
                    source_url: f[4].to_owned(),
                    notice_url: f[5].to_owned(),
                    version: f[6].to_owned(),
                },
            ))
        })
        .collect()
}

impl License {
    /// Separate copyleft software downloaded from its author (not from the distribution): the
    /// owner must acknowledge the notice before it is installed.
    pub fn needs_acknowledgement(&self) -> bool {
        self.copyleft && !self.distro
    }
}

/// One catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    /// Licence facts; `None` only for a hand-parsed catalog (the shipped one has them all, a test
    /// guards that). A pack without them is treated as needing acknowledgement.
    pub license: Option<License>,
    pub id: String,
    /// Part of the wizard's "basic set" switch.
    pub base: bool,
    /// False for shared system tools (git, curl, python3): Praxi never removes those.
    pub removable: bool,
    /// Rough download size shown before the owner approves.
    pub size_mb: u32,
    pub packages: Vec<String>,
    /// Programs the fact sheet looks for. `a/b` means either name.
    pub tools: Vec<String>,
}

/// Parse the catalog text. Unknown or malformed lines are skipped (the catalog is ours, so this
/// only matters for the test that guards the shipped file).
pub fn parse_catalog(text: &str) -> Vec<Pack> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            if f.len() != 6 || !valid_id(f[0]) {
                return None;
            }
            let words = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
            Some(Pack {
                license: None,
                id: f[0].to_owned(),
                base: f[1] == "1",
                removable: f[2] == "1",
                size_mb: f[3].parse().ok()?,
                packages: words(f[4]),
                tools: words(f[5]),
            })
        })
        .collect()
}

fn valid_id(id: &str) -> bool {
    (2..=16).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase())
}

/// The catalog this build ships, with each pack's licence facts attached.
pub fn catalog() -> Vec<Pack> {
    let licenses = parse_licenses(&remote::pack_licenses_txt());
    parse_catalog(&remote::packs_txt())
        .into_iter()
        .map(|mut p| {
            p.license = licenses
                .iter()
                .find(|(id, _)| *id == p.id)
                .map(|(_, l)| l.clone());
            p
        })
        .collect()
}

impl Pack {
    /// The licence notice must be acknowledged before this pack is installed.
    pub fn needs_acknowledgement(&self) -> bool {
        self.license
            .as_ref()
            .is_none_or(License::needs_acknowledgement)
    }

    /// May this pack be installed, given what the owner acknowledged?
    pub fn installable(&self, acknowledged: bool) -> Result<(), PackError> {
        if self.needs_acknowledgement() && !acknowledged {
            Err(PackError::NeedsAcknowledgement)
        } else {
            Ok(())
        }
    }
}

/// SHA-256 (hex) of the scrape add-on's pinned requirements: shown as its lockfile checksum.
pub fn scrape_lock_sha() -> String {
    sha_hex(remote::scrape_lock().as_bytes())
}

/// SHA-256 of the shipped catalog (lowercase hex): what the server verifies.
pub fn catalog_sha() -> String {
    sha_hex(remote::packs_txt().as_bytes())
}

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        })
}

/// Hash of exactly what one pack operation touches (pack id, action, package list). The dry-run
/// plan carries it and the install/remove sends it back, where the server script recomputes it
/// from its checksum-verified catalog: what runs is provably what the owner saw.
pub fn list_sha(id: &str, action: Action) -> Option<String> {
    let pack = catalog().into_iter().find(|p| p.id == id)?;
    let text = format!(
        "{id}
{}
{}
",
        action.word(),
        pack.packages.join(" ")
    );
    Some(sha_hex(text.as_bytes()))
}

/// Ids of the packs the wizard's "basic set" switch installs.
pub fn base_ids() -> Vec<String> {
    catalog()
        .into_iter()
        .filter(|p| p.base)
        .map(|p| p.id)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackStatus {
    Installed,
    /// Some of the pack's packages are there (an interrupted install).
    Partial,
    Missing,
}

impl PackStatus {
    fn parse(s: &str) -> Self {
        match s {
            "installed" => Self::Installed,
            "partial" => Self::Partial,
            _ => Self::Missing,
        }
    }
}

/// The "installed tools" fact sheet of one server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostTools {
    /// The server has `apt` (Debian, Ubuntu): the only family packs support.
    pub supported: bool,
    /// The login can run administrator commands without a password (needed to install).
    pub can_admin: bool,
    pub os_id: String,
    pub packs: Vec<(String, PackStatus)>,
    /// `(tool, found)`, in catalog order.
    pub tools: Vec<(String, bool)>,
}

impl HostTools {
    pub fn status(&self, id: &str) -> PackStatus {
        self.packs
            .iter()
            .find(|(p, _)| p == id)
            .map_or(PackStatus::Missing, |(_, s)| *s)
    }

    /// One short line for a bot's system prompt, so it does not probe with `shell_run`.
    /// Only tools that are really there; empty when there are none.
    pub fn summary_line(&self) -> String {
        let found: Vec<&str> = self
            .tools
            .iter()
            .filter(|(_, f)| *f)
            .map(|(t, _)| t.as_str())
            .collect();
        if found.is_empty() {
            String::new()
        } else {
            format!("Installed tools on this computer: {}.", found.join(", "))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Remove,
}

impl Action {
    fn word(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Remove => "remove",
        }
    }
}

/// What a dry run found for one pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub pack: String,
    pub action: Action,
    /// The catalog's packages that would be installed or removed.
    pub packages: Vec<String>,
    /// How many packages in total apt would unpack (dependencies included); 0 when unknown.
    pub total_packages: u32,
    pub size_mb: u32,
    /// [`list_sha`] of this pack and action: sent back with the install or remove.
    pub list_sha: String,
    /// Why this cannot run (`shared`, `dependants`, `unavailable`).
    pub blocked: Option<Blocked>,
}

impl Plan {
    /// Nothing to do: already installed (or already gone).
    pub fn is_noop(&self) -> bool {
        self.packages.is_empty() && self.blocked.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// Shared system tools: never removed by Praxi.
    Shared,
    /// Other software on the server depends on the pack.
    Dependants,
    /// The server's software sources do not offer a package.
    Unavailable,
    /// The web scraping add-on needs Python 3.10+ with venv (the "dev" tools pack).
    NeedsPython,
}

/// Why a pack operation did not work, already in words the app can map to plain sentences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackError {
    /// Not a Debian/Ubuntu server.
    Unsupported,
    /// The login cannot use `sudo` without a password.
    NeedsAdmin,
    /// The server could not download or install it (offline, disk full, locked apt ...).
    Failed(String),
    /// A catalog id this build does not know.
    UnknownPack,
    /// Shared system tools or software that depends on the pack.
    Blocked(Blocked),
    /// SSH itself failed (login, host key, network).
    Connection(String),
    /// A separate copyleft pack: the owner has not acknowledged its licence notice yet.
    NeedsAcknowledgement,
}

impl PackError {
    /// Stable machine code the app maps to a localised sentence.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::NeedsAdmin => "needs_admin",
            Self::Failed(_) => "failed",
            Self::UnknownPack => "unknown_pack",
            Self::Blocked(Blocked::Shared) => "blocked_shared",
            Self::Blocked(Blocked::Dependants) => "blocked_dependants",
            Self::Blocked(Blocked::Unavailable) => "unavailable",
            Self::Blocked(Blocked::NeedsPython) => "needs_python",
            Self::Connection(_) => "connection",
            Self::NeedsAcknowledgement => "needs_ack",
        }
    }

    /// Technical detail for "copy details" (already sanitised and redacted).
    pub fn detail(&self) -> String {
        match self {
            Self::Failed(d) | Self::Connection(d) => d.clone(),
            other => other.code().to_owned(),
        }
    }
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code())
    }
}

/// Runs pack operations over one SSH [`Transport`].
pub struct Packs<'a> {
    pub transport: &'a dyn Transport,
}

fn conn(e: &TransportError) -> PackError {
    PackError::Connection(sanitize(&e.to_string(), 300))
}

/// `key=value` lines. Unlike the preflight parser, keys may hold digits (`tool_python3`).
fn parse_kv(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| {
            !k.is_empty()
                && k.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        })
        .map(|(k, v)| (k.to_owned(), v.trim().to_owned()))
        .collect()
}

fn get<'k>(kv: &'k [(String, String)], key: &str) -> &'k str {
    kv.iter()
        .find(|(k, _)| k == key)
        .map_or("", |(_, v)| v.as_str())
}

fn names(v: &str) -> Vec<String> {
    v.split(',')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

impl Packs<'_> {
    fn stage(&self, pack: Option<&str>) -> Result<(), PackError> {
        let go = || -> Result<(), TransportError> {
            self.transport
                .upload(StageFile::Provision, remote::provision_sh().as_bytes())?;
            self.transport
                .upload(StageFile::Packs, remote::packs_txt().as_bytes())?;
            if pack == Some(SCRAPE) {
                self.transport.upload(
                    StageFile::ScrapeService,
                    remote::scrape_service_py().as_bytes(),
                )?;
                self.transport
                    .upload(StageFile::ScrapeLock, remote::scrape_lock().as_bytes())?;
                self.transport
                    .upload(StageFile::ScrapeUnit, remote::scrape_unit().as_bytes())?;
            }
            Ok(())
        };
        go().map_err(|e| conn(&e))
    }

    fn cleanup(&self) {
        let _ = self
            .transport
            .run(RemoteCmd::Cleanup, b"", Duration::from_secs(15));
    }

    /// Stage, run one fixed subcommand, clean up. `pack` is checked against the catalog first.
    fn call(
        &self,
        sub: Sub,
        pack: Option<(&str, Action)>,
        approved: Option<&str>,
        timeout: Duration,
    ) -> Result<Output, PackError> {
        let mut stdin = format!("catalog={}\n", catalog_sha());
        if let Some((id, action)) = pack {
            if !catalog().iter().any(|p| p.id == id) {
                return Err(PackError::UnknownPack);
            }
            stdin.push_str(&format!("pack={id}\naction={}\n", action.word()));
            if id == SCRAPE {
                // The server re-hashes the staged files against these: only the app's own copies run.
                for (key, text) in [
                    ("scrape_service", remote::scrape_service_py()),
                    ("scrape_lock", remote::scrape_lock()),
                    ("scrape_unit", remote::scrape_unit()),
                ] {
                    stdin.push_str(&format!("{key}={}\n", sha_hex(text.as_bytes())));
                }
            }
        }
        if let Some(sha) = approved {
            stdin.push_str(&format!("plan={sha}\n"));
        }
        self.stage(pack.map(|(id, _)| id))?;
        let out = self
            .transport
            .run(RemoteCmd::Provision(sub), stdin.as_bytes(), timeout);
        self.cleanup();
        let out = out.map_err(|e| conn(&e))?;
        if out.ok() {
            Ok(out)
        } else {
            Err(classify(&out))
        }
    }

    /// The fact sheet: which packs and tools this server has. Read-only, no root.
    pub fn facts(&self) -> Result<HostTools, PackError> {
        let out = self.call(Sub::PacksFacts, None, None, SHORT)?;
        let kv = parse_kv(&out.stdout);
        let supported = get(&kv, "apt") == "1";
        let cat = catalog();
        let packs = cat
            .iter()
            .map(|p| {
                let s = if supported {
                    PackStatus::parse(get(&kv, &format!("pack_{}", p.id)))
                } else {
                    PackStatus::Missing
                };
                (p.id.clone(), s)
            })
            .collect();
        let tools = cat
            .iter()
            .flat_map(|p| p.tools.iter())
            .map(|t| {
                let name = t.split('/').next().unwrap_or(t).to_owned();
                let found = get(&kv, &format!("tool_{name}")) == "1";
                (name, found)
            })
            .collect();
        Ok(HostTools {
            supported,
            can_admin: get(&kv, "admin") == "1",
            os_id: sanitize(get(&kv, "os_id"), 32),
            packs,
            tools,
        })
    }

    /// Dry run for one pack: changes nothing on the server.
    pub fn plan(&self, id: &str, action: Action) -> Result<Plan, PackError> {
        let size_mb = catalog()
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.size_mb)
            .ok_or(PackError::UnknownPack)?;
        let out = self.call(Sub::PacksPlan, Some((id, action)), None, SHORT * 3)?;
        let kv = parse_kv(&out.stdout);
        let blocked = match (get(&kv, "blocked"), get(&kv, "unavailable")) {
            ("shared", _) => Some(Blocked::Shared),
            ("dependants", _) => Some(Blocked::Dependants),
            (_, u) if !u.is_empty() && id == SCRAPE => Some(Blocked::NeedsPython),
            (_, u) if !u.is_empty() => Some(Blocked::Unavailable),
            _ => None,
        };
        Ok(Plan {
            pack: id.to_owned(),
            action,
            packages: names(get(&kv, "missing")),
            total_packages: get(&kv, "new_packages").parse().unwrap_or(0),
            size_mb,
            list_sha: list_sha(id, action).ok_or(PackError::UnknownPack)?,
            blocked,
        })
    }

    /// Install or remove one pack (root, through the owner's SSH login). Idempotent. `approved` is
    /// the [`Plan::list_sha`] the owner saw; the server refuses anything else, and there is no way
    /// to run without one.
    pub fn apply(&self, id: &str, action: Action, approved: &str) -> Result<(), PackError> {
        self.apply_acknowledged(id, action, approved, false)
    }

    /// [`Packs::apply`] after the owner acknowledged the licence notice of a separate copyleft
    /// pack. Without that, such a pack is refused here, whatever the caller above did.
    pub fn apply_acknowledged(
        &self,
        id: &str,
        action: Action,
        approved: &str,
        acknowledged: bool,
    ) -> Result<(), PackError> {
        if action == Action::Install {
            let pack = catalog()
                .into_iter()
                .find(|p| p.id == id)
                .ok_or(PackError::UnknownPack)?;
            pack.installable(acknowledged)?;
        }
        let sub = match action {
            Action::Install => Sub::PackInstall,
            Action::Remove => Sub::PackRemove,
        };
        self.call(sub, Some((id, action)), Some(approved), LONG)
            .map(|_| ())
    }
}

/// Map a failed script run to an error the app can explain in plain words.
fn classify(out: &Output) -> PackError {
    let err = out.stderr.to_lowercase();
    if err.contains("sudo:") || err.contains("must run as root") {
        PackError::NeedsAdmin
    } else if err.contains("only debian and ubuntu") {
        PackError::Unsupported
    } else if err.contains("blocked: this pack holds shared") {
        PackError::Blocked(Blocked::Shared)
    } else if err.contains("blocked: other software") {
        PackError::Blocked(Blocked::Dependants)
    } else if err.contains("python 3.10 or newer") {
        PackError::Blocked(Blocked::NeedsPython)
    } else if err.contains("not-available: package") {
        PackError::Blocked(Blocked::Unavailable)
    } else {
        let tail: Vec<&str> = out.stderr.lines().rev().take(4).collect();
        let tail = tail.into_iter().rev().collect::<Vec<_>>().join(" | ");
        PackError::Failed(format!(
            "exit {}: {}",
            out.code,
            sanitize(&result::redact(&tail), 400)
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::known_hosts::ScannedKey;

    /// A scripted server that records what it was asked to run.
    struct Fake {
        ran: RefCell<Vec<(RemoteCmd, String)>>,
        uploaded: RefCell<Vec<StageFile>>,
        reply: Box<dyn Fn(RemoteCmd) -> Output>,
    }

    impl Fake {
        fn new(reply: impl Fn(RemoteCmd) -> Output + 'static) -> Self {
            Self {
                ran: RefCell::default(),
                uploaded: RefCell::default(),
                reply: Box::new(reply),
            }
        }
    }

    impl Transport for Fake {
        fn scan_host_keys(&self) -> Result<Vec<ScannedKey>, TransportError> {
            Ok(Vec::new())
        }
        fn run(
            &self,
            cmd: RemoteCmd,
            stdin: &[u8],
            _t: Duration,
        ) -> Result<Output, TransportError> {
            if let RemoteCmd::Stage(f) = cmd {
                self.uploaded.borrow_mut().push(f);
                return Ok(Output::default());
            }
            self.ran
                .borrow_mut()
                .push((cmd, String::from_utf8_lossy(stdin).into_owned()));
            Ok((self.reply)(cmd))
        }
    }

    fn ok(stdout: &str) -> Output {
        Output {
            stdout: stdout.into(),
            ..Output::default()
        }
    }

    #[test]
    fn the_shipped_catalog_is_sound_and_covers_the_agreed_tools() {
        let cat = catalog();
        let ids: Vec<&str> = cat.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["office", "docs", "ocr", "media", "dev", "scrape"]);
        assert_eq!(base_ids(), ["office", "docs", "media", "dev"]);
        let has = |id: &str, pkg: &str| {
            cat.iter()
                .find(|p| p.id == id)
                .unwrap()
                .packages
                .iter()
                .any(|p| p == pkg)
        };
        // Turkish rendering needs the Noto and Liberation fonts; OCR needs Turkish data.
        assert!(has("office", "fonts-noto-core") && has("office", "fonts-liberation"));
        assert!(has("ocr", "tesseract-ocr-tur") && has("ocr", "tesseract-ocr-eng"));
        assert!(!cat.iter().find(|p| p.id == "dev").unwrap().removable);
        // No package belongs to two packs (removing one must not break another) and every name
        // is the plain Debian shape the server script re-checks.
        let mut seen = std::collections::HashSet::new();
        for pkg in cat.iter().flat_map(|p| &p.packages) {
            assert!(seen.insert(pkg.clone()), "{pkg} listed twice");
            assert!(
                pkg.len() >= 2
                    && pkg.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
                    && pkg.bytes().all(|b| b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || matches!(b, b'+' | b'.' | b'-')),
                "{pkg}"
            );
        }
        assert_eq!(catalog_sha().len(), 64);
        // the app shows this total next to the wizard's switch (BASE_SET_MB in toolPacks.ts)
        assert_eq!(
            cat.iter()
                .filter(|p| p.base)
                .map(|p| p.size_mb)
                .sum::<u32>(),
            1350
        );
    }

    #[test]
    fn facts_read_packs_and_tools_and_summarise_only_what_is_there() {
        let t = Fake::new(|_| {
            ok(
                "apt=1\nos_id=ubuntu\npack_office=installed\npack_docs=partial\npack_ocr=missing\n\
                pack_media=missing\npack_dev=installed\ntool_soffice=1\ntool_pandoc=0\ntool_python3=1\n\
                tool_magick=0\nrm -rf=1\n",
            )
        });
        let f = Packs { transport: &t }.facts().unwrap();
        assert!(f.supported);
        assert_eq!(f.status("office"), PackStatus::Installed);
        assert_eq!(f.status("docs"), PackStatus::Partial);
        assert_eq!(f.status("media"), PackStatus::Missing);
        // digits in a key (python3, sqlite3) survive parsing
        assert!(f.tools.contains(&("python3".to_owned(), true)));
        let line = f.summary_line();
        assert!(
            line.contains("soffice") && line.contains("python3"),
            "{line}"
        );
        assert!(!line.contains("pandoc"), "{line}");
        // the fixed catalog is uploaded and the helper cleans up after itself
        assert_eq!(
            *t.uploaded.borrow(),
            [StageFile::Provision, StageFile::Packs],
            "the scrape files are staged only for the scrape pack"
        );
        assert_eq!(t.ran.borrow().last().unwrap().0, RemoteCmd::Cleanup);
    }

    #[test]
    fn a_server_without_apt_is_reported_as_unsupported_not_as_missing_tools() {
        let t = Fake::new(|_| ok("apt=0\nos_id=fedora\npack_office=installed\n"));
        let f = Packs { transport: &t }.facts().unwrap();
        assert!(!f.supported);
        assert_eq!(f.status("office"), PackStatus::Missing);
    }

    #[test]
    fn the_plan_is_a_dry_run_and_install_sends_only_the_catalog_id_and_checksum() {
        let t = Fake::new(|c| match c {
            RemoteCmd::Provision(Sub::PacksPlan) => ok(
                "pack=docs\naction=install\nmissing=pandoc,qpdf\nunavailable=\nnew_packages=31\n",
            ),
            _ => ok("pack_docs=installed\n"),
        });
        let p = Packs { transport: &t };
        let plan = p.plan("docs", Action::Install).unwrap();
        assert_eq!(plan.packages, ["pandoc", "qpdf"]);
        assert_eq!(plan.total_packages, 31);
        assert!(plan.blocked.is_none() && !plan.is_noop());
        // nothing but the unprivileged dry-run subcommand ran
        assert!(
            t.ran
                .borrow()
                .iter()
                .all(|(c, _)| !matches!(c, RemoteCmd::Provision(Sub::PackInstall)))
        );
        assert_eq!(
            plan.list_sha,
            list_sha("docs", Action::Install).unwrap(),
            "the plan carries the hash of the exact package list"
        );
        p.apply("docs", Action::Install, &plan.list_sha).unwrap();
        let ran = t.ran.borrow();
        let (cmd, stdin) = ran
            .iter()
            .find(|(c, _)| *c == RemoteCmd::Provision(Sub::PackInstall))
            .unwrap();
        assert!(
            cmd.command_line().contains("sudo -n"),
            "root only via the owner's login"
        );
        assert_eq!(
            *stdin,
            format!(
                "catalog={}\npack=docs\naction=install\nplan={}\n",
                catalog_sha(),
                list_sha("docs", Action::Install).unwrap()
            )
        );
    }

    #[test]
    fn free_text_is_refused_before_anything_reaches_the_server() {
        let t = Fake::new(|_| ok(""));
        let p = Packs { transport: &t };
        for bad in ["docs; rm -rf /", "pandoc", "../docs", ""] {
            assert_eq!(
                p.apply(bad, Action::Install, "0"),
                Err(PackError::UnknownPack)
            );
            assert!(p.plan(bad, Action::Install).is_err());
        }
        assert!(t.ran.borrow().is_empty() && t.uploaded.borrow().is_empty());
    }

    #[test]
    fn failures_are_classified_into_plain_reasons_and_redacted() {
        let fail = |stderr: &str| {
            let stderr = stderr.to_owned();
            Fake::new(move |_| Output {
                code: 1,
                stderr: stderr.clone(),
                ..Output::default()
            })
        };
        let run = |t: &Fake| Packs { transport: t }.apply("media", Action::Install, "0");
        assert_eq!(
            run(&fail("sudo: a password is required")),
            Err(PackError::NeedsAdmin)
        );
        assert_eq!(
            run(&fail(
                "[praxi-provision] error: not-available: only Debian and Ubuntu servers are supported"
            )),
            Err(PackError::Unsupported)
        );
        assert_eq!(
            run(&fail("error: not-available: package ffmpeg is not offered")),
            Err(PackError::Blocked(Blocked::Unavailable))
        );
        let e = run(&fail(
            "E: Unable to locate package
token=ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        ));
        let PackError::Failed(d) = e.unwrap_err() else {
            panic!("expected Failed")
        };
        assert!(d.contains("Unable to locate"), "{d}");
        assert!(
            !d.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
            "{d}"
        );
    }

    #[test]
    fn removing_shared_tools_or_a_pack_others_depend_on_is_blocked_in_the_plan() {
        let t = Fake::new(|_| ok("pack=dev\naction=remove\nblocked=shared\n"));
        let plan = Packs { transport: &t }.plan("dev", Action::Remove).unwrap();
        assert_eq!(plan.blocked, Some(Blocked::Shared));
        let t = Fake::new(|_| ok("pack=docs\naction=remove\nmissing=qpdf\nblocked=dependants\n"));
        let plan = Packs { transport: &t }
            .plan("docs", Action::Remove)
            .unwrap();
        assert_eq!(plan.blocked, Some(Blocked::Dependants));
    }

    /// The script itself (not just the Rust side) pins what runs as root.
    #[test]
    fn the_server_script_checks_the_catalog_checksum_and_never_purges() {
        let s = remote::provision_sh();
        assert!(s.contains("catalog_check") && s.contains("sha256sum"));
        assert!(s.contains("--no-install-recommends"));
        let from = s.find("# ---- capability packs").unwrap();
        let packs_part = &s[from..from + s[from..].find("case \"${1:-}\" in").unwrap()];
        for banned in [
            "purge",
            "autoremove",
            "eval ",
            "curl ",
            "wget ",
            "add-apt-repository",
        ] {
            assert!(!packs_part.contains(banned), "{banned}");
        }
    }

    #[test]
    fn the_list_hash_covers_pack_action_and_every_package() {
        let a = list_sha("docs", Action::Install).unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, list_sha("docs", Action::Remove).unwrap());
        assert_ne!(a, list_sha("media", Action::Install).unwrap());
        assert_eq!(list_sha("nope", Action::Install), None);
        // the server script hashes the same text: id, action and the single-spaced list
        let pkgs = catalog()
            .into_iter()
            .find(|p| p.id == "docs")
            .unwrap()
            .packages
            .join(" ");
        let text = format!(
            "docs
install
{pkgs}
"
        );
        assert_eq!(a, sha_hex(text.as_bytes()));
    }

    #[test]
    fn a_failed_removal_simulation_is_a_failure_not_an_all_clear() {
        // the script dies when `apt-get -s remove` itself errors (it used to treat that as "no
        // dependants"); the app reports it as a failed dry run and nothing is removed
        let t = Fake::new(|_| Output {
            code: 1,
            stderr: "[praxi-provision] error: could not check what removing this pack would affect"
                .into(),
            ..Output::default()
        });
        let p = Packs { transport: &t };
        assert!(matches!(
            p.plan("media", Action::Remove),
            Err(PackError::Failed(_))
        ));
        let sha = list_sha("media", Action::Remove).unwrap();
        assert!(matches!(
            p.apply("media", Action::Remove, &sha),
            Err(PackError::Failed(_))
        ));
    }

    #[test]
    fn every_pack_has_licence_facts_and_nothing_copyleft_is_pinned_from_upstream() {
        let cat = catalog();
        for p in &cat {
            let l = p.license.as_ref().unwrap_or_else(|| panic!("{} has no licence", p.id));
            assert!(!l.spdx.is_empty(), "{}", p.id);
            assert!(l.source_url.starts_with("https://"), "{}", p.id);
            assert!(l.notice_url.starts_with("https://"), "{}", p.id);
            // a pack Praxi pins and downloads from its author must say which version
            assert_eq!(l.distro || !l.version.is_empty(), true, "{}", p.id);
            // no strong-copyleft software is pulled from upstream by a pack today
            if !l.distro {
                assert!(!l.copyleft && !l.spdx.contains("GPL"), "{}", p.id);
                assert!(!p.needs_acknowledgement(), "{}", p.id);
            }
        }
        let scrape = cat.iter().find(|p| p.id == SCRAPE).unwrap();
        let l = scrape.license.as_ref().unwrap();
        assert_eq!((l.spdx.as_str(), l.distro), ("Apache-2.0", false));
        // the pinned environment is hash-locked and holds no GPL/AGPL project
        let lock = remote::scrape_lock();
        assert!(lock.contains("--require-hashes") || lock.contains("--hash=sha256:"));
        let pins = lock.lines().filter(|l| l.contains("==")).count();
        assert!(pins >= 5 && lock.matches("--hash=sha256:").count() >= pins);
        assert!(lock.contains(&format!("trafilatura=={}", l.version)));
        for banned in ["firecrawl", "scrapy", "mysql-connector", "pyqt", "pymupdf", "ghostscript"] {
            assert!(!lock.to_lowercase().contains(banned), "{banned}");
        }
        assert_eq!(scrape_lock_sha().len(), 64);
    }

    #[test]
    fn a_separate_copyleft_pack_cannot_be_installed_without_the_acknowledgement() {
        let text = "agpltool|AGPL-3.0-only|1|0|https://example.org/src|https://example.org/lic|1.2.3
                    distrotool|GPL-2.0-only|1|1|https://example.org/src|https://example.org/lic|
";
        let ls = parse_licenses(text);
        let mut pack = parse_catalog("agpltool|0|1|10|foo|foo
").remove(0);
        pack.license = Some(ls[0].1.clone());
        assert_eq!(pack.installable(false), Err(PackError::NeedsAcknowledgement));
        assert_eq!(pack.installable(true), Ok(()));
        // GPL software that the server's own distribution ships needs no extra step
        pack.license = Some(ls[1].1.clone());
        assert_eq!(pack.installable(false), Ok(()));
        // a pack with no licence record at all is never silently installable
        pack.license = None;
        assert_eq!(pack.installable(false), Err(PackError::NeedsAcknowledgement));
    }

    #[test]
    fn the_scrape_pack_stages_its_files_and_sends_their_checksums_only_for_itself() {
        let t = Fake::new(|c| match c {
            RemoteCmd::Provision(Sub::PacksPlan) => ok("pack=scrape
action=install
missing=praxi-scrape
unavailable=
"),
            _ => ok("pack_scrape=installed
"),
        });
        let p = Packs { transport: &t };
        let plan = p.plan(SCRAPE, Action::Install).unwrap();
        assert_eq!(plan.packages, ["praxi-scrape"]);
        p.apply(SCRAPE, Action::Install, &plan.list_sha).unwrap();
        let up = t.uploaded.borrow();
        for f in [StageFile::ScrapeService, StageFile::ScrapeLock, StageFile::ScrapeUnit] {
            assert!(up.contains(&f), "{f:?}");
        }
        let ran = t.ran.borrow();
        let (_, stdin) = ran
            .iter()
            .find(|(c, _)| *c == RemoteCmd::Provision(Sub::PackInstall))
            .unwrap();
        for key in ["scrape_service=", "scrape_lock=", "scrape_unit="] {
            assert!(stdin.contains(key), "{key}");
        }
        // a python-less server is explained, not reported as "not offered by the software sources"
        let t = Fake::new(|_| ok("pack=scrape
action=install
missing=
unavailable=python3-venv
"));
        let plan = Packs { transport: &t }.plan(SCRAPE, Action::Install).unwrap();
        assert_eq!(plan.blocked, Some(Blocked::NeedsPython));
    }

    #[test]
    fn the_scrape_unit_is_hardened_and_the_script_verifies_hashes_and_never_runs_as_root_service() {
        let unit = remote::scrape_unit();
        for need in [
            "NoNewPrivileges=yes",
            "ProtectSystem=strict",
            "ProtectHome=yes",
            "PrivateTmp=yes",
            "MemoryMax=",
            "TasksMax=",
            "IPAddressDeny=",
            "IPAddressAllow=localhost",
            "DynamicUser=yes",
            "LoadCredential=token:/etc/praxi/scrape.token",
        ] {
            assert!(unit.contains(need), "{need}");
        }
        let sh = remote::provision_sh();
        assert!(sh.contains("--require-hashes") && sh.contains("--only-binary=:all:"));
        assert!(sh.contains("scrape_verify_staged"));
        let svc = remote::scrape_service_py();
        assert!(svc.contains("BIND_HOST = \"127.0.0.1\"") && svc.contains("X-Praxi-Token"));
    }
}
