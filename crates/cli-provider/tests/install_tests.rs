use cli_provider::{PathEnv, install::run_install};
use std::time::Duration;
mod common;

fn fixture(mode: &str) -> (tempfile::TempDir, PathEnv) {
    let binary = std::path::Path::new(env!("CARGO_BIN_EXE_fake-cli"));
    let dir = common::fixture_dir(binary);
    common::install_binary(
        binary,
        &dir.path()
            .join(format!("npm{}", std::env::consts::EXE_SUFFIX)),
    );
    let env = PathEnv::for_test(dir.path().into())
        .with_env_var("FAKE_INSTALL_LOG", dir.path().to_string_lossy())
        .with_env_var("FAKE_INSTALL_MODE", mode)
        .with_env_var("PRAXI_PASSWORD", "MUST_NOT_ESCAPE")
        .with_env_var("DATABASE_URL", "MUST_NOT_ESCAPE")
        .with_env_var("NODE_OPTIONS", "MUST_NOT_ESCAPE")
        .with_env_var("BASH_ENV", "MUST_NOT_ESCAPE")
        .with_env_var("HTTPS_PROXY", "MUST_NOT_ESCAPE")
        .with_env_var("PRAXI_API_TOKEN", "MUST_NOT_ESCAPE");
    (dir, env)
}

#[tokio::test]
async fn installer_uses_registry_argv_streams_and_drops_non_allowlisted_environment() {
    let (dir, env) = fixture("");
    let mut output = String::new();
    run_install(
        "opencode",
        &env,
        dir.path(),
        Duration::from_secs(5),
        |text| output.push_str(&text),
    )
    .await
    .unwrap();
    let args: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("install-argv")).unwrap())
            .unwrap();
    assert_eq!(args, ["install", "-g", "opencode-ai"]);
    assert!(output.contains("fixture output"));
    let child_env = std::fs::read_to_string(dir.path().join("install-env")).unwrap();
    assert!(!child_env.contains("MUST_NOT_ESCAPE"));
    for key in [
        "NODE_OPTIONS",
        "BASH_ENV",
        "DATABASE_URL",
        "PRAXI_PASSWORD",
        "HTTPS_PROXY",
    ] {
        assert!(!child_env.contains(key));
    }
}

#[tokio::test]
async fn installer_times_out_and_caps_output_without_shells() {
    let (dir, env) = fixture("hang");
    assert_eq!(
        run_install(
            "claude",
            &env,
            dir.path(),
            Duration::from_millis(300),
            |_| {}
        )
        .await,
        Err("timeout")
    );
    // The exclusive create proves the owned fixture has exited on Windows too.
    std::fs::rename(
        dir.path()
            .join(format!("npm{}", std::env::consts::EXE_SUFFIX)),
        dir.path().join("stopped"),
    )
    .unwrap();
    let (dir, env) = fixture("flood");
    assert_eq!(
        run_install("codex", &env, dir.path(), Duration::from_secs(5), |_| {}).await,
        Err("output_limit")
    );
    let (dir, env) = fixture("fail");
    assert_eq!(
        run_install("codex", &env, dir.path(), Duration::from_secs(5), |_| {}).await,
        Err("install_failed")
    );
}
