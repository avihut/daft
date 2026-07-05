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
    // The release pipeline sets DAFT_BUILD_RELEASE (release.yml); the tag check
    // keeps the string clean for ad-hoc builds at a tagged HEAD. Display only —
    // it must never gate behavior (see the dev-build cfg below).
    let is_release_build = std::env::var_os("DAFT_BUILD_RELEASE").is_some();
    let is_tagged_release = git_output(&["tag", "--points-at", "HEAD"])
        .map(|tags| {
            tags.lines()
                .any(|tag| tag.trim().trim_start_matches('v') == pkg_version)
        })
        .unwrap_or(false);

    let display_version = if is_tagged_release || is_release_build {
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
    // A build is "dev" when it comes from a git checkout (rules out crates.io
    // installs) and the release pipeline hasn't said otherwise via
    // DAFT_BUILD_RELEASE. Deliberately NOT gated on is_tagged_release: tag
    // proximity made every local build at a freshly tagged release commit
    // silently drop the DAFT_*_DIR overrides — failing the pre-push unit suite
    // and pointing test state at the real ~/.local/state/daft (#669).
    let has_git_repo = git_output(&["rev-parse", "--git-dir"]).is_some();
    if !is_release_build && has_git_repo {
        println!("cargo:rustc-cfg=daft_dev_build");
    }

    // Only re-run when HEAD changes (branch switch, new commit) or env changes.
    //
    // In worktrees, `--git-dir` returns a worktree-specific path (e.g.,
    // `.git/worktrees/<name>/`) while loose refs and tags live in the common
    // git dir (`.git/`). We must watch both:
    //   - `{git_dir}/HEAD` — changes when the worktree switches branches
    //   - `{common_dir}/{head_ref}` — changes when a new commit is made
    //   - `{common_dir}/packed-refs` — changes when tags are packed/updated
    if let Some(git_dir) = git_output(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");

        let common_dir =
            git_output(&["rev-parse", "--git-common-dir"]).unwrap_or_else(|| git_dir.clone());

        if let Some(head_ref) = git_output(&["symbolic-ref", "--quiet", "HEAD"]) {
            // Watch the loose ref in the common dir (where refs actually live).
            let ref_path = format!("{common_dir}/{head_ref}");
            if std::path::Path::new(&ref_path).exists() {
                println!("cargo:rerun-if-changed={ref_path}");
            }
        }

        // Watch packed-refs for tag changes (tags are often packed).
        let packed_refs = format!("{common_dir}/packed-refs");
        if std::path::Path::new(&packed_refs).exists() {
            println!("cargo:rerun-if-changed={packed_refs}");
        }
    }
    println!("cargo:rerun-if-env-changed=DAFT_BUILD_RELEASE");
}
