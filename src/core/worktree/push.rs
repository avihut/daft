//! Core logic for pushing worktree branches to their remotes.
//!
//! Used by `daft sync --push` to push all branches to their remote
//! tracking branches after updating/rebasing, and home of
//! [`push_with_hooks`] — the shared composition every daft push site
//! routes through to honor and report `pre-push` hooks (#599).

use crate::core::ProgressSink;
use crate::core::worktree::fetch;
use crate::core::worktree::ports::{PushRef, StageRunner};
use crate::executor::presenter::JobPresenter;
use crate::git::push_porcelain::parse_push_report;
use crate::git::{GitCommand, PushIo, PushOptions, PushStream};
use crate::utils::*;
use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// Input parameters for the push operation.
pub struct PushParams {
    /// Use --force-with-lease when pushing.
    pub force_with_lease: bool,
    /// Name of the remote (e.g. "origin").
    pub remote_name: String,
    /// Skip the repo's pre-push hook (`--no-verify` passthrough).
    pub no_verify: bool,
}

/// Result of pushing a single worktree branch.
#[derive(Debug, Default)]
pub struct WorktreePushResult {
    pub worktree_name: String,
    pub branch_name: String,
    pub success: bool,
    /// "Everything up-to-date" — nothing to push.
    pub up_to_date: bool,
    /// Branch has no remote tracking branch.
    pub no_upstream: bool,
    /// Verdict on the repo's pre-push gate for this push.
    pub hook: HookVerdict,
    pub message: String,
}

/// Aggregated result of pushing all worktrees.
pub struct PushResult {
    pub results: Vec<WorktreePushResult>,
    pub remote_name: String,
}

impl PushResult {
    pub fn pushed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.success && !r.up_to_date && !r.no_upstream)
            .count()
    }

    pub fn up_to_date_count(&self) -> usize {
        self.results.iter().filter(|r| r.up_to_date).count()
    }

    pub fn no_upstream_count(&self) -> usize {
        self.results.iter().filter(|r| r.no_upstream).count()
    }

    pub fn failed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| !r.success && !r.no_upstream)
            .count()
    }

    /// Failures that happened with a pre-push hook installed and honored.
    /// These escalate to a non-zero exit (#599): a gate saying no must not
    /// be reduced to a warning. Hook-less or `--no-verify` failures keep the
    /// legacy warn-and-continue ergonomics.
    pub fn gated_failure_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| {
                !r.success
                    && !r.no_upstream
                    && matches!(r.hook, HookVerdict::Rejected | HookVerdict::Passed)
            })
            .count()
    }
}

// ─────────────────────────────────────────────────────────────────────────
// push_with_hooks — the shared seam every daft push site routes through
// ─────────────────────────────────────────────────────────────────────────

/// A daft-initiated push, named by intent. [`push_with_hooks`] dispatches to
/// the matching `GitCommand` primitive.
#[derive(Debug, Clone, Copy)]
pub enum PushAction<'a> {
    /// `git push --set-upstream <remote> <branch>`
    SetUpstream { remote: &'a str, branch: &'a str },
    /// `git push <remote> <branch>` (optionally `--force-with-lease`)
    Sync {
        remote: &'a str,
        branch: &'a str,
        force_with_lease: bool,
    },
    /// `git push <remote> --delete <branch>`
    Delete { remote: &'a str, branch: &'a str },
}

impl PushAction<'_> {
    fn branch(&self) -> &str {
        match self {
            PushAction::SetUpstream { branch, .. }
            | PushAction::Sync { branch, .. }
            | PushAction::Delete { branch, .. } => branch,
        }
    }

    fn is_delete(&self) -> bool {
        matches!(self, PushAction::Delete { .. })
    }

    fn remote(&self) -> &str {
        match self {
            PushAction::SetUpstream { remote, .. }
            | PushAction::Sync { remote, .. }
            | PushAction::Delete { remote, .. } => remote,
        }
    }

    /// Human preview of the underlying git command (verbose job display).
    fn preview(&self) -> String {
        match self {
            PushAction::SetUpstream { remote, branch } => {
                format!("git push --set-upstream {remote} {branch}")
            }
            PushAction::Sync {
                remote,
                branch,
                force_with_lease: false,
            } => format!("git push {remote} {branch}"),
            PushAction::Sync {
                remote,
                branch,
                force_with_lease: true,
            } => format!("git push --force-with-lease {remote} {branch}"),
            PushAction::Delete { remote, branch } => {
                format!("git push {remote} --delete {branch}")
            }
        }
    }

    fn run(&self, git: &GitCommand, cwd: &Path, opts: &PushOptions) -> Result<PushIo> {
        match *self {
            PushAction::SetUpstream { remote, branch } => {
                git.push_set_upstream_from(remote, branch, cwd, opts)
            }
            PushAction::Sync {
                remote,
                branch,
                force_with_lease,
            } => git.push_from(remote, branch, cwd, force_with_lease, opts),
            PushAction::Delete { remote, branch } => {
                git.push_delete_from(remote, branch, cwd, opts)
            }
        }
    }
}

