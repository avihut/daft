//! Operation-scoped changed-file list resolution.
//!
//! A [`ChangedFilesProvider`] produces the list of files the current
//! operation changed. It is constructed cheaply per hook fire and resolves
//! lazily at most once, shared by every consumer: job `glob:`/`exclude:`
//! filters, `changed:` skip/only rules, and `{changed_files}` command
//! injection. Hook types with no notion of a changed set (the lifecycle
//! hooks) simply have no provider — a job that declares file-awareness there
//! must bring its own `files:` command or fail loudly at spec-build time.

use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::hooks::HookType;
use crate::hooks::environment::HookContext;

/// Lazily-resolved changed-file list for one hook fire.
#[derive(Debug)]
pub struct ChangedFilesProvider {
    source: Source,
    cache: OnceLock<Result<Vec<String>, String>>,
}

/// Where the file list comes from.
#[derive(Debug, Clone)]
enum Source {
    /// Files the merge sources changed relative to the target: the three-dot
    /// range `target...source` (merge-base to source tip) per source,
    /// unioned and sorted. This is the merge-gate question — "what does this
    /// track change?" — not the two-dot tree diff.
    MergeRange {
        worktree: PathBuf,
        target: String,
        source_shas: Vec<String>,
    },
    /// What is staged for the commit being made: `git diff --cached`
    /// restricted to added/copied/modified/renamed paths, so a job never
    /// receives a path it cannot open.
    Staged {
        worktree: PathBuf,
        /// The index git pointed the hook at. Load-bearing — see
        /// [`staged_files`].
        index_file: Option<PathBuf>,
    },
    /// What a push would send: the diff between each ref's remote tip and its
    /// local tip, unioned across refs.
    PushRange {
        worktree: PathBuf,
        remote: String,
        /// `(remote_oid, local_oid)` per non-delete ref.
        ranges: Vec<(String, String)>,
    },
    /// Every tracked file. The escape hatch for a stage with no natural
    /// change set (`post-checkout` after a branch switch, say) where the job
    /// wants to look at the whole tree.
    AllTracked { worktree: PathBuf },
    /// A hook-level `files:` command — one list shared by every job in the
    /// hook, resolved once.
    Command { command: String, dir: PathBuf },
    /// A pre-resolved list. Test seam, and the natural variant for future
    /// providers that already hold their list in memory.
    Fixed(Vec<String>),
}

impl ChangedFilesProvider {
    /// Build the provider appropriate for this hook fire, or `None` when the
    /// hook type has no operation-scoped changed set.
    ///
    /// Merge hooks derive the range from the `DAFT_MERGE_*` context:
    /// endpoints are the target (the pinned `DAFT_MERGE_TARGET_SHA` when
    /// present — exact even after the ref moves — else
    /// `DAFT_MERGE_TARGET_BRANCH`) and each entry of
    /// `DAFT_MERGE_SOURCE_SHAS` (newline-joined). The diff runs in
    /// `worktree`, whose repository shares refs and objects with every
    /// sibling worktree, so branch names and SHAs resolve regardless of
    /// which worktree the hook happens to run in.
    pub fn for_hook(ctx: &HookContext, worktree: &Path) -> Option<Self> {
        if !matches!(ctx.hook_type, HookType::PreMerge | HookType::PostMerge) {
            return None;
        }
        let non_empty = |key: &str| {
            ctx.extra_env
                .get(key)
                .map(String::as_str)
                .filter(|v| !v.is_empty())
        };
        let target = non_empty("DAFT_MERGE_TARGET_SHA")
            .or_else(|| non_empty("DAFT_MERGE_TARGET_BRANCH"))?
            .to_string();
        let source_shas: Vec<String> = non_empty("DAFT_MERGE_SOURCE_SHAS")?
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        if source_shas.is_empty() {
            return None;
        }
        Some(Self {
            source: Source::MergeRange {
                worktree: worktree.to_path_buf(),
                target,
                source_shas,
            },
            cache: OnceLock::new(),
        })
    }

