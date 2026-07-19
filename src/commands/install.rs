use anyhow::Result;
use clap::Parser;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::core::install::{InstallOptions, install_at, propagate_starter_to_worktrees};
use crate::core::repo::WorktreePosition;
use crate::hooks::yaml_config_loader::{ConfigStatus, classify_main_config, find_config_file};
use crate::output::{CliOutput, Output, OutputConfig};
use crate::utils::get_current_directory;

#[derive(Parser)]
#[command(name = "daft-install")]
#[command(version = crate::VERSION)]
#[command(about = "Install a starter daft.yml in the current worktree")]
#[command(long_about = r#"
Creates a starter daft.yml at the worktree root with a commented skeleton
covering the major sections (hooks, shared, layout). Modeled on
`lefthook install`.

This is a top-level alias for `daft repo install` (the canonical name); both
run the same thing. The alias is kept so lefthook-style discovery works.

daft.yml is a per-worktree file, so install is repo-aware. Inside a worktree it
targets the worktree root (even from a subdirectory). At the bare container root
of a contained layout it installs across the repo's worktrees — writing the
starter into the default worktree and copying it into the others, like
`daft clone --install`. It refuses only outside a git repository. If a daft.yml
already exists it reports whether the file is tracked or a visitor config and
stops without modifying it.

After writing daft.yml, daft checks whether git already ignores it. If not, it
offers to add `/daft.yml` to .git/info/exclude — a local, per-clone exclude
that is never committed, so a visitor config stays invisible to teammates. On a
terminal it prompts (default No); --git-exclude adds it without prompting; a
non-interactive run only prints a hint and changes nothing. Without
--git-exclude, --quiet skips the check entirely. daft never touches the tracked
.gitignore.
"#)]
pub struct Args {
    #[arg(short = 'q', long = "quiet", help = "Suppress progress reporting")]
    quiet: bool,

    #[arg(short = 'v', long = "verbose", help = "Show detailed progress")]
    verbose: bool,

    #[arg(
        long = "git-exclude",
        help = "Add /daft.yml to .git/info/exclude without prompting (keeps it private to this clone)"
    )]
    git_exclude: bool,
}

pub fn run() -> Result<()> {
    // Read the `-C`-stripped argv (not `std::env::args()`): the top-level
    // `-C <path>` pair is consumed and applied by `cli::install_and_apply`
    // before dispatch, so the raw args still contain it and clap would reject
    // it. `crate::cli::argv()` has it removed. Skip argv[0] so clap sees
    // "install" as the program name and parses the rest. This mirrors the
    // canonical `repo::install::run()` and every sibling dispatcher (shared,
    // layout, repo::remove) — keeping `daft -C <dir> install` working.
    let args_raw: Vec<String> = crate::cli::argv().iter().skip(1).cloned().collect();
    let args = Args::parse_from(args_raw);
    let config = OutputConfig::new(args.quiet, args.verbose);
    let mut output = CliOutput::new(config);
    run_with_output(
        &mut output,
        InstallOptions {
            git_exclude: args.git_exclude,
        },
    )
}

pub fn run_with_output(output: &mut dyn Output, opts: InstallOptions) -> Result<()> {
    let cwd = get_current_directory()?;
    // Resolve interactivity at the boundary, not inside the offer logic. Reading
    // `is_terminal()` deeper down makes the offer untestable: a unit test run
    // from a real terminal inherits a TTY stdin and would block forever on
    // `dialoguer::Confirm`. Computing it here keeps the offer logic
    // deterministic for tests (which pass `interactive: false`).
    let interactive = std::io::stdin().is_terminal() && std::env::var("DAFT_TESTING").is_err();
    install_in_position(&cwd, output, &opts, interactive)
}

/// Repo-aware dispatch for `daft install` / `daft repo install`.
///
/// `daft.yml` is a per-worktree file that daft reads from a worktree root, so
/// install must first work out where `cwd` sits in the repo rather than writing
/// blindly to the current directory:
///
/// - **Not in a repo** → refuse (a stray `daft.yml` on the bare filesystem is
///   never read by daft).
/// - **Container root** of a contained layout → install across the repo's
///   worktrees, exactly like a multi-branch `daft clone --install`: write the
///   starter into the default worktree and copy it into the others. Never write
///   a stray `daft.yml` at the (inert) container root. If the repo is already
///   configured, report it and stop.
/// - **Inside a worktree** (including a nested subdir) → target the worktree
///   *root*. If it already has a `daft.yml`, don't overwrite or hard-error —
///   report whether it is tracked or a visitor config and stop.
///
/// `cwd` and `interactive` are injected (not read from the process) so the
/// whole dispatch is unit-testable without a TTY. The `daft clone --install`
/// path bypasses this entirely: it calls [`crate::core::install::install_at`]
/// with the freshly created worktree it already knows.
fn install_in_position(
    cwd: &Path,
    output: &mut dyn Output,
    opts: &InstallOptions,
    interactive: bool,
) -> Result<()> {
    match crate::core::repo::resolve_worktree_position(cwd) {
        WorktreePosition::NotInRepo => anyhow::bail!(
            "daft install must be run inside a git repository.\n\
             cd into a repo, or use `daft clone --install` to bootstrap one on clone."
        ),
        WorktreePosition::ContainerRoot { representative } => {
            install_at_container_root(representative, output, opts, interactive)
        }
        WorktreePosition::InWorktree { root } => {
            if let Some((existing, _location)) = find_config_file(&root) {
                guide_existing_config(&root, &existing, output);
                return Ok(());
            }
            install_at(&root, output, opts, interactive)
        }
    }
}

