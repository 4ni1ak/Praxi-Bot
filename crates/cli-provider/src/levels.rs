//! Engine modes and approval levels for coding CLIs that run with their own tools.
//!
//! A bot either keeps Praxi in charge of files and shell (`praxi_governed`, the
//! confined engine) or hands them to the CLI (`native_tools`). In the second
//! mode the bot's approval level is mapped onto the CLI's own permission mode.
//! Nothing here spawns a process; [`level_plan`] is the one table the runtime,
//! the API and the UI read.

use serde::{Deserialize, Serialize};

/// Who runs the bot's file and shell actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineMode {
    /// Praxi's own tools and permission engine; the CLI's native tools are off.
    #[default]
    PraxiGoverned,
    /// The CLI's native tools are on and governed by the CLI's permission mode.
    NativeTools,
}

/// Per-bot approval level. The default is `Ask`; `Full` is an explicit owner choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalLevel {
    /// The CLI asks before it changes anything; each request is a Praxi approval card.
    #[default]
    Ask,
    /// File edits inside the workspace are accepted, everything else asks.
    AcceptEdits,
    /// The CLI's own automatic reviewer decides (only where the CLI has one).
    Auto,
    /// No approval prompts: the CLI runs in its most permissive mode.
    Full,
}

impl ApprovalLevel {
    pub const ALL: [Self; 4] = [Self::Ask, Self::AcceptEdits, Self::Auto, Self::Full];

    /// Width of the grant: a higher rank trusts the CLI with more.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Ask => 0,
            Self::AcceptEdits => 1,
            Self::Auto => 2,
            Self::Full => 3,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::AcceptEdits => "accept_edits",
            Self::Auto => "auto",
            Self::Full => "full",
        }
    }
}

impl std::str::FromStr for ApprovalLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|level| level.as_str() == s)
            .ok_or_else(|| format!("unknown approval level {s:?}"))
    }
}

impl EngineMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PraxiGoverned => "praxi_governed",
            Self::NativeTools => "native_tools",
        }
    }
}

impl std::str::FromStr for EngineMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "praxi_governed" => Ok(Self::PraxiGoverned),
            "native_tools" => Ok(Self::NativeTools),
            _ => Err(format!("unknown engine mode {s:?}")),
        }
    }
}

/// Drivers for native-tools mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Driver {
    Claude,
    Codex,
    Cursor,
    OpenCode,
    Gemini,
}

/// How far a driver's interface was checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// Flags and subcommands confirmed in the installed CLI's `--help`.
    Verified,
    /// Interface taken from the vendor reference only (CLI not installed here).
    /// Shown as experimental in the UI.
    Experimental,
}

/// Wire protocol the driver speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// `claude -p` with stream-json on both sides.
    StreamJson,
    /// `codex app-server` JSON-RPC.
    CodexAppServer,
    /// Agent Client Protocol over stdio.
    Acp,
}

impl Driver {
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Cursor,
        Self::OpenCode,
        Self::Gemini,
    ];

    /// Registry id (`claude`, `codex`, `cursor`, `opencode`, `gemini`).
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.id() == id)
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::OpenCode => "OpenCode",
            Self::Gemini => "Gemini CLI",
        }
    }

    #[must_use]
    pub const fn transport(self) -> Transport {
        match self {
            Self::Claude => Transport::StreamJson,
            Self::Codex => Transport::CodexAppServer,
            Self::Cursor | Self::OpenCode | Self::Gemini => Transport::Acp,
        }
    }

    /// Gemini CLI is not installed on the machine this was written on and its
    /// install command is unverified, so its driver ships as experimental.
    #[must_use]
    pub const fn support(self) -> Support {
        match self {
            Self::Gemini => Support::Experimental,
            _ => Support::Verified,
        }
    }
}

/// Codex thread settings for a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexPolicy {
    pub approval_policy: &'static str,
    pub reviewer: &'static str,
    /// `thread/start` `sandbox` value.
    pub sandbox: &'static str,
    /// `turn/start` `sandboxPolicy.type` value.
    pub sandbox_policy: &'static str,
}