    /// A provider over an already-known file list.
    pub fn preresolved(files: Vec<String>) -> Self {
        Self {
            source: Source::Fixed(files),
            cache: OnceLock::new(),
        }
    }

    /// Files staged for the commit in progress.
    pub fn staged(worktree: &Path, index_file: Option<PathBuf>) -> Self {
        Self {
            source: Source::Staged {
                worktree: worktree.to_path_buf(),
                index_file,
            },
            cache: OnceLock::new(),
        }
    }

    /// Files a push would send, from the refs git named on the hook's stdin.
    ///
    /// Deletes carry the zero OID on the local side and contribute nothing —
    /// there is no content to inspect. A ref new to the remote also carries
    /// the zero OID, on the *remote* side, and is handled in
    /// [`diff_push_range`].
    ///
    /// Returns `None` when no ref carries content (a delete-only push). That
    /// is deliberately distinct from a provider resolving to an empty list:
    /// "there is no such source here" makes a file-aware job skip with a
    /// reason, where an empty list would read as "the diff came back empty"
    /// and be indistinguishable from a broken probe.
    pub fn push_range(
        worktree: &Path,
        remote: &str,
        refs: &[crate::core::worktree::ports::PushRef],
    ) -> Option<Self> {
        let ranges: Vec<(String, String)> = refs
            .iter()
            .filter(|r| !is_zero_oid(&r.local_oid))
            .map(|r| (r.remote_oid.clone(), r.local_oid.clone()))
            .collect();
        if ranges.is_empty() {
            return None;
        }
        Some(Self {
            source: Source::PushRange {
                worktree: worktree.to_path_buf(),
                remote: remote.to_string(),
                ranges,
            },
            cache: OnceLock::new(),
        })
    }

    /// Every tracked file in the worktree.
    pub fn all_tracked(worktree: &Path) -> Self {
        Self {
            source: Source::AllTracked {
                worktree: worktree.to_path_buf(),
            },
            cache: OnceLock::new(),
        }
    }

    /// A hook-level `files:` command, resolved once and shared by the hook's
    /// jobs.
    pub fn from_command(command: &str, dir: &Path) -> Self {
        Self {
            source: Source::Command {
                command: command.to_string(),
                dir: dir.to_path_buf(),
            },
            cache: OnceLock::new(),
        }
    }

    /// The changed-file list, resolved on first call and cached (including a
    /// failed resolution — a broken diff fails every consumer identically
    /// rather than retrying per job).
    pub fn files(&self) -> Result<&[String]> {
        let cached = self
            .cache
            .get_or_init(|| resolve(&self.source).map_err(|e| format!("{e:#}")));
        match cached {
            Ok(files) => Ok(files.as_slice()),
            Err(e) => bail!("failed to resolve changed files: {e}"),
        }
    }
}

fn resolve(source: &Source) -> Result<Vec<String>> {
    match source {
        Source::Fixed(files) => Ok(files.clone()),
        Source::MergeRange {
            worktree,
            target,
            source_shas,
        } => {
            let mut union = BTreeSet::new();
            for sha in source_shas {
                union.extend(diff_merge_range(worktree, target, sha)?);
            }
            Ok(union.into_iter().collect())
        }
        Source::Staged {
            worktree,
            index_file,
        } => staged_files(worktree, index_file.as_deref()),
        Source::PushRange {
            worktree,
            remote,
            ranges,
        } => {
            let mut union = BTreeSet::new();
            for (remote_oid, local_oid) in ranges {
                union.extend(diff_push_range(worktree, remote, remote_oid, local_oid)?);
            }
            Ok(union.into_iter().collect())
        }
        Source::AllTracked { worktree } => all_tracked_files(worktree),
        Source::Command { command, dir } => run_files_command(command, dir),
    }
}