/// Coarse verdict on the `pre-push` gate for one push.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HookVerdict {
    /// No pre-push hook is installed; the push ran ungated.
    #[default]
    NoHook,
    /// A hook is installed and the push got past it (the push itself may
    /// still have failed further along, e.g. non-fast-forward).
    Passed,
    /// The push failed before any ref was negotiated with a hook installed —
    /// the local gate (or pre-negotiation transport) refused it.
    Rejected,
    /// The caller passed `--no-verify`; the installed hook was bypassed.
    Bypassed,
}

impl HookVerdict {
    /// Honest, one-line cause for a *failed* gated push, framed to what daft
    /// can actually observe on Path A — git's hook dispatch is opaque, so daft
    /// only sees whether any ref was negotiated before the push died:
    /// - `Rejected`: nothing was negotiated. The repo's pre-push hook refusing
    ///   the push produces this, but so does a pre-negotiation transport error
    ///   (unreachable remote, auth). #599 deliberately does not parse
    ///   locale-dependent stderr to tell them apart, so the message names both
    ///   rather than asserting the hook is to blame.
    /// - `Passed`: refs were negotiated, so the hook accepted the push; the
    ///   remote rejected it downstream (non-fast-forward, permissions).
    ///
    /// The underlying git error travels separately (in `PushOutcome::failure`)
    /// and carries the real diagnostic; this only frames it.
    pub fn failure_cause(self) -> &'static str {
        match self {
            HookVerdict::Rejected => {
                "the repo's pre-push hook may have blocked it, or the remote was unreachable"
            }
            HookVerdict::Passed => "the pre-push hook passed but the remote rejected the push",
            HookVerdict::Bypassed | HookVerdict::NoHook => "the push failed",
        }
    }

    /// Whether re-running with `--no-verify` could plausibly let the push
    /// through. Only when the push never negotiated a ref (`Rejected`) is a
    /// local pre-push hook a candidate cause; a `Passed` push already cleared
    /// the hook, so bypassing it would not change a downstream remote
    /// rejection.
    pub fn no_verify_might_help(self) -> bool {
        matches!(self, HookVerdict::Rejected)
    }
}

/// Caller-facing result of [`push_with_hooks`]. `Err` from the function is
/// reserved for spawn-level problems; a push that ran and failed lands here
/// in `failure` so call sites can grade severity (warn vs abort) by verdict.
#[derive(Debug)]
pub struct PushOutcome {
    /// Every pushed ref was already up to date (porcelain `=`).
    pub up_to_date: bool,
    pub hook: HookVerdict,
    /// `Some(message)` when the push failed.
    pub failure: Option<String>,
}

impl PushOutcome {
    pub fn success(&self) -> bool {
        self.failure.is_none()
    }

    pub fn hook_rejected(&self) -> bool {
        self.hook == HookVerdict::Rejected
    }

    /// Collapse into the legacy contract: bail with the failure message.
    pub fn into_result(self) -> Result<Self> {
        match self.failure {
            None => Ok(self),
            Some(msg) => Err(anyhow::anyhow!(msg)),
        }
    }
}

