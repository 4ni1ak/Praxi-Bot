//! Where the content of a file came from.
//!
//! An injected instruction can sit in any text the user did not write: a cloned repo's README,
//! a download, a mail attachment, a page a tool saved earlier. A pathname list alone cannot tell
//! these from the user's own documents (review of PR #138), so the file tools use PROVENANCE:
//!
//! 1. a persistent ledger ([`Provenance`], a table in the daemon's storage) of files and trees
//!    that Praxi's own tools wrote while the run held untrusted content, moved or copied from
//!    such a place, or fetched through the shell (`git clone`, `curl`, `wget`, `unzip` ...);
//! 2. locations that are external by rule (Downloads, temp dirs, browser and mail caches,
//!    package caches) and any git work tree that has a remote;
//! 3. a read or search is judged by what it returns, not by the folder it was asked about.
//!
//! Everything else is the user's own document and stays private. Not detected: a file the user
//! saved by hand from mail or the web into their documents, outside Praxi.

use std::path::{Path, PathBuf};

/// The persistent ledger of untrusted-origin paths. Paths are given already [`normalize`]d.
pub trait Provenance: Send + Sync {
    /// The path itself, or an ancestor directory marked as a tree, is recorded.
    fn is_marked(&self, normalized: &str) -> bool;
    /// Record a file, or with `tree` everything under a directory. Best effort.
    fn mark(&self, normalized: &str, tree: bool, origin: &str);
}

/// The ledger kept in the daemon's storage (table `file_provenance`).
pub struct StorageProvenance(pub std::sync::Arc<storage::Storage>);

impl Provenance for StorageProvenance {
    fn is_marked(&self, normalized: &str) -> bool {
        self.0.is_untrusted_path(normalized).unwrap_or(false)
    }
    fn mark(&self, normalized: &str, tree: bool, origin: &str) {
        let _ = self.0.mark_untrusted_path(normalized, tree, origin);
    }
}

/// `/` separators, no `\\?\` prefix, no trailing slash, lower case on Windows.
#[must_use]
pub fn normalize(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_owned();
    }
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    if cfg!(windows) {
        s = s.to_lowercase();
    }
    s
}

/// Locations whose content is external by rule: downloads, temp, browser / mail / package caches.
const EXTERNAL_PARTS: &[&str] = &[
    "/downloads/",
    "/download/",
    "/temp/",
    "/tmp/",
    "/attachments/",
    "/attachment/",
    "/quarantine/",
    "/inetcache/",
    "/google/chrome/",
    "/microsoft/edge/",
    "/bravesoftware/",
    "/mozilla/firefox/",
    "/.mozilla/",
    "/thunderbird/",
    "/content.outlook/",
    "/mail downloads/",
    "/.cache/",
    "/.npm/",
    "/node_modules/",
    "/.cargo/registry/",
    "/.cargo/git/",
    "/site-packages/",
];
const EXTERNAL_EXTENSIONS: &[&str] = &["eml", "mbox", "msg", "mhtml"];

/// Whether `path` lies in an external location by rule (see the module docs).
#[must_use]
pub fn external_location(path: &Path) -> bool {
    // rules match case-insensitively on every OS (`Downloads` on Linux too)
    let n = format!("{}/", normalize(path).to_lowercase());
    if EXTERNAL_PARTS.iter().any(|part| n.contains(part)) {
        return true;
    }
    if n.trim_end_matches('/')
        .rsplit_once('.')
        .is_some_and(|(_, ext)| EXTERNAL_EXTENSIONS.contains(&ext))
    {
        return true;
    }
    in_git_work_tree_with_remote(path)
}

/// A git work tree with a remote configured. A folder the owner explicitly trusts is a follow-up
/// (there is no such action yet), so every such tree counts: a README can carry injected text.
fn in_git_work_tree_with_remote(path: &Path) -> bool {
    let mut dir = Some(path);
    for _ in 0..16 {
        let Some(d) = dir else { return false };
        let git = d.join(".git");
        if git.is_file() {
            return true; // worktree / submodule pointer: its remote is elsewhere; be conservative
        }
        if git.is_dir() {
            return std::fs::read_to_string(git.join("config"))
                .is_ok_and(|c| c.contains("[remote \""));
        }
        dir = d.parent();
    }
    false
}