/// What a bot's level means for one driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelPlan {
    pub requested: ApprovalLevel,
    /// The level actually applied: never wider than requested. A level the CLI
    /// has no mode for runs as `Ask`.
    pub applied: ApprovalLevel,
    pub offered: bool,
    /// The CLI's own setting, as shown to the owner.
    pub native: &'static str,
    /// Claude `--permission-mode` value (`default` is renamed `manual` by newer CLIs).
    pub claude_mode: Option<&'static str>,
    pub codex: Option<CodexPolicy>,
    /// Global flags placed before the ACP subcommand (or after `--acp`).
    pub flags: &'static [&'static str],
    /// ACP: accept every forwarded permission request without a card.
    pub acp_allow_all: bool,
}

const fn plan(requested: ApprovalLevel, applied: ApprovalLevel, native: &'static str) -> LevelPlan {
    LevelPlan {
        requested,
        applied,
        offered: requested.rank() == applied.rank(),
        native,
        claude_mode: None,
        codex: None,
        flags: &[],
        acp_allow_all: false,
    }
}

/// The mapping table (the reference project's approval-level provider mappings,
/// adapted: the default is Ask and Full is never implied).
#[must_use]
pub fn level_plan(driver: Driver, level: ApprovalLevel) -> LevelPlan {
    use ApprovalLevel::{AcceptEdits, Ask, Auto, Full};
    match driver {
        Driver::Claude => {
            let (mode, native) = match level {
                Ask => ("default", "--permission-mode default"),
                AcceptEdits => ("acceptEdits", "--permission-mode acceptEdits"),
                Auto => ("auto", "--permission-mode auto"),
                Full => ("bypassPermissions", "--permission-mode bypassPermissions"),
            };
            LevelPlan {
                claude_mode: Some(mode),
                ..plan(level, level, native)
            }
        }
        Driver::Codex => {
            let (applied, native, codex) = match level {
                Ask | AcceptEdits => (
                    Ask,
                    "on-request approvals, workspace-write sandbox",
                    CodexPolicy {
                        approval_policy: "on-request",
                        reviewer: "user",
                        sandbox: "workspace-write",
                        sandbox_policy: "workspaceWrite",
                    },
                ),
                Auto => (
                    Auto,
                    "workspace-write sandbox, auto_review reviewer",
                    CodexPolicy {
                        approval_policy: "on-request",
                        reviewer: "auto_review",
                        sandbox: "workspace-write",
                        sandbox_policy: "workspaceWrite",
                    },
                ),
                Full => (
                    Full,
                    "approvals never, danger-full-access sandbox",
                    CodexPolicy {
                        approval_policy: "never",
                        reviewer: "user",
                        sandbox: "danger-full-access",
                        sandbox_policy: "dangerFullAccess",
                    },
                ),
            };
            LevelPlan {
                codex: Some(codex),
                ..plan(level, applied, native)
            }
        }
        Driver::Cursor => match level {
            Ask | AcceptEdits => plan(level, Ask, "default"),
            Auto => LevelPlan {
                flags: &["--auto-review"],
                ..plan(level, Auto, "--auto-review")
            },
            Full => LevelPlan {
                flags: &["--force"],
                ..plan(level, Full, "--force")
            },
        },
        Driver::OpenCode => match level {
            Ask | AcceptEdits | Auto => plan(level, Ask, "every permission request asks"),
            Full => LevelPlan {
                acp_allow_all: true,
                ..plan(level, Full, "permission requests approved automatically")
            },
        },
        Driver::Gemini => match level {
            Ask | Auto => plan(level, Ask, "default"),
            AcceptEdits => LevelPlan {
                flags: &["--approval-mode", "auto_edit"],
                ..plan(level, AcceptEdits, "--approval-mode auto_edit")
            },
            Full => LevelPlan {
                flags: &["--yolo"],
                acp_allow_all: true,
                ..plan(level, Full, "--yolo")
            },
        },
    }
}

