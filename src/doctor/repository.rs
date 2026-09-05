//! Repository checks for `daft doctor`.
//!
//! Verifies that the current repository is configured correctly for daft:
//! worktree layout, worktree consistency, fetch refspec, remote HEAD.

use crate::core::worktree::porcelain::{WorktreeListEntry, parse_worktree_list_porcelain};
use crate::doctor::{CheckResult, FixAction};
use crate::git::GitCommand;
use std::path::{Path, PathBuf};

/// Context for repository checks - gathered once and shared.
pub struct RepoContext {
    pub git_common_dir: PathBuf,
    pub project_root: PathBuf,
    pub current_worktree: PathBuf,
    pub is_bare: bool,
}

/// Check if the git common dir is a bare repository by reading its config.
///
/// `git rev-parse --is-bare-repository` returns false inside a worktree,
/// even when the underlying repo IS bare. Instead we check the git config
/// at the common dir level.
fn is_common_dir_bare(git_common_dir: &Path) -> bool {
    let config_path = git_common_dir.join("config");
    if let Ok(content) = std::fs::read_to_string(config_path) {
        // Look for bare = true in the [core] section
        let mut in_core = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_core = trimmed.starts_with("[core]");
            } else if in_core && let Some(value) = trimmed.strip_prefix("bare") {
                let value = value.trim().strip_prefix('=').map(|v| v.trim());
                if value == Some("true") {
                    return true;
                }
            }
        }
    }
    false
}

/// Try to build a RepoContext for the current directory.
/// Returns None if not in a git repository.
pub fn get_repo_context() -> Option<RepoContext> {
    if !crate::is_git_repository().ok()? {
        return None;
    }

    let git_common_dir = crate::get_git_common_dir().ok()?;
    let project_root = git_common_dir.parent()?.to_path_buf();
    let is_bare = is_common_dir_bare(&git_common_dir);

    // Resolve the worktree to inspect for config/hooks repo-awarely, not as the
    // raw cwd. Running `daft doctor` from a worktree subdir, or from the bare
    // container root of a contained layout, must still find the worktree where
    // daft.yml actually lives — otherwise every config/hook check (and the
    // Repository config check) goes blind and reports "no hooks configured"
    // while a tracked/visitor daft.yml sits in a sibling worktree.
    let cwd = std::env::current_dir().ok()?;
    let current_worktree = match crate::core::repo::resolve_worktree_position(&cwd) {
        crate::core::repo::WorktreePosition::InWorktree { root } => root,
        crate::core::repo::WorktreePosition::ContainerRoot { representative } => {
            representative.unwrap_or(cwd)
        }
        crate::core::repo::WorktreePosition::NotInRepo => cwd,
    };

    Some(RepoContext {
        git_common_dir,
        project_root,
        current_worktree,
        is_bare,
    })
}

/// Report the main `daft.yml`'s presence and tracking classification.
///
/// daft.yml's tracked-vs-visitor status is a repository-configuration fact, not
/// a hooks fact, so it lives here and is reported at *every* invocation point —
/// from a worktree, a worktree subdir, or the bare container root — because
/// `ctx.current_worktree` is resolved repo-awarely (see [`get_repo_context`]).
/// Always informational (`pass`), in every state including no-config, so it
/// never flips doctor's exit code on an ordinary repo.
pub fn check_daft_config(ctx: &RepoContext) -> CheckResult {
    use crate::hooks::yaml_config_loader::{ConfigStatus, classify_main_config};

    // doctor renders a passing check as "Name (message)", so the message must
    // not carry its own parentheses or it nests them. Use em-dashes instead.
    match classify_main_config(&ctx.current_worktree) {
        ConfigStatus::Tracked => CheckResult::pass("Config", "daft.yml tracked — team baseline"),
        ConfigStatus::Visitor => {
            CheckResult::pass("Config", "daft.yml visitor — private to this clone")
        }
        ConfigStatus::Missing => CheckResult::pass("Config", "no daft.yml"),
    }
}

