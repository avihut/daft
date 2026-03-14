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
    // Auto-detect release builds by checking if HEAD is tagged with this version.
    let is_release = git_output(&["tag", "--points-at", "HEAD"])
        .map(|tags| {
            tags.lines()
                .any(|tag| tag.trim().trim_start_matches('v') == pkg_version)
        })
        .unwrap_or(false);

    let display_version = if is_release || std::env::var("DAFT_BUILD_RELEASE").is_ok() {
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

    // Emit cfg flag for dev builds so DAFT_CONFIG_DIR is only honored in dev.
    // A build is "dev" when it's not a release AND has a git repo (rules out crates.io installs).
    let has_git_repo = git_output(&["rev-parse", "--git-dir"]).is_some();
    if !is_release && has_git_repo {
        println!("cargo:rustc-cfg=daft_dev_build");
    }

    // Only re-run when HEAD changes (branch switch, new commit) or env changes.
    // Resolve the actual git dir — in worktrees `.git` is a file pointing elsewhere.
    if let Some(git_dir) = git_output(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        if let Some(head_ref) = git_output(&["symbolic-ref", "--quiet", "HEAD"]) {
            // HEAD points to a branch — watch that ref file for new commits.
            let ref_path = format!("{git_dir}/{head_ref}");
            if std::path::Path::new(&ref_path).exists() {
                println!("cargo:rerun-if-changed={ref_path}");
            }
        }
    }
    println!("cargo:rerun-if-env-changed=DAFT_BUILD_RELEASE");
}
