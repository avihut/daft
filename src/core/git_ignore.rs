//! How git currently sees a path inside a work tree.
//!
//! Two independent features need the same question answered, from opposite
//! directions:
//!
//! - `daft install` asks *"can git see this file?"* — a `daft.yml` that is
//!   untracked and unignored is a visitor config git would happily commit, so
//!   install offers to exclude it.
//! - `copy:` (#387) asks *"is git already managing this?"* — a declared build
//!   cache must be gitignored, because copying tracked content would duplicate
//!   the working tree git is already responsible for.
//!
//! Both answers come from the same two probes ([`git_ignore_status`]), plus a
//! recursive one that only the second needs ([`has_tracked_under`]).
//!
//! **These are the per-path reference implementations.** `daft install` calls
//! them directly — it classifies exactly one file. `copy:` does not: a glob can
//! expand to thousands of paths on the critical path of every worktree
//! creation, so `core::copy_paths` inlines the same two git probes **batched**,
//! and its perf test cross-checks the batched form against these. Keep the two
//! in step: this module is where the semantics are written down and explained,
//! and a change here is a change there.
//!
//! Every probe runs through [`crate::utils::git_command_at`] — which strips
//! inherited `GIT_*` so `-C` is authoritative even under a git hook — with both
//! pipes nulled, so a `fatal: not a git repository` on stderr can never leak
//! into a live rail region.

use std::path::Path;
use std::process::Stdio;

use crate::utils::git_command_at;

/// How git currently sees a path within a work tree.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum IgnoreStatus {
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
///
/// The `Tracked` arm is reachable for directories too: `check-ignore` consults
/// the index by default, so a directory holding a force-added file reports
/// *not ignored* and the `ls-files` probe then classifies it `Tracked`. That is
/// a behaviour of git's exclude machinery, not a promise of this function —
/// `copy:` states the invariant explicitly with [`has_tracked_under`].
pub fn git_ignore_status(worktree_root: &Path, relpath: &str) -> IgnoreStatus {
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

/// True when git tracks anything at or under `relpath` in `worktree_root`.
///
/// The recursive counterpart to [`git_ignore_status`]'s single-path tracked
/// probe, and the second half of `copy:`'s gitignored-only invariant: a
/// declared cache must be ignored *and* hold nothing git is managing, because
/// copying a force-added `node_modules/vendored.js` would hand the new worktree
/// a second, divergent copy of tracked content.
///
/// Today `check-ignore` alone would also catch that — it consults the index, so
/// a directory with a tracked descendant reports *not ignored*. That is an
/// implementation detail of git's exclude machinery (precisely what
/// `--no-index` turns off), not a contract, and it phrases the invariant as a
/// side effect of an unrelated flag. Asking the question directly makes the
/// rule independent of how `check-ignore` chooses to answer.
///
/// `git ls-files -- <relpath>` lists every tracked path the pathspec covers,
/// which for a directory is its whole subtree; non-empty output is the answer.
/// A git failure (not a repo, git missing) reports `false`: this probe only
/// ever *adds* a reason to refuse, and [`git_ignore_status`] has already
/// reported `Unknown` for those cases.
pub fn has_tracked_under(worktree_root: &Path, relpath: &str) -> bool {
    let out = git_command_at(worktree_root)
        .args(["ls-files", "-z", "--", relpath])
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(out) if out.status.success() => !out.stdout.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Create an isolated, non-bare git repo in `dir` (never this project's
    /// repo — CLAUDE.md Critical Rule #2). Output is captured, not leaked.
    fn init_repo(dir: &Path) {
        let out = git_command_at(dir)
            .args(["init", "-q", "-b", "main"])
            .output()
            .expect("git init");
        assert!(out.status.success(), "git init failed in {}", dir.display());
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = git_command_at(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command");
        assert!(out.status.success(), "git {args:?} failed");
    }

    #[test]
    fn classifies_ignored_visible_and_tracked() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join(".gitignore"), "/target\n").unwrap();
        fs::create_dir_all(dir.path().join("target")).unwrap();
        fs::write(dir.path().join("target/lib.rlib"), "x").unwrap();
        fs::write(dir.path().join("src.rs"), "fn main() {}").unwrap();

        assert_eq!(
            git_ignore_status(dir.path(), "target"),
            IgnoreStatus::Ignored
        );
        assert_eq!(
            git_ignore_status(dir.path(), "src.rs"),
            IgnoreStatus::Visible
        );

        git(dir.path(), &["add", "src.rs"]);
        assert_eq!(
            git_ignore_status(dir.path(), "src.rs"),
            IgnoreStatus::Tracked
        );
    }

    #[test]
    fn unknown_outside_a_repository() {
        let dir = tempdir().unwrap();
        assert_eq!(
            git_ignore_status(dir.path(), "target"),
            IgnoreStatus::Unknown
        );
    }

    #[test]
    fn tracked_probe_sees_force_added_files_under_an_ignored_directory() {
        // `copy: [node_modules]` must refuse the moment git manages anything
        // inside the cache, however deep. The probe answers that directly
        // instead of inferring it from how `check-ignore` treats the index.
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join(".gitignore"), "/node_modules\n").unwrap();
        fs::create_dir_all(dir.path().join("node_modules/pkg/deep")).unwrap();
        fs::write(dir.path().join("node_modules/pkg/deep/index.js"), "//").unwrap();

        assert_eq!(
            git_ignore_status(dir.path(), "node_modules"),
            IgnoreStatus::Ignored
        );
        assert!(
            !has_tracked_under(dir.path(), "node_modules"),
            "an ordinary ignored cache has nothing tracked under it"
        );

        git(dir.path(), &["add", "-f", "node_modules/pkg/deep/index.js"]);

        assert!(
            has_tracked_under(dir.path(), "node_modules"),
            "the recursive probe catches a force-added file at any depth"
        );
        // Today git's own exclude machinery agrees, because check-ignore
        // consults the index — but only as a side effect of a flag (--no-index)
        // that has nothing to do with this invariant, which is why the probe
        // above states it independently.
        assert_eq!(
            git_ignore_status(dir.path(), "node_modules"),
            IgnoreStatus::Tracked
        );
    }

    #[test]
    fn tracked_probe_covers_a_plain_tracked_file() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("README.md"), "# hi").unwrap();
        assert!(!has_tracked_under(dir.path(), "README.md"));
        git(dir.path(), &["add", "README.md"]);
        assert!(has_tracked_under(dir.path(), "README.md"));
    }

    #[test]
    fn both_probes_refuse_a_path_that_climbs_out_of_the_work_tree() {
        // `copy:`'s containment guard states this rule itself rather than
        // resting on git — but it does so *because* git answers the way it
        // does here, and the copy engine's comment says as much. If git ever
        // started resolving `../` pathspecs into an answer, the guard would be
        // the only thing left; pin the premise so a change in either place is
        // visible.
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        fs::write(dir.path().join("outside.txt"), "not yours").unwrap();

        assert_eq!(
            git_ignore_status(&repo, "../outside.txt"),
            IgnoreStatus::Unknown,
            "a path outside the work tree is not something git consents to"
        );
        assert!(
            !has_tracked_under(&repo, "../outside.txt"),
            "a failed pathspec never invents a tracked file"
        );
    }

    #[test]
    fn tracked_probe_is_false_outside_a_repository() {
        let dir = tempdir().unwrap();
        assert!(
            !has_tracked_under(dir.path(), "target"),
            "a git failure never invents a tracked file"
        );
    }
}
