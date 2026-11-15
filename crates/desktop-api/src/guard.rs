//! Who may receive keyboard input: process identity, the window the input would
//! reach, and the pure rules that decide whether a run-scoped control grant may
//! cover that window (card 243-1).
//!
//! Nothing here talks to the OS. A backend fills an [`InputTarget`] from its own
//! window and process APIs; these functions only judge it. Every check fails
//! closed: a missing fact is a refusal, never a pass.

use serde::{Deserialize, Serialize};

/// One running program: image path, process id and process start time.
///
/// The start time is part of the identity so a recycled pid is a different
/// program. `pid == 0`, an empty path or `start_time == 0` is "unknown" and
/// never equal to anything.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    /// Full path of the executable image.
    pub image_path: String,
    /// Process id.
    pub pid: u32,
    /// Process creation time in OS ticks (Windows: FILETIME, 100 ns).
    pub start_time: u64,
}

impl ProcessIdentity {
    /// All three facts are known.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.pid != 0 && self.start_time != 0 && !self.image_path.trim().is_empty()
    }

    /// Same running program as `other`. Incomplete identities never match.
    /// The image path is compared without regard to case (Windows paths).
    #[must_use]
    pub fn same_program(&self, other: &Self) -> bool {
        self.is_complete()
            && other.is_complete()
            && self.pid == other.pid
            && self.start_time == other.start_time
            && self.image_path.eq_ignore_ascii_case(&other.image_path)
    }

    /// File name of the image without directory and extension, lower case.
    #[must_use]
    pub fn program_stem(&self) -> String {
        program_stem(&self.image_path)
    }
}

/// File name stem of an image path (either separator), lower case.
#[must_use]
pub fn program_stem(image_path: &str) -> String {
    let name = image_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(image_path)
        .trim();
    let stem = name
        .rsplit_once('.')
        .map_or(name, |(stem, ext)| if ext.len() <= 4 { stem } else { name });
    stem.to_lowercase()
}

/// The window and program keyboard input would reach right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputTarget {
    /// Platform window id.
    pub window_id: u64,
    /// Window title.
    pub title: String,
    /// Window class (Windows) or equivalent; empty when unknown.
    pub class: String,
    /// The program that owns the window.
    pub identity: ProcessIdentity,
    /// Whether the program runs elevated. `None` = could not be read, which
    /// counts as elevated.
    pub elevated: Option<bool>,
    /// The focused element is a password field.
    pub password_focus: bool,
}

/// Condition the OS backend must verify immediately before it sends input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActGuard {
    /// The only program the input may reach.
    pub expect: ProcessIdentity,
}

/// Why a grant does not cover a window, or why the guard refused to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// The backend could not name the program (or the pid is reused/unknown).
    NoIdentity,
    /// Elevated, or the elevation could not be read.
    Elevated,
    /// Terminal, console or shell.
    Terminal,
    /// An editor or tool with a built-in terminal or command line.
    CommandApp,
    /// A browser driven as a desktop app.
    Browser,
    /// A password manager.
    PasswordManager,
    /// A system shell, launcher or credential surface.
    SystemSurface,
    /// A packaged-app frame host that cannot be bound to one program.
    UwpFrame,
    /// A dialog (file dialogs, the Run dialog, message boxes).
    Dialog,
    /// The focused element is a password field.
    PasswordField,
    /// Keys that open a launcher or switch windows (Win/Super, Ctrl+Esc, ...).
    Chord,
    /// The window that would receive input is not the granted program.
    ForegroundChanged,
}

