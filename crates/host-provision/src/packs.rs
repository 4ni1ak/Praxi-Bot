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

const SHORT: Duration = Duration::from_secs(60);
/// A full office pack download can be slow on a small server.
const LONG: Duration = Duration::from_secs(1800);

/// One catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
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

/// The catalog this build ships.
pub fn catalog() -> Vec<Pack> {
    parse_catalog(&remote::packs_txt())
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
            Self::Connection(_) => "connection",
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
    fn stage(&self) -> Result<(), PackError> {
        self.transport
            .upload(StageFile::Provision, remote::provision_sh().as_bytes())
            .and_then(|()| {
                self.transport
                    .upload(StageFile::Packs, remote::packs_txt().as_bytes())
            })
            .map_err(|e| conn(&e))
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
        }
        if let Some(sha) = approved {
            stdin.push_str(&format!("plan={sha}\n"));
        }
        self.stage()?;
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
        assert_eq!(ids, ["office", "docs", "ocr", "media", "dev"]);
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
            [StageFile::Provision, StageFile::Packs]
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
}