/// Report whether this repository's filesystem can clone blocks.
///
/// `copy:` replicates declared build caches into every new worktree. On a
/// reflinking filesystem (APFS, btrfs, XFS with `reflink=1`, OpenZFS 2.2+,
/// ReFS) that replica costs almost nothing until it diverges; anywhere else it
/// is a real byte copy, which is the entire reason the `fallback` and
/// `max_size` knobs exist. Which one a repo gets is not something a user can
/// read off the mount table, and it decides whether declaring a multi-gigabyte
/// `target/` is a free win or an expensive one.
///
/// Answered by attempting a clone in the worktree, exactly as the copy stage
/// does: the filesystem's *name* is not the question — a `copy:` entry can
/// straddle mount points, and only the attempt is honest.
///
/// Always informational, like [`check_daft_config`]: a filesystem without
/// reflink support is not a misconfiguration and there is nothing to fix, so it
/// must never flip doctor's exit code.
pub fn check_reflink_support(ctx: &RepoContext) -> CheckResult {
    // doctor renders a passing check as "Name (message)", so the message must
    // not carry its own parentheses or it nests them.
    match crate::core::copy_paths::probe_reflink_support(&ctx.current_worktree) {
        Some(true) => CheckResult::pass(
            "Copy-on-write",
            "reflink supported \u{2014} copy: entries clone near-free",
        ),
        Some(false) => CheckResult::pass(
            "Copy-on-write",
            "no reflink support \u{2014} copy: entries fall back to byte copies",
        ),
        // Distinct from "unsupported": the probe never ran, so daft does not
        // know, and saying either answer would be inventing one.
        None => CheckResult::skipped("Copy-on-write", "could not probe the worktree filesystem"),
    }
}

/// Worktrees renamed aside by a removal but not yet reclaimed (#200).
///
/// A removal hands the directory to a detached reaper and returns; if that
/// reaper dies, the space stays occupied until the next removal in the repo
/// sweeps it. That self-healing is what keeps deferred deletion honest, but it
/// only fires when the user removes something again — so a repo they have
/// stopped pruning can sit on the space indefinitely, silently. This check is
/// the surface that makes it visible, and `--fix` reclaims it on demand.
///
/// Two states get reported, and they are not the same problem. A *stale* entry
/// is one whose reap should have finished long ago — the space is simply still
/// there. An *interrupted* one never had its removal committed: the tree was
/// renamed aside but git was never told, so no reaper will ever touch it and
/// git may still point at a path that no longer exists. That is worth saying at
/// any age, so it is not gated on the threshold.
pub fn check_pending_trash(ctx: &RepoContext) -> CheckResult {
    /// Below this a pending entry is almost certainly a reap still running.
    const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60 * 60);

    let pending = crate::core::worktree::trash::pending(&ctx.git_common_dir);
    let reportable = reportable_pending(&pending, STALE_AFTER);

    if reportable.is_empty() {
        return CheckResult::pass("Pending deletions", "no worktrees awaiting reclamation");
    }

    let interrupted = reportable.iter().filter(|e| e.interrupted).count();
    let labels: Vec<String> = reportable
        .iter()
        .map(|e| {
            let name = e
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| e.path.display().to_string());
            let state = if e.interrupted {
                "removal interrupted".to_string()
            } else {
                match e.age {
                    Some(age) => format!("waiting {}", humanize_age(age)),
                    None => "waiting".to_string(),
                }
            };
            format!("{name} — {state}")
        })
        .collect();

    let headline = if interrupted == reportable.len() {
        format!("{interrupted} interrupted removal(s) awaiting resolution")
    } else if interrupted > 0 {
        format!(
            "{} removed worktree(s) still occupying disk space, {interrupted} interrupted",
            reportable.len()
        )
    } else {
        format!(
            "{} removed worktree(s) still occupying disk space",
            reportable.len()
        )
    };

    let git_common_dir = ctx.git_common_dir.clone();
    let dry_labels = labels.clone();
    CheckResult::warning("Pending deletions", &headline)
        .with_details(labels)
        .with_suggestion("run with --fix to resolve them now")
        .with_fix(Box::new(move || {
            // `reclaim` takes the single-flight lock and reports what it could
            // not do, so a fix that freed nothing cannot print as fixed — and
            // it restores an interrupted removal rather than completing it,
            // since git still expects that worktree to be there.
            let git = GitCommand::new(true);
            crate::core::worktree::trash::reclaim(&git, &git_common_dir)
                .map_err(|e| format!("{e:#}"))
        }))
        .with_dry_run_fix(Box::new(move || {
            dry_labels
                .iter()
                .map(|label| FixAction {
                    description: format!("Resolve: {label}"),
                    would_succeed: true,
                    failure_reason: None,
                })
                .collect()
        }))
}