/// Run one daft push with the repo's `pre-push` stage honored and reported.
///
/// Two paths (#599):
/// - **Path B** — `stage.manages_stage("pre-push", cwd)`: daft runs its own
///   stage (per-job reporting via `presenter`), then pushes with git's hook
///   dispatch suppressed so an incumbent hook is not double-fired. Until
///   #468 ships a real adapter, `NoopStageRunner` never takes this path.
/// - **Path A** — otherwise: the push runs with hooks honored; git itself
///   dispatches whatever `pre-push` hook is installed (native, lefthook,
///   husky, pre-commit) as one opaque subprocess. When a hook exists and a
///   presenter is given, the run is reported as a single synthetic
///   `pre-push` phase + job carrying the teed git output — existence-gated
///   so hook-less repos render nothing extra.
///
/// `verify: false` is the explicit `--no-verify` opt-out (the old behavior).
/// `hook_present` short-circuits the existence probe when the caller
/// already resolved it (e.g. once per repo for sync's many worktrees).
pub fn push_with_hooks(
    git: &GitCommand,
    action: PushAction<'_>,
    cwd: &Path,
    verify: bool,
    stage: &dyn StageRunner,
    presenter: Option<&Arc<dyn JobPresenter>>,
    hook_present: Option<bool>,
) -> Result<PushOutcome> {
    let hook_present = hook_present.unwrap_or_else(|| git.pre_push_hook_exists(cwd));

    // Explicit opt-out: push with git's hook dispatch suppressed.
    if !verify {
        let io = action.run(
            git,
            cwd,
            &PushOptions {
                verify: false,
                on_output: None,
            },
        )?;
        return Ok(collapse(io, hook_present, true));
    }

    // Path B — daft owns the stage (#468 adapters; Noop never gets here).
    if stage.manages_stage("pre-push", cwd) {
        let refs = compute_push_refs(cwd, &action);
        let stage_presenter = presenter
            .map(Arc::clone)
            .unwrap_or_else(|| crate::executor::presenter::NullPresenter::arc());
        let outcome = stage.run_stage("pre-push", Some(cwd), &refs, stage_presenter)?;
        if !outcome.success {
            return Ok(PushOutcome {
                up_to_date: false,
                hook: HookVerdict::Rejected,
                failure: Some(
                    outcome
                        .reason
                        .unwrap_or_else(|| "pre-push stage failed".to_string()),
                ),
            });
        }
        // Stage passed: push without letting git re-fire the incumbent.
        let io = action.run(
            git,
            cwd,
            &PushOptions {
                verify: false,
                on_output: None,
            },
        )?;
        let mut collapsed = collapse(io, hook_present, false);
        if collapsed.hook == HookVerdict::Bypassed || collapsed.hook == HookVerdict::NoHook {
            collapsed.hook = HookVerdict::Passed;
        }
        return Ok(collapsed);
    }

    // Path A — git dispatches the hook; report the opaque run when present.
    match presenter {
        Some(presenter) if hook_present => {
            presenter.on_phase_start("pre-push", Some(action.branch()));
            presenter.on_job_start("pre-push", None, Some(&action.preview()));
            let started = Instant::now();
            let tee = |_stream: PushStream, line: &str| {
                presenter.on_job_output("pre-push", line);
            };
            let result = action.run(
                git,
                cwd,
                &PushOptions {
                    verify: true,
                    on_output: Some(&tee),
                },
            );
            let elapsed = started.elapsed();
            // The synthetic job tracks the whole `git push` subprocess, so a
            // failure here marks `pre-push: ✗` even when the hook passed and
            // the push died later (non-fast-forward, transport). That coarse
            // attribution is the accepted Path A tradeoff — git's hook dispatch
            // is opaque, so daft can't pin the failure on the hook specifically.
            // The caller-facing message disambiguates via `HookVerdict`.
            match &result {
                Ok(io) if io.success => presenter.on_job_success("pre-push", elapsed),
                _ => presenter.on_job_failure("pre-push", elapsed),
            }
            presenter.on_phase_complete(elapsed);
            Ok(collapse(result?, true, false))
        }
        _ => {
            let io = action.run(git, cwd, &PushOptions::default())?;
            Ok(collapse(io, hook_present, false))
        }
    }
}

/// Fold a finished push subprocess into the caller-facing outcome.
fn collapse(io: PushIo, hook_present: bool, bypassed: bool) -> PushOutcome {
    let report = parse_push_report(&io.stdout);
    let hook = if !hook_present {
        HookVerdict::NoHook
    } else if bypassed {
        HookVerdict::Bypassed
    } else if !io.success && !report.has_ref_lines() {
        // The push died before any ref was negotiated (E0: a pre-push
        // refusal emits zero porcelain ref lines) — the gate said no.
        HookVerdict::Rejected
    } else {
        HookVerdict::Passed
    };

    if io.success {
        PushOutcome {
            up_to_date: report.all_up_to_date(),
            hook,
            failure: None,
        }
    } else {
        let stderr = io.stderr.trim();
        let message = if stderr.is_empty() {
            "Git push failed".to_string()
        } else {
            format!("Git push failed: {stderr}")
        };
        PushOutcome {
            up_to_date: false,
            hook,
            failure: Some(message),
        }
    }
}