/// git's all-zeroes OID, meaning "this side does not exist" in a `pre-push`
/// ref line: on the local side a delete, on the remote side a new branch.
fn is_zero_oid(oid: &str) -> bool {
    !oid.is_empty() && oid.chars().all(|c| c == '0')
}

/// Files staged for the commit in progress.
///
/// `--diff-filter=ACMR` drops deletions: a `pre-commit` job is handed paths to
/// open, and a deleted path fails every linter it is passed.
///
/// **`GIT_INDEX_FILE` is load-bearing here.** During `git commit -a` (and
/// `commit <paths>`, and `--amend` with paths) git builds a *temporary* index,
/// exports `GIT_INDEX_FILE` pointing at it, and runs the hook. The real
/// `.git/index` at that moment does not contain the changes being committed.
/// [`crate::utils::git_command_at`] deliberately strips the `GIT_*` discovery
/// variables so `-C` is authoritative — which strips this one too — so the
/// dispatcher captures the value on entry and it is re-exported here. Without
/// it a `pre-commit` gate reads the wrong index and silently checks the wrong
/// set of files.
fn staged_files(worktree: &Path, index_file: Option<&Path>) -> Result<Vec<String>> {
    let mut cmd = crate::utils::git_command_at(worktree);
    cmd.args([
        "diff",
        "--cached",
        "--name-only",
        "-z",
        "--no-renames",
        "--diff-filter=ACMR",
    ]);
    if let Some(index) = index_file {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let output = cmd
        .output()
        .context("failed to run git diff --cached for staged files")?;
    if !output.status.success() {
        bail!(
            "git diff --cached failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_nul_delimited(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Files one ref of a push would carry.
///
/// A ref already on the remote diffs `<remote_oid>..<local_oid>`. A ref new to
/// the remote has no such endpoint — git supplies the zero OID — so the
/// question becomes "what does this branch add that no other remote branch
/// already has", which is `git log --name-only <local> --not --remotes=<remote>`.
/// Diffing a new branch against its whole history instead would hand a
/// `pre-push` gate every file the repository has ever contained.
fn diff_push_range(
    worktree: &Path,
    remote: &str,
    remote_oid: &str,
    local_oid: &str,
) -> Result<Vec<String>> {
    let mut cmd = crate::utils::git_command_at(worktree);
    let range;
    let not_remotes;
    let new_branch = is_zero_oid(remote_oid);
    if new_branch {
        not_remotes = format!("--remotes={remote}");
        cmd.args([
            "log",
            "--name-only",
            "--pretty=format:",
            "-z",
            "--no-renames",
            local_oid,
            "--not",
            &not_remotes,
        ]);
    } else {
        range = format!("{remote_oid}..{local_oid}");
        cmd.args(["diff", "--name-only", "-z", "--no-renames", &range]);
    }
    let output = cmd
        .output()
        .context("failed to resolve the files a push would send")?;
    if !output.status.success() {
        bail!(
            "resolving pushed files for {local_oid} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    if new_branch {
        // `git log` interleaves per-commit blocks, so entries can carry the
        // newline that separated a (here empty) commit header from its file
        // list, and the same path appears once per commit that touched it.
        let mut seen = BTreeSet::new();
        Ok(raw
            .split('\0')
            .map(|s| s.trim_matches('\n'))
            .filter(|s| !s.is_empty())
            .filter(|s| seen.insert(s.to_string()))
            .map(str::to_string)
            .collect())
    } else {
        Ok(parse_nul_delimited(&raw))
    }
}

/// Every tracked file, repository-root-relative.
fn all_tracked_files(worktree: &Path) -> Result<Vec<String>> {
    let output = crate::utils::git_command_at(worktree)
        .args(["ls-files", "-z", "--cached"])
        .output()
        .context("failed to run git ls-files")?;
    if !output.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_nul_delimited(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// `git diff --name-only -z --no-renames <target>...<source>` in `worktree`.
/// Paths come back repository-root-relative regardless of cwd, which is the
/// coordinate system every glob pattern uses.
///
/// `-z` is load-bearing, not a micro-optimization. Without it git applies
/// `core.quotePath` (on by default) and C-quotes any path containing a
/// non-ASCII or control byte: `src/café.rs` arrives as the literal
/// `"src/caf\303\251.rs"`, quotes included. That string matches no glob, so
/// the job filtered on `src/**` reports "no changed files match" and skips —
/// a gate silently passing green on the one change it was configured to
/// check. `-z` emits raw NUL-delimited paths, which also removes the
/// newline-in-filename ambiguity that line parsing can never resolve.
fn diff_merge_range(worktree: &Path, target: &str, source: &str) -> Result<Vec<String>> {
    let range = format!("{target}...{source}");
    let output = crate::utils::git_command_at(worktree)
        .args(["diff", "--name-only", "-z", "--no-renames", &range])
        .output()
        .with_context(|| format!("failed to run git diff for '{range}'"))?;
    if !output.status.success() {
        bail!(
            "git diff '{range}' failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_nul_delimited(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Split `git`'s `-z` output into paths. Entries are NUL-terminated, so the
/// final split yields an empty trailing element that is dropped along with
/// any other empties.
fn parse_nul_delimited(raw: &str) -> Vec<String> {
    raw.split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Run a job's `files:` command via `sh -c` in `dir`, expecting one
/// repository-root-relative path per line. A non-zero exit is an error (a
/// gate must not silently under-run on a broken provider); an empty result
/// is a valid "nothing to do" answer the caller turns into a skip.
pub fn run_files_command(command: &str, dir: &Path) -> Result<Vec<String>> {
    let output = std::process::Command::new("sh")
        .args(["-c", command])
        .current_dir(dir)
        .output()
        .with_context(|| format!("failed to run files command: {command}"))?;
    if !output.status.success() {
        bail!(
            "files command failed (exit {}): {command}{}",
            output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into()),
            match String::from_utf8_lossy(&output.stderr).trim() {
                "" => String::new(),
                err => format!("\n{err}"),
            }
        );
    }
    Ok(parse_file_lines(&String::from_utf8_lossy(&output.stdout)))
}

/// One path per line; blank lines dropped, surrounding whitespace trimmed.
fn parse_file_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn strs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn merge_ctx(hook_type: HookType, env: &[(&str, &str)]) -> HookContext {
        let extra: std::collections::BTreeMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        HookContext::new(
            hook_type,
            "merge",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/main",
            "main",
        )
        .with_extra_env(extra)
    }

    #[test]
    fn for_hook_builds_a_provider_for_merge_hooks() {
        let ctx = merge_ctx(
            HookType::PreMerge,
            &[
                ("DAFT_MERGE_TARGET_BRANCH", "main"),
                ("DAFT_MERGE_SOURCE_SHAS", "abc123\ndef456"),
            ],
        );
        let p = ChangedFilesProvider::for_hook(&ctx, Path::new("/project/main"));
        assert!(p.is_some());
    }

    #[test]
    fn for_hook_prefers_the_pinned_target_sha() {
        let ctx = merge_ctx(
            HookType::PostMerge,
            &[
                ("DAFT_MERGE_TARGET_BRANCH", "main"),
                ("DAFT_MERGE_TARGET_SHA", "feedbee"),
                ("DAFT_MERGE_SOURCE_SHAS", "abc123"),
            ],
        );
        let p = ChangedFilesProvider::for_hook(&ctx, Path::new("/x")).unwrap();
        match &p.source {
            Source::MergeRange { target, .. } => assert_eq!(target, "feedbee"),
            other => panic!("expected MergeRange, got {other:?}"),
        }
    }

    #[test]
    fn for_hook_is_none_for_lifecycle_hooks_and_missing_env() {
        // Lifecycle hook type → no provider even with merge-looking env.
        let ctx = merge_ctx(
            HookType::PostCreate,
            &[
                ("DAFT_MERGE_TARGET_BRANCH", "main"),
                ("DAFT_MERGE_SOURCE_SHAS", "abc123"),
            ],
        );
        assert!(ChangedFilesProvider::for_hook(&ctx, Path::new("/x")).is_none());

        // Merge hook type without the env (e.g. a manual re-fire outside a
        // merge) → no provider; file-aware jobs then fail loudly upstream.
        let ctx = merge_ctx(HookType::PreMerge, &[]);
        assert!(ChangedFilesProvider::for_hook(&ctx, Path::new("/x")).is_none());

        // Empty source list → no provider.
        let ctx = merge_ctx(
            HookType::PreMerge,
            &[
                ("DAFT_MERGE_TARGET_BRANCH", "main"),
                ("DAFT_MERGE_SOURCE_SHAS", ""),
            ],
        );
        assert!(ChangedFilesProvider::for_hook(&ctx, Path::new("/x")).is_none());
    }

    #[test]
    fn preresolved_returns_the_list() {
        let p = ChangedFilesProvider::preresolved(strs(&["a.rs", "b/c.md"]));
        assert_eq!(p.files().unwrap(), &strs(&["a.rs", "b/c.md"])[..]);
        // Second call hits the cache and agrees.
        assert_eq!(p.files().unwrap().len(), 2);
    }

    #[test]
    fn run_files_command_parses_lines_and_trims() {
        let dir = tempfile::tempdir().unwrap();
        let files =
            run_files_command("printf 'a.rs\\n  b/c.md  \\n\\n./d.txt\\n'", dir.path()).unwrap();
        assert_eq!(files, strs(&["a.rs", "b/c.md", "./d.txt"]));
    }

    #[test]
    fn run_files_command_empty_output_is_ok_and_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run_files_command("true", dir.path()).unwrap().is_empty());
    }

    #[test]
    fn run_files_command_failure_is_an_error_with_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let err = run_files_command("echo boom >&2; exit 3", dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("exit 3"), "{msg}");
        assert!(msg.contains("boom"), "{msg}");
    }

    /// Build an isolated scratch repo (never this project's repo) with a
    /// `main` branch and a `track` branch that adds/changes files, then
    /// check the three-dot union. Identity comes from env vars per the
    /// safe-testing rules — no global (or even local) config writes needed.
    #[test]
    fn merge_range_resolves_three_dot_union() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(root)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        git(&["init", "-q", "-b", "main"]);
        std::fs::write(root.join("base.txt"), "base").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);

        git(&["checkout", "-q", "-b", "track"]);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("docs.md"), "docs").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "track work"]);
        let track_sha = git(&["rev-parse", "HEAD"]);

        // Advance main independently — three-dot must NOT count this file
        // (it diffs from the merge-base, not the current target tip).
        git(&["checkout", "-q", "main"]);
        std::fs::write(root.join("target-only.txt"), "t").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "target moves on"]);

        let provider = ChangedFilesProvider {
            source: Source::MergeRange {
                worktree: root.to_path_buf(),
                target: "main".to_string(),
                source_shas: vec![track_sha],
            },
            cache: OnceLock::new(),
        };
        let files = provider.files().unwrap();
        assert_eq!(files, &strs(&["docs.md", "src/lib.rs"])[..]);
    }

    #[test]
    fn merge_range_bad_ref_is_a_loud_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let out = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success());

        let provider = ChangedFilesProvider {
            source: Source::MergeRange {
                worktree: root.to_path_buf(),
                target: "no-such-ref".to_string(),
                source_shas: vec!["also-missing".to_string()],
            },
            cache: OnceLock::new(),
        };
        let err = provider.files().unwrap_err();
        assert!(
            format!("{err:#}").contains("no-such-ref...also-missing"),
            "{err:#}"
        );
    }
}