impl Refusal {
    /// Stable code for cards and audit.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::NoIdentity => "no_identity",
            Self::Elevated => "elevated",
            Self::Terminal => "terminal",
            Self::CommandApp => "command_app",
            Self::Browser => "browser",
            Self::PasswordManager => "password_manager",
            Self::SystemSurface => "system_surface",
            Self::UwpFrame => "uwp_frame",
            Self::Dialog => "dialog",
            Self::PasswordField => "password_field",
            Self::Chord => "chord",
            Self::ForegroundChanged => "foreground_changed",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

const TERMINALS: &[&str] = &[
    "cmd",
    "powershell",
    "powershell_ise",
    "pwsh",
    "windowsterminal",
    "wt",
    "conhost",
    "openconsole",
    "wsl",
    "wslhost",
    "bash",
    "sh",
    "zsh",
    "fish",
    "mintty",
    "alacritty",
    "wezterm",
    "wezterm-gui",
    "kitty",
    "hyper",
    "tabby",
    "putty",
    "kitty",
    "gnome-terminal",
    "konsole",
    "xterm",
    "terminal",
    "iterm2",
    "ConEmu64",
    "conemu",
];

const COMMAND_APPS: &[&str] = &[
    "code",
    "code - insiders",
    "codium",
    "vscodium",
    "cursor",
    "windsurf",
    "devenv",
    "idea",
    "idea64",
    "pycharm",
    "pycharm64",
    "webstorm",
    "webstorm64",
    "clion",
    "clion64",
    "rider",
    "rider64",
    "goland",
    "goland64",
    "rubymine",
    "rubymine64",
    "phpstorm",
    "phpstorm64",
    "datagrip",
    "datagrip64",
    "studio",
    "studio64",
    "explorer",
    "taskmgr",
    "mmc",
    "regedit",
    "taskschd",
    "msconfig",
    "control",
];

const BROWSERS: &[&str] = &[
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "opera",
    "vivaldi",
    "iexplore",
    "chromium",
    "arc",
    "safari",
    "tor",
    "waterfox",
    "librewolf",
];

const PASSWORD_MANAGERS: &[&str] = &[
    "1password",
    "keepass",
    "keepassxc",
    "bitwarden",
    "lastpass",
    "dashlane",
    "enpass",
    "nordpass",
    "roboform",
    "keeper",
];

const SYSTEM_SURFACES: &[&str] = &[
    "searchhost",
    "searchapp",
    "searchui",
    "startmenuexperiencehost",
    "shellexperiencehost",
    "shellhost",
    "lockapp",
    "logonui",
    "consent",
    "credentialuibroker",
    "securityhealthsystray",
    "winlogon",
    "dwm",
    "runtimebroker",
];

/// Refusal for a program by its image name, if it is on a deny list.
#[must_use]
pub fn program_refusal(image_path: &str) -> Option<Refusal> {
    let stem = program_stem(image_path);
    let listed = |list: &[&str]| list.iter().any(|n| n.eq_ignore_ascii_case(&stem));
    if listed(TERMINALS) {
        Some(Refusal::Terminal)
    } else if stem == "applicationframehost" {
        Some(Refusal::UwpFrame)
    } else if listed(SYSTEM_SURFACES) {
        Some(Refusal::SystemSurface)
    } else if listed(COMMAND_APPS) {
        Some(Refusal::CommandApp)
    } else if listed(BROWSERS) {
        Some(Refusal::Browser)
    } else if listed(PASSWORD_MANAGERS) {
        Some(Refusal::PasswordManager)
    } else {
        None
    }
}

/// Refusal for a window class: console windows are terminals, and the
/// standard dialog class (`#32770`, used by file dialogs, the Run dialog and
/// message boxes) is a dialog in any program.
#[must_use]
pub fn class_refusal(class: &str) -> Option<Refusal> {
    let c = class.trim().to_ascii_lowercase();
    match c.as_str() {
        "consolewindowclass"
        | "cascadia_hosting_window_class"
        | "pseudoconsolewindow"
        | "virtualconsoleclass" => Some(Refusal::Terminal),
        "#32770" => Some(Refusal::Dialog),
        _ => None,
    }
}

/// Whether the window itself may be covered by a control grant at all,
/// independent of which program was granted. `None` = it may.
#[must_use]
pub fn judge(target: &InputTarget) -> Option<Refusal> {
    if !target.identity.is_complete() {
        return Some(Refusal::NoIdentity);
    }
    if target.elevated != Some(false) {
        return Some(Refusal::Elevated);
    }
    if let Some(r) = program_refusal(&target.identity.image_path) {
        return Some(r);
    }
    if let Some(r) = class_refusal(&target.class) {
        return Some(r);
    }
    if target.password_focus {
        return Some(Refusal::PasswordField);
    }
    None
}

/// The recheck a backend runs right before it sends input: the window that
/// would receive it must be the granted program and must still pass [`judge`].
///
/// # Errors
/// The [`Refusal`] to report; the input must not be sent.
pub fn check_guard(guard: &ActGuard, now: &InputTarget) -> Result<(), Refusal> {
    if !guard.expect.same_program(&now.identity) {
        return Err(if now.identity.is_complete() {
            Refusal::ForegroundChanged
        } else {
            Refusal::NoIdentity
        });
    }
    judge(now).map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(path: &str, pid: u32, start: u64) -> ProcessIdentity {
        ProcessIdentity {
            image_path: path.into(),
            pid,
            start_time: start,
        }
    }

    fn target(path: &str, class: &str) -> InputTarget {
        InputTarget {
            window_id: 1,
            title: "t".into(),
            class: class.into(),
            identity: ident(path, 10, 99),
            elevated: Some(false),
            password_focus: false,
        }
    }

    #[test]
    fn a_normal_program_passes() {
        let t = target("C:\\Windows\\System32\\notepad.exe", "Notepad");
        assert_eq!(judge(&t), None);
        let g = ActGuard {
            expect: t.identity.clone(),
        };
        assert_eq!(check_guard(&g, &t), Ok(()));
    }

    #[test]
    fn same_pid_with_another_start_time_is_another_program() {
        let t = target("C:\\Windows\\notepad.exe", "Notepad");
        let g = ActGuard {
            expect: ident("C:\\Windows\\notepad.exe", 10, 98),
        };
        assert_eq!(check_guard(&g, &t), Err(Refusal::ForegroundChanged));
    }

    #[test]
    fn an_unknown_identity_never_matches() {
        let mut t = target("C:\\Windows\\notepad.exe", "Notepad");
        t.identity.start_time = 0;
        let g = ActGuard {
            expect: t.identity.clone(),
        };
        assert_eq!(check_guard(&g, &t), Err(Refusal::NoIdentity));
        assert_eq!(judge(&t), Some(Refusal::NoIdentity));
    }

    #[test]
    fn elevated_or_unreadable_elevation_is_refused() {
        let mut t = target("C:\\Windows\\notepad.exe", "Notepad");
        t.elevated = Some(true);
        assert_eq!(judge(&t), Some(Refusal::Elevated));
        t.elevated = None;
        assert_eq!(judge(&t), Some(Refusal::Elevated));
    }

    #[test]
    fn listed_programs_are_refused_by_kind() {
        for (path, want) in [
            ("C:\\Windows\\System32\\cmd.exe", Refusal::Terminal),
            (
                "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
                Refusal::Terminal,
            ),
            ("C:\\Windows\\explorer.exe", Refusal::CommandApp),
            ("C:\\x\\Code.exe", Refusal::CommandApp),
            ("C:\\x\\chrome.exe", Refusal::Browser),
            ("C:\\x\\KeePassXC.exe", Refusal::PasswordManager),
            (
                "C:\\Windows\\System32\\ApplicationFrameHost.exe",
                Refusal::UwpFrame,
            ),
            (
                "C:\\Windows\\SystemApps\\SearchHost.exe",
                Refusal::SystemSurface,
            ),
        ] {
            assert_eq!(program_refusal(path), Some(want), "{path}");
        }
        assert_eq!(program_refusal("C:\\Windows\\notepad.exe"), None);
        assert_eq!(program_refusal("/usr/bin/gedit"), None);
    }

    #[test]
    fn console_and_dialog_classes_are_refused_in_any_program() {
        let t = target("C:\\Windows\\notepad.exe", "ConsoleWindowClass");
        assert_eq!(judge(&t), Some(Refusal::Terminal));
        let t = target("C:\\Windows\\notepad.exe", "#32770");
        assert_eq!(judge(&t), Some(Refusal::Dialog));
    }

    #[test]
    fn a_password_field_is_refused() {
        let mut t = target("C:\\Windows\\notepad.exe", "Notepad");
        t.password_focus = true;
        assert_eq!(judge(&t), Some(Refusal::PasswordField));
    }

    #[test]
    fn the_stem_ignores_directories_and_extension() {
        assert_eq!(program_stem("C:\\A B\\Foo.EXE"), "foo");
        assert_eq!(program_stem("/usr/bin/bash"), "bash");
        assert_eq!(program_stem("Code - Insiders.exe"), "code - insiders");
    }
}