/// Install from the bare container root of a contained layout. The container
/// root is not a worktree (a `daft.yml` there is inert), so install into the
/// repo's worktrees instead — the same shape as a multi-branch
/// `daft clone --install`: write the starter into the default worktree and copy
/// it into the others.
fn install_at_container_root(
    representative: Option<PathBuf>,
    output: &mut dyn Output,
    opts: &InstallOptions,
    interactive: bool,
) -> Result<()> {
    let Some(primary) = representative else {
        anyhow::bail!(
            "This repository has no worktrees yet.\n\
             Create one (e.g. `daft start <branch>`) and run daft install there."
        );
    };

    // Already configured? Report it and stop, like `daft clone --install` skips.
    if find_config_file(&primary).is_some() {
        guide_existing_repo_config(&primary, output);
        return Ok(());
    }

    install_at(&primary, output, opts, interactive)?;
    propagate_starter_to_worktrees(&primary, output);
    Ok(())
}

/// Container-root case where the repo is already configured: report the
/// existing config's status and stop without changes. Kept to a single
/// parenthesis-free line plus a status line so it reads cleanly in a terminal.
fn guide_existing_repo_config(primary: &Path, output: &mut dyn Output) {
    output.result("daft.yml is already present in this repository — nothing to install.");
    match classify_main_config(primary) {
        ConfigStatus::Tracked => {
            output.info("The existing config is tracked — a committed team baseline.");
        }
        ConfigStatus::Visitor => {
            output.info(
                "The existing config is a visitor config — untracked, private to this clone.",
            );
        }
        ConfigStatus::Missing => {}
    }
}