/// Pending entries worth telling the user about: anything whose removal was
/// interrupted, plus anything that has outlived several sweeps.
///
/// An entry of unknown age is treated as *not* stale: an unreadable timestamp
/// is no evidence a reap failed, and guessing would turn a healthy repo into a
/// standing warning. An interrupted entry is reported regardless — there its
/// age says nothing, because no reaper is coming for it either way.
fn reportable_pending(
    pending: &[crate::core::worktree::trash::PendingEntry],
    threshold: std::time::Duration,
) -> Vec<&crate::core::worktree::trash::PendingEntry> {
    pending
        .iter()
        .filter(|e| e.interrupted || e.age.is_some_and(|age| age > threshold))
        .collect()
}

fn humanize_age(age: std::time::Duration) -> String {
    let hours = age.as_secs() / 3600;
    if hours < 24 {
        return format!("{hours}h");
    }
    format!("{}d", hours / 24)
}

/// Check that the repository uses a daft-compatible worktree layout.
pub fn check_worktree_layout(ctx: &RepoContext) -> CheckResult {
    let git = GitCommand::new(true);

    if !ctx.is_bare {
        return CheckResult::pass("Worktree layout", "standard repository");
    }

    // Count actual worktrees (exclude the bare repo entry)
    let worktree_count = match git.worktree_list_porcelain() {
        Ok(output) => {
            let entries = parse_worktree_list_porcelain(&output);
            entries.iter().filter(|e| !e.is_bare).count()
        }
        Err(_) => 0,
    };

    CheckResult::pass(
        "Worktree layout",
        &format!("bare repo with {worktree_count} worktrees"),
    )
}

/// Check that every recorded worktree identity still matches reality.
///
/// Daft records what branch each worktree was created for, so `daft list` can
/// still name a worktree whose HEAD is detached for a reason git does not
/// record. A record that disagrees with the branch actually checked out —
/// someone checked out a different branch into the worktree, or renamed one
/// outside `daft rename` — is not wrong in a way that breaks anything today
/// (live state always wins the name), but it means the fallback would name
/// the wrong branch the moment that worktree detaches. Reconciling is a
/// one-way update: the checkout is the truth, the record follows it.
pub fn check_worktree_identity_drift(ctx: &RepoContext) -> CheckResult {
    let records = crate::core::worktree::identity_store::read_identities(&ctx.git_common_dir);
    if records.is_empty() {
        return CheckResult::pass("Worktree identities", "no records to check");
    }

    let git = GitCommand::new(true);
    let porcelain = match git.worktree_list_porcelain() {
        Ok(output) => output,
        Err(e) => {
            return CheckResult::warning(
                "Worktree identities",
                &format!("could not list worktrees: {e}"),
            );
        }
    };

    // Only attached worktrees can disagree: a detached one is exactly the
    // case the record exists to answer, not a contradiction of it.
    let drifted: Vec<(PathBuf, String, String)> = parse_worktree_list_porcelain(&porcelain)
        .into_iter()
        .filter(|e| !e.is_bare)
        .filter_map(|entry| {
            let checked_out = entry.branch.clone()?;
            let id = crate::core::worktree::identity_store::worktree_id_for(&entry.path)?;
            let recorded = records.get(&id)?.branch.clone();
            (recorded != checked_out).then_some((entry.path, checked_out, recorded))
        })
        .collect();

    if drifted.is_empty() {
        return CheckResult::pass(
            "Worktree identities",
            &format!("all {} record(s) match their checkout", records.len()),
        );
    }

    let details: Vec<String> = drifted
        .iter()
        .map(|(path, checked_out, recorded)| {
            format!(
                "{}: checked out {checked_out}, recorded {recorded}",
                path.display()
            )
        })
        .collect();
    let fix_targets = drifted.clone();
    let dry_details = details.clone();
    // Capture the checked repo's dir by move: fixes run *after* doctor has
    // restored the original cwd (and under --all-repos, from a different
    // repo entirely), so a cwd-derived lookup here would write these targets
    // into some other repo's store.
    let fix_dir = ctx.git_common_dir.clone();

    CheckResult::warning(
        "Worktree identities",
        &format!("{} worktree(s) drifted from their record", drifted.len()),
    )
    .with_details(details)
    .with_suggestion("run with --fix to update the records to match what is checked out")
    .with_fix(Box::new(move || {
        let store = crate::core::worktree::identity_store::IdentityStore::open(&fix_dir)
            .ok_or_else(|| "identity records unavailable".to_string())?;
        for (path, checked_out, _) in &fix_targets {
            store.record(path, checked_out);
        }
        Ok(())
    }))
    .with_dry_run_fix(Box::new(move || {
        dry_details
            .iter()
            .map(|detail| FixAction {
                description: String::from("Update record: ") + detail,
                would_succeed: true,
                failure_reason: None,
            })
            .collect()
    }))
}

