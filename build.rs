use std::process::Command;

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn main() {
    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap();

    // DAFT_VERSION: always clean, used by clap attributes and man pages.
    println!("cargo:rustc-env=DAFT_VERSION={pkg_version}");

    // DAFT_VERSION_DISPLAY: includes branch/hash for dev builds, used by `daft --version`.
    let display_version = if std::env::var("DAFT_BUILD_RELEASE").is_ok() {
        pkg_version
    } else {
        let hash = git_output(&["rev-parse", "--short", "HEAD"]);
        let branch = git_output(&["rev-parse", "--abbrev-ref", "HEAD"]);

        match (branch, hash) {
            (Some(b), Some(h)) => format!("{pkg_version} (dev {b} {h})"),
            (None, Some(h)) => format!("{pkg_version} (dev {h})"),
            _ => pkg_version,
        }
    };

    println!("cargo:rustc-env=DAFT_VERSION_DISPLAY={display_version}");

    // Only re-run when HEAD changes (branch switch, new commit)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-env-changed=DAFT_BUILD_RELEASE");
}