/// Best-effort construction of the refs a push will update, in the 4-field
/// shape git feeds a `pre-push` hook (Path B's `run_stage` input). The zero
/// oid is the protocol's "unknown/absent" on either side.
fn compute_push_refs(cwd: &Path, action: &PushAction<'_>) -> Vec<PushRef> {
    let branch_ref = format!("refs/heads/{}", action.branch());
    let tracking_ref = format!("refs/remotes/{}/{}", action.remote(), action.branch());
    let local_oid = rev_parse_oid(cwd, &branch_ref);
    let remote_oid = rev_parse_oid(cwd, &tracking_ref);

    if action.is_delete() {
        vec![PushRef {
            local_ref: "(delete)".to_string(),
            local_oid: zero_oid_like(remote_oid.as_deref()),
            remote_ref: branch_ref,
            remote_oid: remote_oid.unwrap_or_else(|| zero_oid_like(None)),
        }]
    } else {
        vec![PushRef {
            local_ref: branch_ref.clone(),
            local_oid: local_oid.clone().unwrap_or_else(|| zero_oid_like(None)),
            remote_ref: branch_ref,
            remote_oid: remote_oid.unwrap_or_else(|| zero_oid_like(local_oid.as_deref())),
        }]
    }
}

fn rev_parse_oid(cwd: &Path, refname: &str) -> Option<String> {
    let mut cmd = git_command_at(cwd);
    cmd.args(["rev-parse", "--verify", "--quiet", refname]);
    cmd.stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!oid.is_empty()).then_some(oid)
}

/// Zero oid sized to match its counterpart (SHA-1 vs SHA-256 repos).
fn zero_oid_like(counterpart: Option<&str>) -> String {
    "0".repeat(counterpart.map_or(40, str::len))
}