/// Check that all worktree paths exist on disk.
pub fn check_worktree_consistency(_ctx: &RepoContext) -> CheckResult {
    let git = GitCommand::new(true);

    let porcelain = match git.worktree_list_porcelain() {
        Ok(output) => output,
        Err(e) => {
            return CheckResult::warning(
                "Worktree consistency",
                &format!("could not list worktrees: {e}"),
            );
        }
    };

    let entries = parse_worktree_list_porcelain(&porcelain);
    // Only check non-bare entries
    let worktree_entries: Vec<&WorktreeListEntry> = entries.iter().filter(|e| !e.is_bare).collect();
    let total = worktree_entries.len();

    let mut orphaned = Vec::new();
    for entry in &worktree_entries {
        if !entry.path.exists() {
            orphaned.push(entry.path.display().to_string());
        }
    }

    if orphaned.is_empty() {
        if total == 0 {
            CheckResult::pass("Worktree consistency", "all paths valid")
        } else {
            CheckResult::pass(
                "Worktree consistency",
                &format!("all {total} worktree paths valid"),
            )
        }
    } else {
        let details: Vec<String> = orphaned
            .iter()
            .map(|p| format!("Missing path: {p}"))
            .collect();
        CheckResult::warning(
            "Worktree consistency",
            &format!("{} orphaned worktree entries", orphaned.len()),
        )
        .with_suggestion("Run 'git worktree prune' to clean up orphaned entries")
        .with_fix(Box::new(fix_worktree_consistency))
        .with_dry_run_fix(Box::new(dry_run_worktree_consistency))
        .with_details(details)
    }
}

