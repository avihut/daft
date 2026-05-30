use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::output::{CliOutput, Output, OutputConfig};
use crate::utils::{get_current_directory, git_command_at};

#[derive(Parser)]
#[command(name = "daft-install")]
#[command(version = crate::VERSION)]
#[command(about = "Install a starter daft.yml in the current worktree")]
#[command(long_about = r#"
Creates a starter daft.yml at the current worktree root with a commented
skeleton covering the major sections (hooks, shared, layout). Modeled on
`lefthook install`.

This is a top-level alias for `daft repo install` (the canonical name); both
run the same thing. The alias is kept so lefthook-style discovery works.

If daft.yml already exists, the command refuses without modifying anything;
edit the existing file with your editor or a future `daft config` TUI.

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

/// Behavioural options shared by `daft install` and `daft repo install`.
pub struct InstallOptions {
    /// When true, add `/daft.yml` to `.git/info/exclude` without prompting.
    pub git_exclude: bool,
}

const STARTER_TEMPLATE: &str = include_str!("install/starter.yml");

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
    install_at(&cwd, output, &opts, interactive)
}

/// Install a starter daft.yml at `worktree_root`, then — when it would be
/// visible to git — offer to exclude it (prompt when `interactive`, else a
/// hint). Shared entry point: `daft install`/`daft repo install` call it with
/// the cwd and computed interactivity; `daft clone --install` calls it with the
/// freshly-created worktree (see `commands::clone`).
pub fn install_at(
    worktree_root: &Path,
    output: &mut dyn Output,
    opts: &InstallOptions,
    interactive: bool,
) -> Result<()> {
    install_starter(worktree_root, output)?;
    maybe_offer_git_exclude(worktree_root, output, opts, interactive)?;
    Ok(())
}

pub fn install_starter(worktree_root: &Path, output: &mut dyn Output) -> Result<()> {
    let target = worktree_root.join("daft.yml");
    if target.exists() {
        anyhow::bail!(
            "daft.yml already exists at {}. Edit it directly with your editor.",
            target.display()
        );
    }
    fs::write(&target, STARTER_TEMPLATE)
        .with_context(|| format!("Failed to write {}", target.display()))?;

    output.result(&format!("Installed daft.yml at {}", target.display()));
    Ok(())
}

/// The pattern we add for a visitor `daft.yml`. The leading slash anchors it to
/// the worktree root — where `install` writes the file — so it never matches a
/// nested `daft.yml` elsewhere in the tree.
const EXCLUDE_PATTERN: &str = "/daft.yml";

/// How git currently sees a path within a work tree, for the purpose of
/// deciding whether to offer to exclude it.
#[derive(Debug, PartialEq, Eq)]
enum IgnoreStatus {
    /// A `.gitignore`/exclude pattern matches the path — git already hides it.
    Ignored,
    /// Tracked by git (committed or staged). Excluding it is a no-op — git
    /// ignores exclude rules for tracked files — and a tracked daft.yml is a
    /// team baseline, not a visitor file. Never offered.
    Tracked,
    /// Untracked and not ignored — a visitor file git can currently see. This
    /// is the only status that triggers the exclude offer.
    Visible,
    /// git could not answer (not a repo, git missing or errored). Skip silently.
    Unknown,
}

/// Classify how git sees `relpath` (relative to `worktree_root`).
///
/// Probes through `git_command_at` — which strips inherited `GIT_*` so `-C` is
/// authoritative — with both pipes nulled, mirroring the conservative pattern in
/// `file::merge::is_target_untracked`:
/// 1. `rev-parse --is-inside-work-tree` — not in a repo → `Unknown`.
/// 2. `check-ignore -q` — exit 0 → `Ignored`; 1 → not ignored (continue);
///    anything else (128, …) → `Unknown`.
/// 3. `ls-files --error-unmatch` — `check-ignore` reports exit 1 for BOTH an
///    untracked-visible file AND a tracked one (tracked files are never
///    "ignored"), so disambiguate: tracked → `Tracked`, otherwise `Visible`.
///    (`daft install` always writes a fresh, untracked daft.yml, so `Tracked`
///    is unreachable from the command today — this keeps the helper correct if
///    it is ever reused where the file could already be tracked.)
fn git_ignore_status(worktree_root: &Path, relpath: &str) -> IgnoreStatus {
    let inside = git_command_at(worktree_root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !matches!(inside, Ok(s) if s.success()) {
        return IgnoreStatus::Unknown;
    }

    let checked = git_command_at(worktree_root)
        .args(["check-ignore", "-q", "--", relpath])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match checked.as_ref().map(|s| s.code()) {
        Ok(Some(0)) => return IgnoreStatus::Ignored,
        Ok(Some(1)) => {} // not ignored — fall through to the tracked probe
        _ => return IgnoreStatus::Unknown, // 128 / other / error — can't tell
    }

    let tracked = git_command_at(worktree_root)
        .args(["ls-files", "--error-unmatch", "--", relpath])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match tracked {
        Ok(s) if s.success() => IgnoreStatus::Tracked,
        Ok(_) => IgnoreStatus::Visible,
        Err(_) => IgnoreStatus::Unknown,
    }
}

/// Resolve the repository's local exclude file (`.git/info/exclude`) for the
/// repo containing `worktree_root`. `git rev-parse --git-path` resolves the
/// gitlink indirection of linked worktrees to the shared common dir. The
/// returned path can be relative to the `-C` dir, so join it onto
/// `worktree_root` (mirrors `resolve_common_dir_cli` in remove_repo.rs).
fn git_exclude_path(worktree_root: &Path) -> Result<PathBuf> {
    let out = git_command_at(worktree_root)
        .args(["rev-parse", "--git-path", "info/exclude"])
        .stderr(Stdio::null())
        .output()
        .context("Failed to run `git rev-parse --git-path info/exclude`")?;
    if !out.status.success() {
        anyhow::bail!("{} is not inside a git repository", worktree_root.display());
    }
    let raw = String::from_utf8(out.stdout)
        .context("git rev-parse output is not UTF-8")?
        .trim()
        .to_string();
    let p = PathBuf::from(&raw);
    Ok(if p.is_absolute() {
        p
    } else {
        worktree_root.join(p)
    })
}

/// Append `pattern` to the repo's `.git/info/exclude`, idempotently.
///
/// Returns the exclude file path that was written (for messaging). If the
/// pattern is already present on its own line the file is left untouched.
fn add_to_git_exclude(worktree_root: &Path, pattern: &str) -> Result<PathBuf> {
    let exclude_path = git_exclude_path(worktree_root)?;

    let existing = fs::read_to_string(&exclude_path).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == pattern) {
        return Ok(exclude_path);
    }

    if let Some(parent) = exclude_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(pattern);
    content.push('\n');
    fs::write(&exclude_path, &content)
        .with_context(|| format!("Failed to write {}", exclude_path.display()))?;
    Ok(exclude_path)
}

