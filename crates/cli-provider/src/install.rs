//! Fixed-argv installers. Authorization and audit must be completed by the host
//! before calling this module. This is never registered as an agent tool.
use crate::{
    PathEnv,
    detect::{
        ChildTree, ResolvedCommand, apply_sanitized_env, find_candidate, isolate, resolve_spawn,
    },
};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::AsyncReadExt;

pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
const OUTPUT_LIMIT: usize = 64 * 1024;

/// Resolve npm without ever handing a batch script to cmd.exe.
///
/// # Errors
/// Refuses missing runtimes and shims that cannot be resolved safely.
pub fn resolve_installer(env: &PathEnv) -> Result<ResolvedCommand, &'static str> {
    let candidate = find_candidate("npm", env).ok_or("npm_missing")?;
    if candidate
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd"))
    {
        let dir = candidate.parent().ok_or("npm_missing")?;
        let root = dir.join("node_modules/npm");
        let script =
            std::fs::canonicalize(root.join("bin/npm-cli.js")).map_err(|_| "npm_shim_refused")?;
        let root = std::fs::canonicalize(root).map_err(|_| "npm_shim_refused")?;
        if !script.starts_with(&root) || !crate::search::trusted_candidate(&script) {
            return Err("npm_shim_refused");
        }
        let node = if dir.join("node.exe").is_file() {
            dir.join("node.exe")
        } else {
            find_candidate("node", env).ok_or("node_missing")?
        };
        if !node
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
            || !crate::search::trusted_candidate(&node)
        {
            return Err("node_refused");
        }
        return Ok(ResolvedCommand {
            program: node,
            args: vec![script.to_string_lossy().into_owned()],
        });
    }
    resolve_spawn(&candidate, env).map_err(|_| "npm_shim_refused")
}

/// Only fixed registry argv is accepted. No command argument is supplied by a client.
///
/// # Errors
/// Returns a stable code for an unsupported installer, spawn failure, timeout,
/// oversized output or a nonzero installer exit.
pub async fn run_install(
    id: &str,
    env: &PathEnv,
    cwd: &Path,
    timeout: Duration,
    output: impl FnMut(String) + Send,
) -> Result<(), &'static str> {
    let install = crate::engine_spec(id)
        .and_then(super::registry::CliEngineSpec::install)
        .filter(|i| i.verified && i.executable)
        .ok_or("copy_only")?;
    let resolved = match install.program {
        "npm" => resolve_installer(env)?,
        "powershell.exe" => {
            let root = env
                .env_vars
                .get("SYSTEMROOT")
                .or_else(|| env.env_vars.get("SystemRoot"))
                .ok_or("powershell_missing")?;
            let program = Path::new(root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
            if !program.is_absolute()
                || !program.is_file()
                || !crate::search::trusted_candidate(&program)
            {
                return Err("powershell_missing");
            }
            ResolvedCommand {
                program,
                args: Vec::new(),
            }
        }
        "sh" => {
            let program = find_candidate("sh", env).ok_or("shell_missing")?;
            if !matches!(
                program.to_str(),
                Some(
                    "/bin/sh"
                        | "/usr/bin/sh"
                        | "/usr/bin/dash"
                        | "/bin/dash"
                        | "/bin/bash"
                        | "/usr/bin/bash"
                )
            ) {
                return Err("shell_refused");
            }
            ResolvedCommand {
                program,
                args: Vec::new(),
            }
        }
        _ => return Err("installer_refused"),
    };
    run(
        &resolved,
        install.args,
        env,
        cwd,
        timeout.min(INSTALL_TIMEOUT),
        output,
    )
    .await
}

async fn run(
    resolved: &ResolvedCommand,
    args: &[&str],
    env: &PathEnv,
    cwd: &Path,
    timeout: Duration,
    mut output: impl FnMut(String) + Send,
) -> Result<(), &'static str> {
    let mut cmd = tokio::process::Command::new(&resolved.program);
    cmd.args(&resolved.args)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_sanitized_env(&mut cmd, env);
    cmd.env("NO_COLOR", "1")
        .env("npm_config_update_notifier", "false");
    #[cfg(windows)]
    let mut wrap = isolate(cmd, crate::detect::SharedJob::new());
    #[cfg(not(windows))]
    let mut wrap = isolate(cmd);
    let mut child = ChildTree::new(wrap.spawn().map_err(|_| "spawn_failed")?);
    let mut stdout = child.stdout().take().ok_or("stdout_missing")?;
    let mut stderr = child.stderr().take().ok_or("stderr_missing")?;
    let result = tokio::time::timeout(timeout, async {
        let mut bytes = 0;
        let (mut out_done, mut err_done) = (false, false);
        let mut out = [0; 2048];
        let mut err = [0; 2048];
        let mut out_line = Vec::new();
        let mut err_line = Vec::new();
        loop {
            let (n, chunk, line) = tokio::select! {
                n = stdout.read(&mut out), if !out_done => {
                    let n = n.map_err(|_| "read_failed")?;
                    out_done = n == 0; (n, &out[..n], &mut out_line)
                },
                n = stderr.read(&mut err), if !err_done => {
                    let n = n.map_err(|_| "read_failed")?;
                    err_done = n == 0; (n, &err[..n], &mut err_line)
                },
                status = child.wait(), if out_done && err_done => {
                    return if status.map_err(|_| "wait_failed")?.success() { Ok(()) } else { Err("install_failed") };
                }
            };
            bytes += n;
            if bytes > OUTPUT_LIMIT { return Err("output_limit"); }
            emit_lines(line, chunk, n == 0, &mut output);
        }
    }).await.unwrap_or(Err("timeout"));
    child.terminate().await;
    result
}

fn emit_lines(pending: &mut Vec<u8>, chunk: &[u8], eof: bool, output: &mut impl FnMut(String)) {
    pending.extend_from_slice(chunk);
    while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
        let line: Vec<_> = pending.drain(..=end).collect();
        output(safe_output(&line));
    }
    if eof && !pending.is_empty() {
        output(safe_output(pending));
        pending.clear();
    }
}

fn safe_output(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lower = text.to_ascii_lowercase();
    if [
        "token", "password", "secret", "_auth", "api_key", "https://", "http://", "bearer ",
    ]
    .iter()
    .any(|s| lower.contains(s))
    {
        return "[installer output redacted]\n".to_owned();
    }
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn unverified_commands_cannot_execute() {
        let dir = tempfile::tempdir().unwrap();
        let env = PathEnv::for_test(dir.path().into());
        for id in ["gemini", "chatgpt-plan", "npm;whoami"] {
            assert_eq!(
                run_install(id, &env, dir.path(), INSTALL_TIMEOUT, |_| {}).await,
                Err("copy_only")
            );
        }
    }
    #[test]
    fn output_is_untrusted_and_bounded_by_runner() {
        assert_eq!(
            safe_output(b"token=secret"),
            "[installer output redacted]\n"
        );
        assert!(!safe_output(b"\x1b[0mhello\n").contains('\x1b'));
        let mut pending = Vec::new();
        let mut output = String::new();
        emit_lines(&mut pending, b"se", false, &mut |s| output.push_str(&s));
        emit_lines(&mut pending, b"cret=value\n", false, &mut |s| {
            output.push_str(&s);
        });
        assert_eq!(output, "[installer output redacted]\n");
    }
}