/// Fix orphaned worktree entries by running git worktree prune.
pub fn fix_worktree_consistency() -> Result<(), String> {
    let output = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .output()
        .map_err(|e| format!("Failed to run 'git worktree prune': {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("git worktree prune failed: {stderr}"))
    }
}

/// Dry-run simulation for worktree consistency fix.
pub fn dry_run_worktree_consistency() -> Vec<FixAction> {
    let git_available = which::which("git").is_ok();
    vec![FixAction {
        description: "Run git worktree prune to clean up orphaned entries".to_string(),
        would_succeed: git_available,
        failure_reason: if git_available {
            None
        } else {
            Some("git is not available".to_string())
        },
    }]
}

/// Check that the fetch refspec is configured correctly for bare repos.
pub fn check_fetch_refspec(ctx: &RepoContext) -> CheckResult {
    if !ctx.is_bare {
        return CheckResult::skipped("Fetch refspec", "not a bare repository");
    }

    let git = GitCommand::new(true);
    let expected = "+refs/heads/*:refs/remotes/origin/*";

    match git.config_get("remote.origin.fetch") {
        Ok(Some(refspec)) if refspec == expected => {
            CheckResult::pass("Fetch refspec", "correctly configured")
        }
        Ok(Some(refspec)) => {
            CheckResult::warning("Fetch refspec", &format!("unexpected value: {refspec}"))
                .with_suggestion(
                    "Run 'git config remote.origin.fetch \"+refs/heads/*:refs/remotes/origin/*\"'",
                )
                .with_fix(Box::new(fix_fetch_refspec))
                .with_dry_run_fix(Box::new(dry_run_fetch_refspec))
        }
        Ok(None) => {
            // Check if origin remote exists
            match git.remote_exists("origin") {
                Ok(true) => CheckResult::warning("Fetch refspec", "not configured")
                    .with_suggestion("Run 'git config remote.origin.fetch \"+refs/heads/*:refs/remotes/origin/*\"'")
                    .with_fix(Box::new(fix_fetch_refspec))
                    .with_dry_run_fix(Box::new(dry_run_fetch_refspec)),
                _ => CheckResult::skipped("Fetch refspec", "no origin remote"),
            }
        }
        Err(e) => CheckResult::warning("Fetch refspec", &format!("could not read config: {e}")),
    }
}

/// Fix the fetch refspec by setting the correct value.
pub fn fix_fetch_refspec() -> Result<(), String> {
    let git = GitCommand::new(true);
    git.setup_fetch_refspec("origin")
        .map_err(|e| format!("Failed to set fetch refspec: {e}"))
}

/// Dry-run simulation for fetch refspec fix.
pub fn dry_run_fetch_refspec() -> Vec<FixAction> {
    let expected = "+refs/heads/*:refs/remotes/origin/*";
    vec![FixAction {
        description: format!("Set fetch refspec to {expected}"),
        would_succeed: true,
        failure_reason: None,
    }]
}

/// Check if remote-sync settings have been explicitly configured.
///
/// Shows a one-time informational note when none of the three remote-sync
/// keys are set, so users know the defaults have changed.
pub fn check_remote_sync_config(_ctx: &RepoContext) -> CheckResult {
    use crate::commands::config::resolve::{Snapshot, resolve_all};

    // Through the resolver rather than six hand-rolled config reads. Those
    // saw only the local and global scopes, so a value set at system or
    // worktree scope — or through the environment — read as "nothing is
    // configured", and this check now points the user at a behavior row that
    // would disagree with it.
    let configured = Snapshot::capture(&GitCommand::new(true))
        .map(|snapshot| {
            let set = resolve_all(&snapshot);
            set.behavior("remote-sync")
                .is_some_and(|behavior| behavior.is_set(&set.settings))
        })
        .unwrap_or(false);

    if configured {
        CheckResult::pass("Remote sync", "Remote sync settings are configured")
    } else {
        CheckResult::warning(
            "Remote sync",
            "Remote sync defaults have changed \u{2014} daft no longer fetches, pushes, or deletes remote branches by default",
        )
        .with_suggestion(
            "Run `daft config set remote-sync on` to opt back in, or `daft config get \
             remote-sync --origin` to see what the three settings are doing.",
        )
    }
}

/// Does `refs/remotes/origin/<branch>` resolve?
///
/// `git symbolic-ref` reports a pointer without saying whether it resolves,
/// and a dangling `origin/HEAD` outlives the ref it names. Without this gate
/// the detail below would upgrade a harmless "set" into a confident lie —
/// naming a branch that is not there. Same reasoning as the gate
/// `core::remote::local_head_branch` puts on a bare `HEAD`.
///
/// It confirms the *ref*, not the branch's continued existence on the remote:
/// a remote-tracking ref survives the branch being deleted upstream, and even
/// `fetch --prune` keeps the one `origin/HEAD` points at. Detecting that kind
/// of staleness is #933's job, not this check's.
fn remote_tracking_ref_exists(dir: &Path, branch: &str) -> bool {
    crate::utils::git_command_at(dir)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/remotes/origin/{branch}"))
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// What a present `refs/remotes/origin/HEAD` actually says.
///
/// Read from the **symref**, not config. `origin/HEAD` is a ref — git stores
/// it at `refs/remotes/origin/HEAD` and writes no `remote.origin.head` key, so
/// the config read this replaced could never answer and the check always
/// rendered the bare "set" (#935).
enum RemoteHeadState {
    /// Names a branch whose remote-tracking ref resolves.
    Names(String),
    /// Names a branch whose remote-tracking ref is gone. The shape a
    /// default-branch rename leaves behind when nobody re-points the symref
    /// (#933) — and it is not inert: `local_default_branch` reads this symref
    /// ahead of the bare-HEAD rung, so registration re-derives the dead name
    /// and writes it into the catalog, every time, until this is repaired.
    Dangling(String),
    /// Present but not symbolic, so it names no branch at all.
    Opaque,
}

fn remote_head_state(git_common_dir: &Path) -> RemoteHeadState {
    match crate::core::remote::local_default_branch(git_common_dir, "origin") {
        Some(branch) if remote_tracking_ref_exists(git_common_dir, &branch) => {
            RemoteHeadState::Names(branch)
        }
        Some(branch) => RemoteHeadState::Dangling(branch),
        None => RemoteHeadState::Opaque,
    }
}

/// Check that remote HEAD (refs/remotes/origin/HEAD) is set.
pub fn check_remote_head(ctx: &RepoContext) -> CheckResult {
    if !ctx.is_bare {
        return CheckResult::skipped("Remote HEAD", "not a bare repository");
    }

    let git = GitCommand::new(true);

    // Check if origin remote exists first
    match git.remote_exists("origin") {
        Ok(false) => return CheckResult::skipped("Remote HEAD", "no origin remote"),
        Err(_) => return CheckResult::skipped("Remote HEAD", "could not check remotes"),
        Ok(true) => {}
    }

    match git.show_ref_exists("refs/remotes/origin/HEAD") {
        Ok(true) => match remote_head_state(&ctx.git_common_dir) {
            RemoteHeadState::Names(branch) => {
                CheckResult::pass("Remote HEAD", &format!("set to origin/{branch}"))
            }
            RemoteHeadState::Dangling(branch) => CheckResult::warning(
                "Remote HEAD",
                &format!("points at origin/{branch}, which no longer exists"),
            )
            .with_suggestion("Run 'git remote set-head origin --auto'")
            .with_fix(Box::new(fix_remote_head))
            .with_dry_run_fix(Box::new(dry_run_remote_head)),
            RemoteHeadState::Opaque => CheckResult::pass("Remote HEAD", "set"),
        },
        Ok(false) => CheckResult::warning("Remote HEAD", "not set")
            .with_suggestion("Run 'git remote set-head origin --auto'")
            .with_fix(Box::new(fix_remote_head))
            .with_dry_run_fix(Box::new(dry_run_remote_head)),
        Err(e) => CheckResult::warning("Remote HEAD", &format!("could not check: {e}")),
    }
}

/// Fix remote HEAD by running git remote set-head origin --auto.
pub fn fix_remote_head() -> Result<(), String> {
    let git = GitCommand::new(true);
    git.remote_set_head_auto("origin")
        .map_err(|e| format!("Failed to set remote HEAD: {e}"))
}

/// Dry-run simulation for remote HEAD fix.
pub fn dry_run_remote_head() -> Vec<FixAction> {
    vec![FixAction {
        description: "Run git remote set-head origin --auto".to_string(),
        would_succeed: true,
        failure_reason: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::trash::PendingEntry;
    use crate::doctor::CheckStatus;
    use std::time::Duration;

    fn entry(name: &str, age: Option<Duration>) -> PendingEntry {
        PendingEntry {
            path: std::path::PathBuf::from(name),
            age,
            interrupted: false,
        }
    }

    /// A reap in flight is the ordinary case; reporting it would make every
    /// removal look like a problem for as long as the delete takes.
    #[test]
    fn a_young_pending_entry_is_not_reported() {
        let pending = vec![entry("just-removed", Some(Duration::from_secs(5)))];
        assert!(reportable_pending(&pending, Duration::from_secs(3600)).is_empty());
    }

    /// An entry outliving the threshold means sweeps are not reaching it.
    #[test]
    fn an_old_pending_entry_is_reported() {
        let pending = vec![
            entry("fresh", Some(Duration::from_secs(5))),
            entry("stuck", Some(Duration::from_secs(90_000))),
        ];
        let stale = reportable_pending(&pending, Duration::from_secs(3600));
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].path, std::path::PathBuf::from("stuck"));
    }

    /// An unreadable timestamp is not evidence of failure — treating it as
    /// stale would leave a healthy repo permanently warning.
    #[test]
    fn a_pending_entry_of_unknown_age_is_not_reported() {
        let pending = vec![entry("unknown", None)];
        assert!(reportable_pending(&pending, Duration::from_secs(3600)).is_empty());
    }

    /// An interrupted removal is never healthy, however young: no reaper will
    /// ever take it, so waiting for a threshold only delays the report.
    #[test]
    fn an_interrupted_removal_is_reported_at_any_age() {
        let pending = vec![PendingEntry {
            path: std::path::PathBuf::from("cut-short"),
            age: Some(Duration::from_secs(2)),
            interrupted: true,
        }];
        let reportable = reportable_pending(&pending, Duration::from_secs(3600));
        assert_eq!(reportable.len(), 1);
    }

    #[test]
    fn age_reads_in_hours_then_days() {
        assert_eq!(humanize_age(Duration::from_secs(3600)), "1h");
        assert_eq!(humanize_age(Duration::from_secs(3600 * 25)), "1d");
    }

    #[test]
    fn test_is_common_dir_bare_true() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        std::fs::write(
            &config,
            "[core]\n\trepositoryformatversion = 0\n\tbare = true\n",
        )
        .unwrap();
        assert!(is_common_dir_bare(temp.path()));
    }

    #[test]
    fn test_is_common_dir_bare_false() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        std::fs::write(
            &config,
            "[core]\n\trepositoryformatversion = 0\n\tbare = false\n",
        )
        .unwrap();
        assert!(!is_common_dir_bare(temp.path()));
    }

    #[test]
    fn test_is_common_dir_bare_no_config() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!is_common_dir_bare(temp.path()));
    }

    /// The deferred --fix closure must write into the repo the check was
    /// built from, not whatever the process cwd names when apply_fixes later
    /// runs: doctor restores the original cwd between collect and fix, so
    /// under --all-repos a cwd-derived store lookup wrote repo B's drift
    /// targets into repo A's store — same-named ids (`master`) even
    /// overwrite A's real records via upsert — while B's drift survived
    /// behind a green "done".
    #[test]
    #[serial_test::serial]
    fn identity_fix_writes_into_the_checked_repo_not_the_cwd() {
        let _state = crate::store::paths::IsolatedStateDir::new();
        let orig_cwd = std::env::current_dir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "--allow-empty", "-q", "-m", "init"]);
        git(&repo, &["branch", "feat-x"]);
        let wt = tmp.path().join("wt");
        git(
            &repo,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "feat-x"],
        );
        let common = repo.join(".git");

        // A record disagreeing with the checkout: drift.
        let store = crate::core::worktree::identity_store::IdentityStore::open(&common).unwrap();
        store.record(&wt, "feat-recorded");

        // The check runs with cwd inside the repo, as doctor's collect pass
        // arranges…
        std::env::set_current_dir(&repo).unwrap();
        let ctx = RepoContext {
            git_common_dir: common.clone(),
            project_root: repo.clone(),
            current_worktree: repo.clone(),
            is_bare: false,
        };
        let result = check_worktree_identity_drift(&ctx);
        std::env::set_current_dir(&orig_cwd).unwrap();
        assert_eq!(result.status, CheckStatus::Warning);
        let fix = result.fix.expect("drift offers a fix");

        // …but the fix runs after doctor restored the original cwd.
        fix().expect("fix succeeds away from the repo's cwd");

        let records = crate::core::worktree::identity_store::read_identities(&common);
        let id = crate::core::worktree::identity_store::worktree_id_for(&wt).unwrap();
        assert_eq!(
            records[&id].branch, "feat-x",
            "the record is reconciled to the checkout, in the checked repo's store"
        );
    }

    #[test]
    fn test_dry_run_worktree_consistency() {
        let actions = dry_run_worktree_consistency();
        assert_eq!(actions.len(), 1);
        assert!(actions[0].description.contains("git worktree prune"));
        // In test env, git should be available
        assert!(actions[0].would_succeed);
    }

    #[test]
    fn test_dry_run_fetch_refspec() {
        let actions = dry_run_fetch_refspec();
        assert_eq!(actions.len(), 1);
        assert!(actions[0].description.contains("fetch refspec"));
    }

    #[test]
    fn test_dry_run_remote_head() {
        let actions = dry_run_remote_head();
        assert_eq!(actions.len(), 1);
        assert!(actions[0].description.contains("git remote set-head"));
    }

    // ── check_daft_config (tracked / visitor / missing) ──────────────────────

    fn git(dir: &Path, args: &[&str]) {
        let out = crate::utils::git_command_at(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command");
        assert!(out.status.success(), "git {args:?} failed");
    }

    fn ctx_for(worktree: &Path) -> RepoContext {
        RepoContext {
            git_common_dir: worktree.join(".git"),
            project_root: worktree.to_path_buf(),
            current_worktree: worktree.to_path_buf(),
            is_bare: false,
        }
    }

    #[test]
    fn test_check_daft_config_missing() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        let result = check_daft_config(&ctx_for(dir.path()));
        assert_eq!(result.status, CheckStatus::Pass);
        assert_eq!(result.message, "no daft.yml");
    }

    #[test]
    fn test_check_daft_config_visitor() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("daft.yml"), "hooks: {}").unwrap();
        let result = check_daft_config(&ctx_for(dir.path()));
        assert_eq!(result.status, CheckStatus::Pass);
        assert!(
            result.message.contains("visitor"),
            "got: {}",
            result.message
        );
    }

    // ── check_reflink_support ───────────────────────────────────────────

    #[test]
    fn reflink_check_is_informational_either_way_and_leaves_nothing_behind() {
        // Both answers are Pass: a filesystem that cannot reflink is a fact
        // about the machine, not a problem with the repo, and a permanent
        // yellow row on every ext4 install would be noise. The two answers do
        // have to differ — a row that said the same thing regardless would be
        // reporting nothing.
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);

        let result = check_reflink_support(&ctx_for(dir.path()));
        assert_eq!(result.status, CheckStatus::Pass, "{}", result.message);
        assert!(
            result.message.contains("reflink"),
            "got: {}",
            result.message
        );
        assert!(
            !result.message.contains('('),
            "doctor renders a pass as \"Name (message)\": {}",
            result.message
        );

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains("reflink-probe"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the probe must not litter the worktree: {leftovers:?}"
        );
    }

    #[test]
    fn reflink_check_skips_when_it_cannot_probe() {
        // "Could not ask" is a third answer, not a quiet vote for either of
        // the other two.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-worktree");
        assert_eq!(
            check_reflink_support(&ctx_for(&missing)).status,
            CheckStatus::Skipped
        );
    }

    #[test]
    fn test_check_daft_config_tracked() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("daft.yml"), "hooks: {}").unwrap();
        git(dir.path(), &["add", "daft.yml"]);
        git(dir.path(), &["commit", "-q", "-m", "add"]);
        let result = check_daft_config(&ctx_for(dir.path()));
        assert_eq!(result.status, CheckStatus::Pass);
        assert!(
            result.message.contains("tracked"),
            "got: {}",
            result.message
        );
    }

    /// Run git in an isolated temp repo with a fixed identity — never global
    /// config (CLAUDE.md Rule #1), never this project's repo (Rule #2).
    fn git_at(dir: &Path, args: &[&str]) {
        let out = crate::utils::git_command_at(dir)
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

    /// `git_at`, but hands back trimmed stdout.
    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = crate::utils::git_command_at(dir)
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
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo whose `origin/HEAD` names a branch that is really there.
    fn repo_with_remote_branch(dir: &Path, branch: &str) -> String {
        git_at(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("f"), "x").expect("write fixture file");
        git_at(dir, &["add", "f"]);
        git_at(dir, &["commit", "-q", "-m", "init"]);
        let sha = git_out(dir, &["rev-parse", "HEAD"]);
        git_at(
            dir,
            &["update-ref", &format!("refs/remotes/origin/{branch}"), &sha],
        );
        sha
    }

    /// #935: the detail came from `config_get("remote.origin.head")`, a key
    /// git never writes, so a repo with `origin/HEAD` properly set still
    /// rendered the bare "set". Reading the symref is what makes the detailed
    /// arm reachable at all.
    #[test]
    fn remote_head_state_names_the_branch_the_symref_points_at() {
        let dir = tempfile::tempdir().unwrap();
        repo_with_remote_branch(dir.path(), "develop");
        git_at(
            dir.path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/develop",
            ],
        );
        assert!(matches!(
            remote_head_state(dir.path()),
            RemoteHeadState::Names(b) if b == "develop"
        ));
    }

    /// #933: a symref pointing at a ref that is gone — what a default-branch
    /// rename leaves behind. `show_ref_exists` still says the symref is there
    /// (gix resolves the pointer, not the target), so the check reaches this
    /// and must not report a branch that does not exist as a green pass.
    #[test]
    fn remote_head_state_reports_a_dangling_symref() {
        let dir = tempfile::tempdir().unwrap();
        git_at(dir.path(), &["init", "-q", "--bare", "-b", "main"]);
        git_at(
            dir.path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/develop",
            ],
        );
        assert!(matches!(
            remote_head_state(dir.path()),
            RemoteHeadState::Dangling(b) if b == "develop"
        ));
    }

    /// `origin/HEAD` written as a plain sha rather than a symref — `git
    /// symbolic-ref` exits non-zero there, so it names no branch and the check
    /// keeps the bare "set" rather than claiming one.
    #[test]
    fn remote_head_state_is_opaque_when_the_ref_is_not_symbolic() {
        let dir = tempfile::tempdir().unwrap();
        let sha = repo_with_remote_branch(dir.path(), "develop");
        git_at(
            dir.path(),
            &["update-ref", "--no-deref", "refs/remotes/origin/HEAD", &sha],
        );
        assert!(matches!(
            remote_head_state(dir.path()),
            RemoteHeadState::Opaque
        ));
    }
}
