//! Hermetic installer: checks durable audit at process entry, installs only itself.
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--version"] {
        let bin = std::env::current_exe().unwrap();
        let version = if bin.parent().unwrap().join("installed").exists() {
            "9.9.9"
        } else {
            "1.0.0"
        };
        println!("{version} (Fixture)");
        return;
    }
    if args.first().is_some_and(|arg| arg != "install") {
        println!("{{\"loggedIn\":false,\"authenticated\":false}}");
        return;
    }
    assert_eq!(args, ["install", "-g", "opencode-ai"]);
    let storage = storage::Storage::open(std::env::current_dir().unwrap()).unwrap();
    let user = storage.list_users().unwrap()[0].id;
    let audit = storage.list_audit(user).unwrap();
    assert_eq!(audit.last().unwrap().action, "provider.cli_install.started");
    let executable = std::env::current_exe().unwrap();
    let bin = executable.parent().unwrap();
    std::fs::write(bin.join("audit-observed-before-install"), b"yes").unwrap();
    std::fs::write(bin.join("argv.json"), serde_json::to_vec(&args).unwrap()).unwrap();
    std::fs::write(bin.join("installed"), b"yes").unwrap();
    println!("<script>fixture installer output</script>");
}
