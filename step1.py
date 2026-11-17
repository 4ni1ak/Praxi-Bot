def sub(p, a, b, cnt=1):
    s = open(p, encoding="utf-8").read()
    assert s.count(a) == cnt, (p, a, s.count(a))
    open(p, "w", encoding="utf-8").write(s.replace(a, b))

p = "crates/host-provision/src/engine.rs"
sub(p, """    pub app_version: String,
    /// Install the virtual display""", """    pub app_version: String,
    /// The release tag this app was built from (`v0.1.0-alpha.1`), when the build knows it. A
    /// pre-release tag is not `v<app_version>`: the server must fetch the tag that really exists.
    pub release_tag: Option<String>,
    /// Install the virtual display""")
sub(p, """        let tag = format!("v{app}");
        self.ui.note(""", """        let tag = match install_tag(self.params.release_tag.as_deref(), &app) {
            Ok(tag) => tag,
            Err(e) => {
                let r = StepResult::fail(
                    "install",
                    T,
                    format!("cannot tell which release to install: {e}"),
                    "Update the desktop app to a published release.",
                );
                return self.finish(report, r);
            }
        };
        self.ui.note(""")
s = open(p, encoding="utf-8").read()
s = s.replace("fn detect_only(existing: &Existing, app: &str) -> StepResult {", """/// The GitHub release tag the server downloads: the tag the app was built from, else `v<version>`.
fn install_tag(release_tag: Option<&str>, app: &str) -> Result<String, crate::validate::Error> {
    crate::validate::version_tag(release_tag.filter(|t| !t.is_empty()).unwrap_or(app))
}

fn detect_only(existing: &Existing, app: &str) -> StepResult {""", 1)
s += """
#[cfg(test)]
mod install_tag_tests {
    use super::install_tag;

    #[test]
    fn a_pre_release_app_installs_its_own_tag_not_v_plus_the_cargo_version() {
        assert_eq!(install_tag(None, "0.1.0").unwrap(), "v0.1.0");
        assert_eq!(
            install_tag(Some("v0.1.0-alpha.1"), "0.1.0").unwrap(),
            "v0.1.0-alpha.1"
        );
        assert!(install_tag(Some("v0.1.0; rm -rf /"), "0.1.0").is_err());
    }
}
"""
open(p, "w", encoding="utf-8").write(s)
sub("crates/host-provision/src/engine_tests.rs", '            app_version: "0.1.0".into(),\n', '            app_version: "0.1.0".into(),\n            release_tag: None,\n')
sub("crates/host-provision/src/engine_tests.rs", '                app_version: "0.1.0".into(),\n', '                app_version: "0.1.0".into(),\n                release_tag: None,\n')
p = "apps/cli/src/host_provision.rs"
s = open(p, encoding="utf-8").read()
s = s.replace('app_version: env!("CARGO_PKG_VERSION").to_owned(),', 'app_version: env!("CARGO_PKG_VERSION").to_owned(),\n            release_tag: option_env!("PRAXI_RELEASE_TAG").map(str::to_owned),')
open(p, "w", encoding="utf-8").write(s)
sub(".github/workflows/installer.yml", """          cargo build --release -p cli
          New-Item""", """          # The release tag this build belongs to (v0.1.0-alpha.1): `bot host add` tells the server
          # which GitHub release to install; the Cargo version alone (0.1.0) is not that tag.
          if ($env:GITHUB_REF_TYPE -eq 'tag') { $env:PRAXI_RELEASE_TAG = $env:GITHUB_REF_NAME }
          cargo build --release -p cli
          New-Item""")