/// Untrusted by rule or by the ledger.
#[must_use]
pub fn is_untrusted(ledger: Option<&dyn Provenance>, path: &Path) -> bool {
    external_location(path) || ledger.is_some_and(|l| l.is_marked(&normalize(path)))
}

/// Directories too broad to mark as a tree (a drive root or the home directory).
fn too_broad(path: &Path) -> bool {
    path.parent().is_none()
        || std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .is_some_and(|h| normalize(Path::new(&h)) == normalize(path))
}

/// What a shell command line fetches from outside, as `(path, whole_tree)`: `git clone` / `pull`
/// / `fetch`, `curl` / `wget` / `Invoke-WebRequest` downloads, `unzip -d`, `tar -C`,
/// `Expand-Archive`. Relative targets resolve against `cwd`. Detection is by token, so a
/// command assembled at run time (variables, `eval`) is not seen.
#[must_use]
pub fn shell_fetch_targets(command: &str, cwd: &Path) -> Vec<(PathBuf, bool)> {
    let mut out = Vec::new();
    let resolve = |p: &str| -> PathBuf {
        let p = p.trim_matches(['"', '\'']);
        let path = Path::new(p);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        }
    };
    let name_of = |url: &str| -> Option<String> {
        let tail = url.trim_matches(['"', '\'']).split(['?', '#']).next()?;
        let seg = tail.trim_end_matches('/').rsplit(['/', ':']).next()?;
        let seg = seg.trim_end_matches(".git");
        (!seg.is_empty()).then(|| seg.to_owned())
    };
    for segment in command.split(['\n', ';', '|', '&']) {
        let t: Vec<&str> = segment.split_whitespace().collect();
        let Some(head) = t.first().map(|h| h.to_ascii_lowercase()) else {
            continue;
        };
        let after = |flags: &[&str]| -> Option<&str> {
            t.iter()
                .position(|w| flags.iter().any(|f| w.eq_ignore_ascii_case(f)))
                .and_then(|i| t.get(i + 1).copied())
        };
        let is_url = |w: &&str| w.contains("://") || w.starts_with("git@");
        match head.trim_end_matches(".exe") {
            "git" => {
                let (mut base, mut sub, mut i) = (cwd.to_path_buf(), None, 1);
                while i < t.len() {
                    let w = t[i];
                    if w == "-C" {
                        if let Some(d) = t.get(i + 1) {
                            base = resolve(d);
                        }
                        i += 2;
                    } else if w.starts_with('-') {
                        i += 1;
                    } else {
                        sub = Some((i, w.to_ascii_lowercase()));
                        break;
                    }
                }
                match sub {
                    Some((at, s)) if s == "clone" => {
                        let operands: Vec<&str> = t[at + 1..]
                            .iter()
                            .copied()
                            .filter(|w| !w.starts_with('-'))
                            .collect();
                        if let Some(u) = operands
                            .iter()
                            .position(|w| is_url(w) || w.contains('/') || w.contains('\\'))
                        {
                            let target = operands
                                .get(u + 1)
                                .map(|d| base.join(resolve(d)))
                                .or_else(|| name_of(operands[u]).map(|n| base.join(n)));
                            if let Some(target) = target {
                                out.push((target, true));
                            }
                        }
                    }
                    Some((_, s))
                        if matches!(
                            s.as_str(),
                            "pull" | "fetch" | "submodule" | "checkout" | "switch" | "merge"
                        ) =>
                    {
                        out.push((base, true));
                    }
                    _ => {}
                }
            }
            "curl" => {
                if let Some(f) = after(&["-o", "--output"]) {
                    out.push((resolve(f), false));
                } else if t.iter().any(|w| *w == "-O" || *w == "--remote-name")
                    && let Some(n) = t.iter().find(|w| is_url(w)).and_then(|u| name_of(u))
                {
                    out.push((cwd.join(n), false));
                }
            }
            "wget" => {
                if let Some(f) = after(&["-O", "--output-document"]) {
                    out.push((resolve(f), false));
                } else if let Some(d) = after(&["-P", "--directory-prefix"]) {
                    out.push((resolve(d), true));
                } else if let Some(n) = t.iter().find(|w| is_url(w)).and_then(|u| name_of(u)) {
                    out.push((cwd.join(n), false));
                }
            }
            "invoke-webrequest" | "iwr" | "invoke-restmethod" | "irm" => {
                if let Some(f) = after(&["-outfile"]) {
                    out.push((resolve(f), false));
                }
            }
            "unzip" => {
                if let Some(d) = after(&["-d"]) {
                    out.push((resolve(d), true));
                }
            }
            "tar" => {
                if t.iter().any(|w| {
                    w.starts_with('-') && w.contains('x')
                        || *w == "x"
                        || w.starts_with("xf")
                        || w.starts_with("xzf")
                }) {
                    out.push((
                        after(&["-C"]).map_or_else(|| cwd.to_path_buf(), resolve),
                        true,
                    ));
                }
            }
            "expand-archive" => {
                if let Some(d) = after(&["-destinationpath"]) {
                    out.push((resolve(d), true));
                }
            }
            _ => {}
        }
    }
    out.retain(|(p, tree)| !(*tree && too_broad(p)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[derive(Default)]
    struct Mem(Mutex<Vec<(String, bool)>>);
    impl Provenance for Mem {
        fn is_marked(&self, n: &str) -> bool {
            self.0
                .lock()
                .iter()
                .any(|(p, tree)| p == n || (*tree && n.starts_with(&format!("{p}/"))))
        }
        fn mark(&self, n: &str, tree: bool, _: &str) {
            self.0.lock().push((n.to_owned(), tree));
        }
    }

    #[test]
    fn external_by_rule_and_by_ledger_but_a_plain_document_is_private() {
        assert!(external_location(Path::new(r"C:\Users\a\Downloads\x.pdf")));
        assert!(external_location(Path::new(
            "/home/a/.cargo/registry/src/x/README.md"
        )));
        assert!(external_location(Path::new("/home/a/mail/msg.eml")));
        // (fake paths: a real temp dir is itself external by rule)
        let doc = Path::new("/home/a/Documents/notes.txt");
        assert!(!is_untrusted(None, doc));
        let ledger = Mem::default();
        ledger.mark(&normalize(Path::new("/home/a/Documents/saved")), true, "x");
        assert!(is_untrusted(
            Some(&ledger),
            Path::new("/home/a/Documents/saved/a.md")
        ));
        assert!(!is_untrusted(Some(&ledger), doc));
    }

    #[test]
    fn a_git_work_tree_with_a_remote_is_external_and_one_without_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(
            repo.join(".git").join("config"),
            "[core]
",
        )
        .unwrap();
        assert!(!in_git_work_tree_with_remote(&repo.join("README.md")));
        std::fs::write(
            repo.join(".git").join("config"),
            "[remote \"origin\"]
 url = x
",
        )
        .unwrap();
        assert!(in_git_work_tree_with_remote(
            &repo.join("sub").join("README.md")
        ));
    }

    #[test]
    fn shell_fetches_name_their_target() {
        let cwd = Path::new("/work");
        let t = |c: &str| -> Vec<(String, bool)> {
            shell_fetch_targets(c, cwd)
                .into_iter()
                .map(|(p, tree)| (normalize(&p), tree))
                .collect()
        };
        let one = |p: &str, tree: bool| vec![(p.to_owned(), tree)];
        assert_eq!(t("git clone https://x.test/a/b.git"), one("/work/b", true));
        assert_eq!(
            t("git clone https://x.test/a/b.git dst && ls"),
            one("/work/dst", true)
        );
        assert_eq!(
            t("curl -sL https://x.test/f.zip -o out/f.zip"),
            one("/work/out/f.zip", false)
        );
        assert_eq!(t("wget https://x.test/p/f.tgz"), one("/work/f.tgz", false));
        assert_eq!(t("unzip a.zip -d /work/u"), one("/work/u", true));
        assert_eq!(t("ls -la"), Vec::<(String, bool)>::new());
    }
}
