//! The scripts that ship inside the app and are uploaded to the server.
//! Line endings are normalised to LF so a Windows checkout cannot break them.

fn lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

pub fn provision_sh() -> String {
    lf(include_str!("../scripts/provision.sh"))
}

pub fn installer_sh() -> String {
    lf(include_str!("../../../scripts/install-linux.sh"))
}

pub fn setup_headless_sh() -> String {
    lf(include_str!("../../../scripts/linux/setup-headless.sh"))
}