/// Launch arguments of the ACP drivers (everything after the executable).
/// `None` for drivers that do not speak ACP. The prompt never appears here.
#[must_use]
pub fn acp_args(driver: Driver, plan: &LevelPlan, model: Option<&str>) -> Option<Vec<String>> {
    let own = |s: &[&str]| s.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    let mut args = Vec::new();
    match driver {
        Driver::Cursor => {
            // Global flags precede `acp` (cursor-agent --help).
            args.extend(own(plan.flags));
            if let Some(m) = model {
                args.extend(["--model".into(), m.to_owned()]);
            }
            args.push("acp".into());
        }
        Driver::OpenCode => args.push("acp".into()),
        Driver::Gemini => {
            args.push("--acp".into());
            args.extend(own(plan.flags));
            if let Some(m) = model {
                args.extend(["-m".into(), m.to_owned()]);
            }
        }
        Driver::Claude | Driver::Codex => return None,
    }
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::too_many_lines)] // One table, one test.
    fn level_mapping_per_cli() {
        use ApprovalLevel::{AcceptEdits, Ask, Auto, Full};
        // (driver, level) -> (applied level, native setting, ACP argv)
        let cases: &[(Driver, ApprovalLevel, ApprovalLevel, &str, &[&str])] = &[
            (Driver::Claude, Ask, Ask, "--permission-mode default", &[]),
            (
                Driver::Claude,
                AcceptEdits,
                AcceptEdits,
                "--permission-mode acceptEdits",
                &[],
            ),
            (Driver::Claude, Auto, Auto, "--permission-mode auto", &[]),
            (
                Driver::Claude,
                Full,
                Full,
                "--permission-mode bypassPermissions",
                &[],
            ),
            (
                Driver::Codex,
                AcceptEdits,
                Ask,
                "on-request approvals, workspace-write sandbox",
                &[],
            ),
            (
                Driver::Codex,
                Auto,
                Auto,
                "workspace-write sandbox, auto_review reviewer",
                &[],
            ),
            (
                Driver::Codex,
                Full,
                Full,
                "approvals never, danger-full-access sandbox",
                &[],
            ),
            (Driver::Cursor, Ask, Ask, "default", &["acp"]),
            (Driver::Cursor, AcceptEdits, Ask, "default", &["acp"]),
            (
                Driver::Cursor,
                Auto,
                Auto,
                "--auto-review",
                &["--auto-review", "acp"],
            ),
            (Driver::Cursor, Full, Full, "--force", &["--force", "acp"]),
            (
                Driver::OpenCode,
                Auto,
                Ask,
                "every permission request asks",
                &["acp"],
            ),
            (
                Driver::OpenCode,
                Full,
                Full,
                "permission requests approved automatically",
                &["acp"],
            ),
            (
                Driver::Gemini,
                AcceptEdits,
                AcceptEdits,
                "--approval-mode auto_edit",
                &["--acp", "--approval-mode", "auto_edit"],
            ),
            (Driver::Gemini, Auto, Ask, "default", &["--acp"]),
            (Driver::Gemini, Full, Full, "--yolo", &["--acp", "--yolo"]),
        ];
        for (driver, level, applied, native, argv) in cases {
            let p = level_plan(*driver, *level);
            assert_eq!(p.applied, *applied, "{driver:?} {level:?}");
            assert_eq!(p.native, *native, "{driver:?} {level:?}");
            assert_eq!(p.offered, level == applied);
            if let Some(got) = acp_args(*driver, &p, None) {
                assert_eq!(got, *argv, "{driver:?} {level:?}");
            }
        }
        // Nothing widens: an applied level is never wider than the requested one.
        for driver in Driver::ALL {
            for level in ApprovalLevel::ALL {
                assert!(level_plan(driver, level).applied.rank() <= level.rank());
            }
        }
        // Full access is never the default and only Full auto-allows ACP requests.
        assert_eq!(ApprovalLevel::default(), Ask);
        for driver in Driver::ALL {
            for level in [Ask, AcceptEdits, Auto] {
                assert!(!level_plan(driver, level).acp_allow_all);
            }
        }
        assert_eq!(
            acp_args(
                Driver::Cursor,
                &level_plan(Driver::Cursor, Ask),
                Some("auto")
            )
            .unwrap(),
            ["--model", "auto", "acp"]
        );
        assert_eq!(Driver::Gemini.support(), Support::Experimental);
    }
}