/// After install, offer to keep daft.yml private when git would otherwise see
/// it. No-op unless daft.yml is *visible* to git (inside a work tree, untracked,
/// and not already ignored) — so it never fires for a committed team baseline
/// or a config already in `.gitignore`/`info/exclude`.
///
/// - `--git-exclude`: add the exclude entry without prompting.
/// - `--quiet`: do nothing — no prompt, no mutation (there is no consent to infer).
/// - `interactive == true`: prompt (default No).
/// - `interactive == false`: print a copy-pasteable hint, change nothing.
///
/// `interactive` is decided by the caller (`run_with_output`) — typically
/// `stdin().is_terminal() && DAFT_TESTING unset`. Passing it in keeps this
/// function deterministic and unit-testable: a test that owns a TTY must never
/// be able to reach `dialoguer::Confirm` and block.
fn maybe_offer_git_exclude(
    worktree_root: &Path,
    output: &mut dyn Output,
    opts: &InstallOptions,
    interactive: bool,
) -> Result<()> {
    if git_ignore_status(worktree_root, "daft.yml") != IgnoreStatus::Visible {
        return Ok(());
    }

    if opts.git_exclude {
        let path = add_to_git_exclude(worktree_root, EXCLUDE_PATTERN)?;
        output.success(&format!(
            "Added {EXCLUDE_PATTERN} to {} — daft.yml stays private to this clone.",
            path.display()
        ));
        return Ok(());
    }

    if output.is_quiet() {
        return Ok(());
    }

    if !interactive {
        output.info(
            "daft.yml is visible to git. To keep it private to this clone (never committed), run:",
        );
        output.info("  echo '/daft.yml' >> \"$(git rev-parse --git-path info/exclude)\"");
        return Ok(());
    }

    let confirmed = dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt(
            "Keep daft.yml private to this clone? \
             (adds /daft.yml to .git/info/exclude — never committed)",
        )
        .default(false)
        .interact()
        .context("Failed to read confirmation")?;
    if confirmed {
        let path = add_to_git_exclude(worktree_root, EXCLUDE_PATTERN)?;
        output.success(&format!(
            "Added {EXCLUDE_PATTERN} to {} — daft.yml stays private to this clone.",
            path.display()
        ));
    } else {
        output.info(
            "Left daft.yml visible to git. Commit it for a team baseline, or exclude it later.",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::TestOutput;
    use tempfile::tempdir;

    #[test]
    fn test_install_creates_starter_file() {
        let dir = tempdir().unwrap();
        let mut output = TestOutput::new();
        install_starter(dir.path(), &mut output).unwrap();
        assert!(dir.path().join("daft.yml").is_file());
    }

    #[test]
    fn test_install_refuses_if_already_exists() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("daft.yml"), "hooks: {}").unwrap();
        let mut output = TestOutput::new();
        let result = install_starter(dir.path(), &mut output);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    /// Create an isolated, non-bare git repo in `dir` (never this project's
    /// repo — CLAUDE.md Critical Rule #2). Output is captured, not leaked.
    fn init_repo(dir: &Path) {
        let out = git_command_at(dir)
            .args(["init", "-q"])
            .output()
            .expect("git init");
        assert!(out.status.success(), "git init failed in {}", dir.display());
    }

    #[test]
    fn test_git_ignore_status_visible_then_ignored() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("daft.yml"), STARTER_TEMPLATE).unwrap();

        // Freshly written, untracked, no exclude rule → git can see it.
        assert_eq!(
            git_ignore_status(dir.path(), "daft.yml"),
            IgnoreStatus::Visible
        );

        // After excluding it, git hides it.
        add_to_git_exclude(dir.path(), EXCLUDE_PATTERN).unwrap();
        assert_eq!(
            git_ignore_status(dir.path(), "daft.yml"),
            IgnoreStatus::Ignored
        );
    }

    #[test]
    fn test_git_ignore_status_tracked_is_not_visible() {
        // `git check-ignore` returns exit 1 for a tracked file just as it does
        // for an untracked-visible one. A tracked daft.yml must classify as
        // Tracked (never Visible), so the exclude offer is suppressed —
        // excluding a tracked file would be a silent no-op.
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("daft.yml"), STARTER_TEMPLATE).unwrap();
        let add = git_command_at(dir.path())
            .args(["add", "daft.yml"])
            .output()
            .expect("git add");
        assert!(add.status.success());

        assert_eq!(
            git_ignore_status(dir.path(), "daft.yml"),
            IgnoreStatus::Tracked
        );

        // Even with the flag, a tracked file is left untouched.
        let mut output = TestOutput::new();
        maybe_offer_git_exclude(
            dir.path(),
            &mut output,
            &InstallOptions { git_exclude: true },
            false,
        )
        .unwrap();
        assert!(output.successes().is_empty());
        assert_eq!(
            git_ignore_status(dir.path(), "daft.yml"),
            IgnoreStatus::Tracked,
            "a tracked daft.yml must not be excluded"
        );
    }

    #[test]
    fn test_git_ignore_status_unknown_outside_repo() {
        let dir = tempdir().unwrap();
        // No `git init` — plain filesystem dir.
        assert_eq!(
            git_ignore_status(dir.path(), "daft.yml"),
            IgnoreStatus::Unknown
        );
    }

    #[test]
    fn test_add_to_git_exclude_is_idempotent() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());

        let path = add_to_git_exclude(dir.path(), EXCLUDE_PATTERN).unwrap();
        add_to_git_exclude(dir.path(), EXCLUDE_PATTERN).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let occurrences = content
            .lines()
            .filter(|l| l.trim() == EXCLUDE_PATTERN)
            .count();
        assert_eq!(occurrences, 1, "exclude entry must not be duplicated");
    }

    #[test]
    fn test_maybe_offer_git_exclude_flag_adds_entry() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("daft.yml"), STARTER_TEMPLATE).unwrap();

        let mut output = TestOutput::new();
        maybe_offer_git_exclude(
            dir.path(),
            &mut output,
            &InstallOptions { git_exclude: true },
            false,
        )
        .unwrap();

        assert_eq!(
            git_ignore_status(dir.path(), "daft.yml"),
            IgnoreStatus::Ignored
        );
        assert!(output.has_success("private to this clone"));
    }

    #[test]
    fn test_maybe_offer_git_exclude_quiet_is_noop() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("daft.yml"), STARTER_TEMPLATE).unwrap();

        let mut output = TestOutput::quiet();
        maybe_offer_git_exclude(
            dir.path(),
            &mut output,
            &InstallOptions { git_exclude: false },
            false,
        )
        .unwrap();

        // Quiet implies no consent to infer: nothing is excluded.
        assert_eq!(
            git_ignore_status(dir.path(), "daft.yml"),
            IgnoreStatus::Visible
        );
    }

    #[test]
    fn test_maybe_offer_git_exclude_noninteractive_hints_without_mutating() {
        // interactive=false forces the hint branch deterministically — no
        // prompt, no mutation, and crucially no dependence on whether the test
        // process owns a TTY (a real TTY would block dialoguer::Confirm, which
        // is exactly why interactivity is injected rather than read in here).
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("daft.yml"), STARTER_TEMPLATE).unwrap();

        let mut output = TestOutput::new();
        maybe_offer_git_exclude(
            dir.path(),
            &mut output,
            &InstallOptions { git_exclude: false },
            false,
        )
        .unwrap();

        assert_eq!(
            git_ignore_status(dir.path(), "daft.yml"),
            IgnoreStatus::Visible,
            "non-interactive run must not mutate info/exclude"
        );
        assert!(
            output.has_info("info/exclude"),
            "non-interactive run should print a copy-pasteable hint"
        );
    }

    #[test]
    fn test_maybe_offer_git_exclude_skips_when_not_in_repo() {
        let dir = tempdir().unwrap();
        // No git init.
        fs::write(dir.path().join("daft.yml"), STARTER_TEMPLATE).unwrap();

        let mut output = TestOutput::new();
        // git_exclude:true would normally add — but outside a repo it's Unknown,
        // so the whole step is a silent no-op (no error, no hint, no mutation).
        maybe_offer_git_exclude(
            dir.path(),
            &mut output,
            &InstallOptions { git_exclude: true },
            false,
        )
        .unwrap();

        assert!(output.successes().is_empty());
        assert!(output.infos().is_empty());
    }
}