/// Execute the push operation across all worktrees (sequential path).
pub fn execute(
    params: &PushParams,
    git: &GitCommand,
    project_root: &Path,
    progress: &mut dyn ProgressSink,
    exclude_branches: &HashSet<String>,
    stage: &dyn StageRunner,
    presenter: Option<&Arc<dyn JobPresenter>>,
) -> Result<PushResult> {
    let original_dir = get_current_directory()?;
    let worktrees = fetch::get_all_worktrees_with_branches(git)?;

    // All worktrees share one hooks dir — probe once for the whole pass.
    let hook_present = git.pre_push_hook_exists(project_root);

    let mut results: Vec<WorktreePushResult> = Vec::new();

    for (path, branch) in &worktrees {
        if exclude_branches.contains(branch) {
            continue;
        }

        let worktree_name = path
            .strip_prefix(project_root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("unknown")
            .to_string();

        progress.on_step(&format!("Pushing '{worktree_name}'"));

        let result = push_single_worktree(
            git,
            path,
            &worktree_name,
            branch,
            params,
            progress,
            stage,
            presenter,
            Some(hook_present),
        );
        results.push(result);
    }

    change_directory(&original_dir)?;

    Ok(PushResult {
        results,
        remote_name: params.remote_name.clone(),
    })
}

/// Push a single worktree branch to its remote tracking branch.
///
/// Checks for an upstream tracking remote first; skips if none is set.
/// Uses an explicit working directory for thread-safe parallel execution.
///
/// `hook_present` short-circuits the per-push hook probe when the caller
/// already resolved it for the repo (all worktrees share one hooks dir).
#[allow(clippy::too_many_arguments)]
pub fn push_single_worktree(
    git: &GitCommand,
    worktree_path: &Path,
    worktree_name: &str,
    branch_name: &str,
    params: &PushParams,
    progress: &mut dyn ProgressSink,
    stage: &dyn StageRunner,
    presenter: Option<&Arc<dyn JobPresenter>>,
    hook_present: Option<bool>,
) -> WorktreePushResult {
    // Verify directory exists
    if !worktree_path.is_dir() {
        return WorktreePushResult {
            worktree_name: worktree_name.to_string(),
            branch_name: branch_name.to_string(),
            message: format!("Directory not found: {}", worktree_path.display()),
            ..Default::default()
        };
    }

    // Check if branch has upstream tracking
    match git.get_branch_tracking_remote_from(branch_name, worktree_path) {
        Ok(None) => {
            progress.on_warning(&format!(
                "Skipping '{worktree_name}': no upstream tracking branch"
            ));
            return WorktreePushResult {
                worktree_name: worktree_name.to_string(),
                branch_name: branch_name.to_string(),
                success: true,
                no_upstream: true,
                message: "No upstream tracking branch".to_string(),
                ..Default::default()
            };
        }
        Err(e) => {
            return WorktreePushResult {
                worktree_name: worktree_name.to_string(),
                branch_name: branch_name.to_string(),
                message: format!("Failed to check tracking remote: {e}"),
                ..Default::default()
            };
        }
        Ok(Some(_)) => {}
    }

    let action = PushAction::Sync {
        remote: &params.remote_name,
        branch: branch_name,
        force_with_lease: params.force_with_lease,
    };

    match push_with_hooks(
        git,
        action,
        worktree_path,
        !params.no_verify,
        stage,
        presenter,
        hook_present,
    ) {
        Ok(outcome) => {
            let hook = outcome.hook;
            match outcome.failure {
                None => WorktreePushResult {
                    worktree_name: worktree_name.to_string(),
                    branch_name: branch_name.to_string(),
                    success: true,
                    up_to_date: outcome.up_to_date,
                    hook,
                    message: if outcome.up_to_date {
                        "Already up to date".to_string()
                    } else {
                        "Pushed successfully".to_string()
                    },
                    ..Default::default()
                },
                Some(msg) => {
                    progress.on_warning(&format!("Failed to push '{worktree_name}': {msg}"));
                    WorktreePushResult {
                        worktree_name: worktree_name.to_string(),
                        branch_name: branch_name.to_string(),
                        hook,
                        message: msg,
                        ..Default::default()
                    }
                }
            }
        }
        Err(e) => {
            let msg = format!("{e}");
            progress.on_warning(&format!("Failed to push '{worktree_name}': {msg}"));
            WorktreePushResult {
                worktree_name: worktree_name.to_string(),
                branch_name: branch_name.to_string(),
                message: msg,
                ..Default::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::ports::{NoopStageRunner, StageOutcome};
    use std::process::Stdio;
    use std::sync::Mutex;
    use std::time::Duration;

    // ── Pure collapse() classification ──────────────────────────────────

    fn io(success: bool, stdout: &str, stderr: &str) -> PushIo {
        PushIo {
            success,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    const UP_TO_DATE: &str = "To /tmp/r.git\n=\trefs/heads/b:refs/heads/b\t[up to date]\nDone\n";
    const PUSHED: &str = "To /tmp/r.git\n \trefs/heads/b:refs/heads/b\ta..b\nDone\n";
    const NON_FF: &str =
        "To /tmp/r.git\n!\trefs/heads/b:refs/heads/b\t[rejected] (non-fast-forward)\nDone\n";

    #[test]
    fn collapse_success_without_hook() {
        let outcome = collapse(io(true, PUSHED, ""), false, false);
        assert!(outcome.success());
        assert!(!outcome.up_to_date);
        assert_eq!(outcome.hook, HookVerdict::NoHook);
    }

    #[test]
    fn collapse_up_to_date_with_passing_hook() {
        let outcome = collapse(io(true, UP_TO_DATE, ""), true, false);
        assert!(outcome.success());
        assert!(outcome.up_to_date);
        assert_eq!(outcome.hook, HookVerdict::Passed);
    }

    #[test]
    fn collapse_gate_refusal_is_rejected() {
        // Hook refusals die before ref negotiation: no porcelain ref lines.
        let outcome = collapse(io(false, "HOOK NOISE\n", "HOOK SAYS NO\n"), true, false);
        assert!(!outcome.success());
        assert_eq!(outcome.hook, HookVerdict::Rejected);
        assert!(outcome.failure.unwrap().contains("HOOK SAYS NO"));
    }

    #[test]
    fn collapse_non_ff_failure_means_hook_passed() {
        let outcome = collapse(io(false, NON_FF, "error: failed to push\n"), true, false);
        assert!(!outcome.success());
        assert_eq!(outcome.hook, HookVerdict::Passed);
    }

    #[test]
    fn collapse_bypass_wins_over_rejection_shape() {
        let outcome = collapse(io(true, PUSHED, ""), true, true);
        assert_eq!(outcome.hook, HookVerdict::Bypassed);
    }

    #[test]
    fn failure_cause_does_not_blame_the_hook_for_a_rejected_push() {
        // A `Rejected` push died before ref negotiation — that is the hook OR a
        // transport error, and daft must not assert the hook is to blame (the
        // review's Medium finding). The phrasing names both possibilities.
        let cause = HookVerdict::Rejected.failure_cause();
        assert!(
            cause.contains("pre-push hook"),
            "names the hook as a candidate"
        );
        assert!(
            cause.contains("unreachable"),
            "also names transport failure so a network error isn't blamed on the hook: {cause}"
        );
    }

    #[test]
    fn failure_cause_for_passed_blames_the_remote_not_the_hook() {
        // A `Passed` push cleared the hook; a downstream failure is the remote's
        // (non-fast-forward, perms). The message must not imply the hook rejected.
        let cause = HookVerdict::Passed.failure_cause();
        assert!(cause.contains("passed"), "states the hook passed");
        assert!(
            cause.contains("remote rejected"),
            "attributes the failure to the remote"
        );
    }

    #[test]
    fn no_verify_hint_only_offered_when_the_hook_is_a_candidate() {
        // `--no-verify` bypasses the local hook, so it only helps a `Rejected`
        // push; a `Passed`-then-failed push would fail the same way bypassed.
        assert!(HookVerdict::Rejected.no_verify_might_help());
        assert!(!HookVerdict::Passed.no_verify_might_help());
        assert!(!HookVerdict::NoHook.no_verify_might_help());
        assert!(!HookVerdict::Bypassed.no_verify_might_help());
    }

    // ── Recording fakes (reconcile.rs fake-adapter shape) ───────────────

    #[derive(Default)]
    struct RecordingPresenter {
        events: Mutex<Vec<String>>,
    }

    impl RecordingPresenter {
        fn arc() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }

        fn push(&self, event: String) {
            self.events.lock().unwrap().push(event);
        }
    }

    impl JobPresenter for RecordingPresenter {
        fn on_phase_start(&self, phase_name: &str, target: Option<&str>) {
            self.push(format!(
                "phase_start:{phase_name}:{}",
                target.unwrap_or("-")
            ));
        }
        fn on_job_start(&self, name: &str, _d: Option<&str>, _c: Option<&str>) {
            self.push(format!("job_start:{name}"));
        }
        fn on_job_output(&self, name: &str, line: &str) {
            self.push(format!("job_output:{name}:{line}"));
        }
        fn on_job_success(&self, name: &str, _duration: Duration) {
            self.push(format!("job_success:{name}"));
        }
        fn on_job_failure(&self, name: &str, _duration: Duration) {
            self.push(format!("job_failure:{name}"));
        }
        fn on_job_skipped(&self, name: &str, _r: &str, _d: Duration, _s: bool, _c: Option<&str>) {
            self.push(format!("job_skipped:{name}"));
        }
        fn on_job_cancelled(&self, name: &str, _duration: Duration) {
            self.push(format!("job_cancelled:{name}"));
        }
        fn on_job_background(&self, name: &str, _description: Option<&str>) {
            self.push(format!("job_background:{name}"));
        }
        fn on_message(&self, msg: &str) {
            self.push(format!("message:{msg}"));
        }
        fn on_phase_complete(&self, _total_duration: Duration) {
            self.push("phase_complete".to_string());
        }
        fn take_results(&self) -> Vec<crate::executor::JobResult> {
            Vec::new()
        }
    }

    /// Fake Path-B adapter: manages `pre-push`, records calls, returns a
    /// scripted verdict.
    struct FakeStageRunner {
        succeed: bool,
        calls: Mutex<Vec<(String, Vec<PushRef>)>>,
    }

    impl FakeStageRunner {
        fn new(succeed: bool) -> Self {
            Self {
                succeed,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl StageRunner for FakeStageRunner {
        fn manages_stage(&self, stage: &str, _repo_cwd: &Path) -> bool {
            stage == "pre-push"
        }

        fn run_stage(
            &self,
            stage: &str,
            _worktree_cwd: Option<&Path>,
            refs: &[PushRef],
            _presenter: Arc<dyn JobPresenter>,
        ) -> Result<StageOutcome> {
            self.calls
                .lock()
                .unwrap()
                .push((stage.to_string(), refs.to_vec()));
            Ok(StageOutcome {
                success: self.succeed,
                skipped: false,
                reason: (!self.succeed).then(|| "stage job failed".to_string()),
            })
        }
    }

    // ── End-to-end against real git in isolated temp repos ──────────────

    struct TestRepo {
        _dir: tempfile::TempDir,
        work: std::path::PathBuf,
        remote: std::path::PathBuf,
    }

    fn git_in(dir: &Path, args: &[&str]) -> std::process::Output {
        let mut cmd = git_command_at(dir);
        cmd.args(args)
            .envs([
                ("GIT_AUTHOR_NAME", "Test"),
                ("GIT_AUTHOR_EMAIL", "test@test.com"),
                ("GIT_COMMITTER_NAME", "Test"),
                ("GIT_COMMITTER_EMAIL", "test@test.com"),
            ])
            .stdin(Stdio::null());
        cmd.output().expect("git invocation failed")
    }

    fn assert_git(dir: &Path, args: &[&str]) {
        let out = git_in(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Local bare remote + one-commit clone on branch `main`.
    fn test_repo() -> TestRepo {
        let dir = tempfile::tempdir().expect("tempdir");
        let remote = dir.path().join("remote.git");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&remote).unwrap();
        assert_git(&remote, &["init", "--bare", "--quiet", "."]);
        std::fs::create_dir_all(&work).unwrap();
        assert_git(&work, &["init", "--quiet", "-b", "main", "."]);
        assert_git(
            &work,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(work.join("a.txt"), "hi\n").unwrap();
        assert_git(&work, &["add", "a.txt"]);
        assert_git(&work, &["commit", "--quiet", "-m", "init"]);
        TestRepo {
            _dir: dir,
            work,
            remote,
        }
    }

    fn install_pre_push(repo: &TestRepo, script: &str) {
        let hooks = repo.work.join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("pre-push");
        std::fs::write(&hook, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn remote_has_branch(repo: &TestRepo, branch: &str) -> bool {
        let out = git_in(
            &repo.remote,
            &["show-ref", "--verify", &format!("refs/heads/{branch}")],
        );
        out.status.success()
    }

    fn sync_main<'a>() -> PushAction<'a> {
        PushAction::Sync {
            remote: "origin",
            branch: "main",
            force_with_lease: false,
        }
    }

    #[test]
    fn failing_hook_rejects_push_and_reports_synthetic_job() {
        let repo = test_repo();
        install_pre_push(&repo, "#!/bin/sh\necho \"GATE SAYS NO\" >&2\nexit 1\n");
        let git = GitCommand::new(false);
        let presenter = RecordingPresenter::arc();
        let presenter_dyn: Arc<dyn JobPresenter> = presenter.clone();

        let outcome = push_with_hooks(
            &git,
            sync_main(),
            &repo.work,
            true,
            &NoopStageRunner,
            Some(&presenter_dyn),
            None,
        )
        .unwrap();

        assert!(!outcome.success());
        assert_eq!(outcome.hook, HookVerdict::Rejected);
        assert!(outcome.failure.as_deref().unwrap().contains("GATE SAYS NO"));
        assert!(!remote_has_branch(&repo, "main"), "push must be blocked");

        let events = presenter.events();
        assert_eq!(events.first().unwrap(), "phase_start:pre-push:main");
        assert!(events.contains(&"job_start:pre-push".to_string()));
        assert!(
            events
                .iter()
                .any(|e| e.starts_with("job_output:pre-push:") && e.contains("GATE SAYS NO")),
            "hook stderr must be teed through the presenter: {events:?}"
        );
        assert!(events.contains(&"job_failure:pre-push".to_string()));
        assert_eq!(events.last().unwrap(), "phase_complete");
    }

    #[test]
    fn passing_hook_allows_push_and_reports_success() {
        let repo = test_repo();
        install_pre_push(&repo, "#!/bin/sh\necho \"gate ok\"\nexit 0\n");
        let git = GitCommand::new(false);
        let presenter = RecordingPresenter::arc();
        let presenter_dyn: Arc<dyn JobPresenter> = presenter.clone();

        let outcome = push_with_hooks(
            &git,
            sync_main(),
            &repo.work,
            true,
            &NoopStageRunner,
            Some(&presenter_dyn),
            None,
        )
        .unwrap();

        assert!(outcome.success(), "failure: {:?}", outcome.failure);
        assert_eq!(outcome.hook, HookVerdict::Passed);
        assert!(!outcome.up_to_date);
        assert!(remote_has_branch(&repo, "main"));
        let events = presenter.events();
        assert!(events.contains(&"job_success:pre-push".to_string()));
    }

    #[test]
    fn no_verify_bypasses_failing_hook_without_phase() {
        let repo = test_repo();
        install_pre_push(&repo, "#!/bin/sh\nexit 1\n");
        let git = GitCommand::new(false);
        let presenter = RecordingPresenter::arc();
        let presenter_dyn: Arc<dyn JobPresenter> = presenter.clone();

        let outcome = push_with_hooks(
            &git,
            sync_main(),
            &repo.work,
            false, // --no-verify passthrough
            &NoopStageRunner,
            Some(&presenter_dyn),
            None,
        )
        .unwrap();

        assert!(outcome.success());
        assert_eq!(outcome.hook, HookVerdict::Bypassed);
        assert!(remote_has_branch(&repo, "main"));
        assert!(
            presenter.events().is_empty(),
            "bypassed pushes render no synthetic phase"
        );
    }

    #[test]
    fn hookless_repo_renders_no_phase() {
        let repo = test_repo();
        let git = GitCommand::new(false);
        let presenter = RecordingPresenter::arc();
        let presenter_dyn: Arc<dyn JobPresenter> = presenter.clone();

        let outcome = push_with_hooks(
            &git,
            sync_main(),
            &repo.work,
            true,
            &NoopStageRunner,
            Some(&presenter_dyn),
            None,
        )
        .unwrap();

        assert!(outcome.success());
        assert_eq!(outcome.hook, HookVerdict::NoHook);
        assert!(
            presenter.events().is_empty(),
            "existence gate must suppress the synthetic phase"
        );
    }

    #[test]
    fn up_to_date_detection_survives_hook_reporting() {
        let repo = test_repo();
        install_pre_push(&repo, "#!/bin/sh\nexit 0\n");
        let git = GitCommand::new(false);

        let first = push_with_hooks(
            &git,
            sync_main(),
            &repo.work,
            true,
            &NoopStageRunner,
            None,
            None,
        )
        .unwrap();
        assert!(first.success() && !first.up_to_date);

        let second = push_with_hooks(
            &git,
            sync_main(),
            &repo.work,
            true,
            &NoopStageRunner,
            None,
            None,
        )
        .unwrap();
        assert!(second.success());
        assert!(second.up_to_date, "second push must classify as up-to-date");
    }

    #[test]
    fn managing_stage_runner_failure_blocks_push() {
        let repo = test_repo();
        let git = GitCommand::new(false);
        let runner = FakeStageRunner::new(false);

        let outcome =
            push_with_hooks(&git, sync_main(), &repo.work, true, &runner, None, None).unwrap();

        assert!(!outcome.success());
        assert_eq!(outcome.hook, HookVerdict::Rejected);
        assert!(
            !remote_has_branch(&repo, "main"),
            "stage failure gates the push"
        );

        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (stage, refs) = &calls[0];
        assert_eq!(stage, "pre-push");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].local_ref, "refs/heads/main");
        assert_ne!(refs[0].local_oid, zero_oid_like(None));
        assert_eq!(refs[0].remote_oid, zero_oid_like(None), "new remote ref");
    }

    #[test]
    fn managing_stage_runner_success_pushes_without_refiring_incumbent() {
        let repo = test_repo();
        // A failing native hook proves Path B suppresses git's dispatch:
        // if git re-fired it, the push would be rejected.
        install_pre_push(&repo, "#!/bin/sh\nexit 1\n");
        let git = GitCommand::new(false);
        let runner = FakeStageRunner::new(true);

        let outcome =
            push_with_hooks(&git, sync_main(), &repo.work, true, &runner, None, None).unwrap();

        assert!(outcome.success(), "failure: {:?}", outcome.failure);
        assert_eq!(outcome.hook, HookVerdict::Passed);
        assert!(remote_has_branch(&repo, "main"));
    }

    #[test]
    fn delete_action_builds_delete_shaped_refs() {
        let repo = test_repo();
        let refs = compute_push_refs(
            &repo.work,
            &PushAction::Delete {
                remote: "origin",
                branch: "main",
            },
        );
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].local_ref, "(delete)");
        assert_eq!(refs[0].remote_ref, "refs/heads/main");
        assert_eq!(refs[0].local_oid, zero_oid_like(None));
    }
}