/// Report an existing `daft.yml` instead of failing: state its tracking status
/// (tracked team baseline vs. untracked visitor config) and what to do next.
/// Phrased with em-dashes, not nested parentheses.
fn guide_existing_config(root: &Path, existing: &Path, output: &mut dyn Output) {
    let rel = existing.strip_prefix(root).unwrap_or(existing);
    match classify_main_config(root) {
        ConfigStatus::Tracked => {
            output.result(&format!(
                "{} already exists here and is tracked — a committed team baseline.",
                rel.display()
            ));
            output.info(
                "Nothing to install. For personal, uncommitted overrides, create daft.local.yml.",
            );
        }
        ConfigStatus::Visitor => {
            output.result(&format!(
                "{} already exists here and is a visitor config — untracked, private to this clone.",
                rel.display()
            ));
            output.info(
                "Nothing to install. Edit it directly, or commit it to share with your team.",
            );
        }
        // find_config_file located a file but classify reports Missing — only
        // reachable if it vanished between the two probes. Generic message.
        ConfigStatus::Missing => {
            output.result(&format!(
                "{} already exists here. Nothing to install.",
                rel.display()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::TestOutput;
    use crate::store::paths::IsolatedStateDir;
    use crate::utils::git_command_at;
    use serial_test::serial;
    use std::fs;
    use tempfile::tempdir;

    /// Run git in `dir` with a fixed identity (no global config — Rule #1).
    fn git_at(dir: &Path, args: &[&str]) {
        let out = git_command_at(dir)
            // A machine-global commit.gpgsign=true would make every fixture
            // commit hit the gpg agent — slow, and flaky under parallel load.
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn no_exclude() -> InstallOptions {
        InstallOptions { git_exclude: false }
    }

    #[test]
    fn test_install_in_position_refuses_outside_repo() {
        let dir = tempdir().unwrap();
        let mut output = TestOutput::new();
        let result = install_in_position(dir.path(), &mut output, &no_exclude(), false);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("inside a git repository")
        );
        assert!(
            !dir.path().join("daft.yml").exists(),
            "must not write a daft.yml outside a repo"
        );
    }

    #[test]
    fn test_install_in_position_writes_to_worktree_root_from_subdir() {
        let dir = tempdir().unwrap();
        git_at(dir.path(), &["init", "-q", "-b", "main"]);
        let sub = dir.path().join("nested/deep");
        fs::create_dir_all(&sub).unwrap();

        let mut output = TestOutput::new();
        install_in_position(&sub, &mut output, &no_exclude(), false).unwrap();

        assert!(
            dir.path().join("daft.yml").is_file(),
            "must write to the worktree root"
        );
        assert!(
            !sub.join("daft.yml").exists(),
            "must not write into the subdir"
        );
    }

    #[test]
    fn test_install_in_position_guides_on_existing_visitor() {
        let dir = tempdir().unwrap();
        git_at(dir.path(), &["init", "-q", "-b", "main"]);
        // Untracked daft.yml → visitor.
        fs::write(dir.path().join("daft.yml"), "hooks: {}\n").unwrap();

        let mut output = TestOutput::new();
        let result = install_in_position(dir.path(), &mut output, &no_exclude(), false);

        assert!(result.is_ok(), "existing config must not hard-error");
        assert!(
            output.has_result("visitor"),
            "expected a visitor guidance line, got: {:?}",
            output.results()
        );
    }

    #[test]
    fn test_install_in_position_guides_on_existing_tracked() {
        let dir = tempdir().unwrap();
        git_at(dir.path(), &["init", "-q", "-b", "main"]);
        fs::write(dir.path().join("daft.yml"), "hooks: {}\n").unwrap();
        git_at(dir.path(), &["add", "daft.yml"]);
        git_at(dir.path(), &["commit", "-q", "-m", "add"]);

        let mut output = TestOutput::new();
        let result = install_in_position(dir.path(), &mut output, &no_exclude(), false);

        assert!(result.is_ok());
        assert!(
            output.has_result("tracked"),
            "expected a tracked guidance line, got: {:?}",
            output.results()
        );
    }

    /// Build a contained-layout repo under `base`: `<base>/proj/.git` is bare,
    /// worktrees are subdirs. `branches[0]` is the default branch; when
    /// `with_config` a tracked daft.yml is committed on it. Returns the project
    /// (container) root.
    fn build_contained(base: &Path, with_config: bool, branches: &[&str]) -> PathBuf {
        let default = branches[0];
        let src = base.join("src");
        fs::create_dir_all(&src).unwrap();
        git_at(&src, &["init", "-q", "-b", default]);
        fs::write(src.join("README.md"), "hi").unwrap();
        if with_config {
            fs::write(src.join("daft.yml"), "hooks: {}\n").unwrap();
        }
        git_at(&src, &["add", "-A"]);
        git_at(&src, &["commit", "-q", "-m", "init"]);
        for b in &branches[1..] {
            git_at(&src, &["branch", b]);
        }

        let proj = base.join("proj");
        fs::create_dir_all(&proj).unwrap();
        git_at(
            base,
            &[
                "clone",
                "-q",
                "--bare",
                src.to_str().unwrap(),
                proj.join(".git").to_str().unwrap(),
            ],
        );
        git_at(
            &proj,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        );
        git_at(&proj, &["fetch", "-q", "origin"]);
        git_at(&proj, &["remote", "set-head", "origin", default]);
        for b in branches {
            git_at(&proj, &["worktree", "add", "-q", b, b]);
        }
        proj
    }

    #[test]
    fn test_install_in_position_container_root_skips_when_configured() {
        let base = tempdir().unwrap();
        let proj = build_contained(base.path(), true, &["main"]);

        let mut output = TestOutput::new();
        let result = install_in_position(&proj, &mut output, &no_exclude(), false);

        assert!(result.is_ok(), "already-configured repo must not error");
        assert!(
            output.has_result("already present"),
            "expected an already-present note, got: {:?}",
            output.results()
        );
        assert!(
            !proj.join("daft.yml").exists(),
            "must not write a stray daft.yml at the container root"
        );
    }

    #[test]
    #[serial]
    fn test_install_in_position_container_root_installs_across_worktrees() {
        // Isolate the state dir: a container-root install propagates visitor
        // seeds, which open the coordinator store via `paths::for_repo` and
        // would otherwise write the developer's real `~/.local/state/daft`
        // (#697 — the same isolation-leak class as #478/#669).
        let _state = IsolatedStateDir::new();
        let base = tempdir().unwrap();
        let proj = build_contained(base.path(), false, &["main", "feature"]);

        let mut output = TestOutput::new();
        let result = install_in_position(&proj, &mut output, &no_exclude(), false);

        assert!(result.is_ok(), "container-root install should succeed");
        assert!(
            proj.join("main/daft.yml").is_file(),
            "the default worktree must get a daft.yml"
        );
        assert!(
            proj.join("feature/daft.yml").is_file(),
            "sibling worktrees must get the daft.yml too (like clone --install)"
        );
        assert!(
            !proj.join("daft.yml").exists(),
            "must not write a stray daft.yml at the container root"
        );
    }
}
