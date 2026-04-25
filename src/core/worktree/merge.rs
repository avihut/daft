//! Core logic for `daft merge`.
//!
//! This module owns the business logic for starting a merge in a target
//! worktree. In Slice 2 the scope is intentionally minimal: dispatch
//! `git merge <sources...>` in the target directory and report whether the
//! invocation conflicted. Richer outcome detection (already-up-to-date,
//! fast-forward vs. true merge, octopus announcements, target resolution)
//! lands in later slices.
//!
//! The direct use of `std::process::Command::new("git")` here is a Slice-2
//! shortcut. A later slice will replace it with a `GitCommand::merge_in`
//! helper (analogous to `GitCommand::rebase_in` in `src/git/remote.rs`) once
//! we need to capture stdout/stderr to detect signals like
//! "Already up to date." and distinguish true merge conflicts from other
//! failure modes.
//!
//! # Parameters
//!
//! [`StartParams`] captures the inputs to a merge start: the list of source
//! refs that will be merged into the target worktree's current branch.
//!
//! # Outcome
//!
//! [`StartOutcome`] reports the result: whether the merge was a no-op because
//! the target was already up to date, and whether the `git merge` invocation
//! exited non-zero. Later slices will expand this to distinguish fast-forward,
//! true merge, and octopus cases, and to separate real conflicts from other
//! failure modes.

use crate::git::GitCommand;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Inputs to a merge-start operation.
#[derive(Debug, Clone)]
pub struct StartParams {
    /// One or more source refs to merge into the target worktree's branch.
    pub sources: Vec<String>,
    /// Optional target worktree/branch. `None` → current worktree's branch.
    pub target: Option<String>,
    /// Git passthrough flags. Serialized into the `git merge` argv between the
    /// `merge` keyword and the source list.
    pub flags: EffectiveFlags,
    /// Caller's stance on adopting a ref-only target via an ephemeral worktree
    /// when the merge cannot fast-forward. Consulted only on the ref-only
    /// non-FF branch of [`execute_start`]; ignored elsewhere.
    pub adopt: AdoptChoice,
    /// Require the target worktree to be clean (no uncommitted changes) before
    /// starting the merge. Sourced by the command layer from
    /// `daft.merge.requireCleanTarget`; defaults to `true`.
    pub require_clean_target: bool,
    /// When set, the core writes this as the daft-merge-intent.json marker
    /// file BEFORE invoking `git commit` on the squash path. This allows
    /// `--continue` to resume cleanup after an editor abort. The core removes
    /// the marker on a successful commit. `None` means no cleanup was
    /// requested (or the path is `--no-commit`).
    pub cleanup_intent: Option<MergeIntentTemplate>,
}

/// Template for the cleanup intent marker, provided by the command layer
/// before `source_shas` are available.
///
/// The core fills in `source_shas` from the captured SHAs before writing
/// the final `MergeIntent` JSON.
#[derive(Debug, Clone)]
pub struct MergeIntentTemplate {
    pub remove_worktree: bool,
    pub also_branch: bool,
}

impl Default for StartParams {
    /// Default [`StartParams`]: no sources, current-worktree target, default
    /// flags and adopt state, and `require_clean_target = true` to match the
    /// safety-first config default. Used by unit tests; the command layer
    /// always constructs StartParams explicitly from CLI args + settings.
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            target: None,
            flags: EffectiveFlags::default(),
            adopt: AdoptChoice::default(),
            require_clean_target: true,
            cleanup_intent: None,
        }
    }
}

/// Caller-provided flag state for the adopt-target decision.
///
/// Maps to the CLI plus settings:
/// * `adopt_target` — `--adopt-target` was passed.
/// * `no_adopt_target` — `--no-adopt-target` was passed.
/// * `yes` — `-y`/`--yes` was passed.
/// * `preset` — user-configured default from
///   `daft.merge.adoptTargetOnDemand`; used by [`decide_adopt`] when neither
///   explicit adopt flag is set.
///
/// Clap enforces at parse time that `adopt_target` and `no_adopt_target` are
/// not both set. `yes` is orthogonal and coerces to `adopt_target` only when
/// neither adopt flag is set (see [`resolve_adopt_flags`]).
#[derive(Debug, Default, Clone, Copy)]
pub struct AdoptChoice {
    pub adopt_target: bool,
    pub no_adopt_target: bool,
    pub yes: bool,
    pub preset: AdoptPreset,
}

/// User-configured default for adopt-target behavior.
///
/// Drives [`decide_adopt`] when neither explicit adopt flag is set. Sourced
/// from `daft.merge.adoptTargetOnDemand` via `DaftSettings` and threaded
/// through [`AdoptChoice::preset`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AdoptPreset {
    /// Prompt the user in TTY, refuse in non-TTY (default).
    #[default]
    Prompt,
    /// Always adopt.
    Yes,
    /// Always refuse.
    No,
}

/// Result of the adopt-target decision.
///
/// `Yes`/`No` are final; `Ask` means the caller must prompt the user (e.g.
/// via [`ask_adopt_user`]) and proceed based on the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptDecision {
    /// Proceed with ephemeral worktree creation.
    Yes,
    /// Refuse; bail with guidance.
    No,
    /// Prompt the user interactively.
    Ask,
}

/// Decide whether to create an ephemeral worktree.
///
/// Flags win over preset; `--adopt-target` and `--no-adopt-target` are
/// mutually exclusive (enforced by clap upstream). Without flags, `preset`
/// drives the decision: `Yes`/`No` are always final, and `Prompt` returns
/// `Ask` when stdin is a TTY or `No` otherwise so piped/non-interactive
/// invocations never hang.
///
/// Pure — no I/O, no global state. The `is_tty` input is the caller's
/// observation of stdin at the moment of the decision.
pub fn decide_adopt(
    flag_yes: bool,
    flag_no: bool,
    is_tty: bool,
    preset: AdoptPreset,
) -> AdoptDecision {
    if flag_yes {
        return AdoptDecision::Yes;
    }
    if flag_no {
        return AdoptDecision::No;
    }
    match (preset, is_tty) {
        (AdoptPreset::Yes, _) => AdoptDecision::Yes,
        (AdoptPreset::No, _) => AdoptDecision::No,
        (AdoptPreset::Prompt, true) => AdoptDecision::Ask,
        (AdoptPreset::Prompt, false) => AdoptDecision::No,
    }
}

/// Coerce `-y` into an explicit `--adopt-target` when neither adopt flag is
/// supplied, emitting a single-line announcement for traceability.
///
/// Returns `(flag_yes, flag_no)` ready to feed into [`decide_adopt`]. When
/// `-y` triggered the coercion we log to stderr so the invocation stays
/// self-describing in scripts and CI logs. If either explicit adopt flag is
/// already set, `-y` is a no-op on this axis (explicit wins over implicit);
/// no announcement is emitted.
pub fn resolve_adopt_flags(adopt: &AdoptChoice) -> (bool, bool) {
    if adopt.yes && !adopt.adopt_target && !adopt.no_adopt_target {
        eprintln!("merge: auto-accepting adopt-target prompt because -y was passed");
        (true, false)
    } else {
        (adopt.adopt_target, adopt.no_adopt_target)
    }
}

/// Prompt the user to confirm ephemeral worktree creation for a non-FF merge
/// against a ref-only target.
///
/// Writes the prompt to stderr (keeping stdout clean for the final status
/// line) and reads a single line from stdin. Only an exact `y`/`yes`
/// (case-insensitive, trimmed) returns true; any other response — including
/// an empty line or EOF — is treated as "no", matching the `[y/N]` default.
pub fn ask_adopt_user(target_branch: &str) -> Result<bool> {
    use std::io::{self, Write};
    eprint!(
        "target '{}' has no worktree and this merge cannot fast-forward.\n\
         create an ephemeral worktree to perform the merge? [y/N] ",
        target_branch
    );
    io::stderr().flush().ok();
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .context("failed to read answer for adopt-target prompt")?;
    Ok(matches!(
        buf.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Fast-forward mode explicitly requested on the CLI.
///
/// `Auto` corresponds to `--ff` (git's default behavior), `Only` to `--ff-only`
/// (refuse if FF is not possible), and `Never` to `--no-ff` (always create a
/// merge commit). `None` on [`EffectiveFlags::ff`] means the user didn't
/// specify and git's own default (effectively `Auto`) applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfMode {
    Auto,
    Only,
    Never,
}

/// GPG signing request for the merge commit.
///
/// * `Default` — `-S` with no value; git uses `user.signingKey` or the
///   default secret key.
/// * `KeyId(id)` — `-S<KEYID>`; sign with a specific key.
/// * `Disabled` — `--no-gpg-sign`; explicitly disable signing even if
///   `commit.gpgsign` is set in config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpgSign {
    Default,
    KeyId(String),
    Disabled,
}

/// All git-merge passthrough flags resolved to their effective values.
///
/// Each field is `Option<_>` (or a bool for truly additive flags) so that
/// `None` means "user didn't specify — let git use its default". This keeps
/// [`render_flags`] output minimal: we only emit a flag when the user asked
/// for one, and we never stamp a redundant default onto git's argv.
///
/// Slice-13 will layer repo/global config defaults in; for now these values
/// come strictly from the CLI.
#[derive(Debug, Default, Clone)]
pub struct EffectiveFlags {
    pub message: Option<String>,
    pub file: Option<PathBuf>,
    pub edit: Option<bool>,
    pub cleanup: Option<String>,
    pub ff: Option<FfMode>,
    pub squash: Option<bool>,
    pub commit: Option<bool>,
    pub signoff: Option<bool>,
    pub strategy: Option<String>,
    pub strategy_options: Vec<String>,
    pub gpg_sign: Option<GpgSign>,
    pub verify_signatures: Option<bool>,
    pub allow_unrelated_histories: bool,
    pub stat: Option<bool>,
}

impl EffectiveFlags {
    /// Returns `true` when a `--squash` merge would need to open an editor
    /// to compose a commit message.
    ///
    /// This is the case when:
    /// - `--squash` is enabled (`squash == Some(true)`)
    /// - `--no-commit` / `commit == false` is NOT set (i.e. we will commit)
    /// - no commit message is supplied via `-m` / `--message`
    /// - no message file is supplied via `-F` / `--file`
    /// - `--no-edit` is NOT set (`edit != Some(false)`)
    ///
    /// The pre-flight TTY guard calls this to refuse before any merge work
    /// runs when stdin is not a terminal.
    pub fn squash_would_open_editor(&self) -> bool {
        matches!(self.squash, Some(true))
            && !matches!(self.commit, Some(false))
            && self.message.is_none()
            && self.file.is_none()
            && !matches!(self.edit, Some(false))
    }
}

/// Serialize [`EffectiveFlags`] into `git merge` argv fragments.
///
/// The caller splices the returned vector between the `merge` keyword and the
/// source refs, producing e.g. `git merge --no-ff -m "msg" feature/x`.
///
/// Order of emitted flags follows the struct's field order, which mirrors
/// git's own reference grouping (message/editor, ff, squash, commit, signoff,
/// strategy, gpg, verify, allow-unrelated, stat). Order isn't semantically
/// meaningful for git — none of these flags interact positionally — but a
/// stable order makes test assertions deterministic and diffs readable.
///
/// When `squash == Some(true)`, message-composing flags (`-m`, `-F`,
/// `--edit`/`--no-edit`, `--cleanup`, `--signoff`, `-S`/`--no-gpg-sign`) are
/// **omitted** from the merge argv. They are commit-time concerns and are
/// forwarded to the subsequent `git commit` step via [`render_commit_flags`].
/// Passing them to `git merge --squash` would be silently accepted but have no
/// effect. The non-squash path keeps emitting them so existing regular-merge
/// behavior is unchanged.
pub fn render_flags(flags: &EffectiveFlags) -> Vec<String> {
    let is_squash = flags.squash == Some(true);
    let mut out: Vec<String> = Vec::new();
    // Message-composing flags: emit only for non-squash paths. For squash
    // merges these are forwarded to `git commit` via render_commit_flags.
    if !is_squash {
        if let Some(m) = &flags.message {
            out.extend(["-m".into(), m.clone()]);
        }
        if let Some(f) = &flags.file {
            out.extend(["-F".into(), f.display().to_string()]);
        }
        match flags.edit {
            Some(true) => out.push("--edit".into()),
            Some(false) => out.push("--no-edit".into()),
            None => {}
        }
    }
    // cleanup: emit only for non-squash paths. For squash merges this is
    // forwarded to `git commit` via render_commit_flags.
    if !is_squash {
        if let Some(c) = &flags.cleanup {
            out.extend(["--cleanup".into(), c.clone()]);
        }
    }
    match flags.ff {
        Some(FfMode::Auto) => out.push("--ff".into()),
        Some(FfMode::Only) => out.push("--ff-only".into()),
        Some(FfMode::Never) => out.push("--no-ff".into()),
        None => {}
    }
    match flags.squash {
        Some(true) => out.push("--squash".into()),
        Some(false) => out.push("--no-squash".into()),
        None => {}
    }
    match flags.commit {
        Some(true) => out.push("--commit".into()),
        Some(false) => out.push("--no-commit".into()),
        None => {}
    }
    // signoff: emit only for non-squash paths.
    if !is_squash {
        match flags.signoff {
            Some(true) => out.push("--signoff".into()),
            Some(false) => out.push("--no-signoff".into()),
            None => {}
        }
    }
    if let Some(s) = &flags.strategy {
        out.extend(["-s".into(), s.clone()]);
    }
    for x in &flags.strategy_options {
        out.extend(["-X".into(), x.clone()]);
    }
    // gpg-sign: emit only for non-squash paths.
    if !is_squash {
        match &flags.gpg_sign {
            Some(GpgSign::Default) => out.push("-S".into()),
            Some(GpgSign::KeyId(k)) => out.push(format!("-S{k}")),
            Some(GpgSign::Disabled) => out.push("--no-gpg-sign".into()),
            None => {}
        }
    }
    match flags.verify_signatures {
        Some(true) => out.push("--verify-signatures".into()),
        Some(false) => out.push("--no-verify-signatures".into()),
        None => {}
    }
    if flags.allow_unrelated_histories {
        out.push("--allow-unrelated-histories".into());
    }
    match flags.stat {
        Some(true) => out.push("--stat".into()),
        Some(false) => out.push("--no-stat".into()),
        None => {}
    }
    out
}

/// Serialize commit-time flags from [`EffectiveFlags`] into `git commit` argv.
///
/// These flags are message-composing or signing concerns that apply to the
/// `git commit` step after `git merge --squash`, NOT to `git merge` itself.
/// For non-squash merges these flags are passed to `git merge` via
/// [`render_flags`]; this function should not be called in that path.
///
/// Flags emitted (when set):
/// * `-m <msg>` from `flags.message`
/// * `-F <path>` from `flags.file`
/// * `--edit` / `--no-edit` from `flags.edit`
/// * `--cleanup <mode>` from `flags.cleanup`
/// * `--signoff` from `flags.signoff` (only `Some(true)` — `--no-signoff` is
///   a no-op for commits and omitted)
/// * `-S` / `-S<keyid>` / `--no-gpg-sign` from `flags.gpg_sign`
pub fn render_commit_flags(flags: &EffectiveFlags) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(m) = &flags.message {
        out.extend(["-m".into(), m.clone()]);
    }
    if let Some(f) = &flags.file {
        out.extend(["-F".into(), f.display().to_string()]);
    }
    match flags.edit {
        Some(true) => out.push("--edit".into()),
        Some(false) => out.push("--no-edit".into()),
        None => {}
    }
    if let Some(c) = &flags.cleanup {
        out.extend(["--cleanup".into(), c.clone()]);
    }
    if flags.signoff == Some(true) {
        out.push("--signoff".into());
    }
    match &flags.gpg_sign {
        Some(GpgSign::Default) => out.push("-S".into()),
        Some(GpgSign::KeyId(k)) => out.push(format!("-S{k}")),
        Some(GpgSign::Disabled) => out.push("--no-gpg-sign".into()),
        None => {}
    }
    out
}

/// Target of a merge after resolution.
///
/// `path` is `None` when the user named a target branch that exists as a ref
/// but has no checked-out worktree (Slice 9). With a worktree present, `path`
/// holds its absolute filesystem path; without one, Slice 9's FF-plumbing and
/// Slice 10's ephemeral-worktree flows take over.
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    /// Branch being merged into — used for display and comparisons.
    pub branch: String,
    /// Path to the target worktree on disk, when one exists. `None` means the
    /// target is a branch ref with no sibling worktree.
    pub path: Option<PathBuf>,
}

/// Fires pre-merge / post-merge hooks.
///
/// Implemented by the command layer (wrapping [`HookExecutor`]) and by a
/// no-op [`NullHookRunner`] for unit tests that exercise `execute_start`
/// without the full hook infrastructure. The runner owns its target
/// worktree path so callers don't have to thread it through the trait.
///
/// Failure semantics are the runner's responsibility:
/// * `fire_pre_merge` returning `Err` aborts the merge (caller propagates).
/// * `fire_post_merge` returning `Err` is surfaced by the caller as a
///   warning; the merge result is never rolled back.
pub trait HookRunner {
    /// Fire the `pre-merge` hook. Returning `Err` aborts the merge.
    fn fire_pre_merge(&mut self, ctx: &MergeHookContext) -> Result<()>;
    /// Fire the `post-merge` hook. Returning `Err` does NOT roll back the
    /// merge; the command layer surfaces it as a warning.
    fn fire_post_merge(&mut self, ctx: &MergeHookContext) -> Result<()>;
}

/// No-op [`HookRunner`] for unit tests and callers that don't wire hooks.
pub struct NullHookRunner;

impl HookRunner for NullHookRunner {
    fn fire_pre_merge(&mut self, _: &MergeHookContext) -> Result<()> {
        Ok(())
    }
    fn fire_post_merge(&mut self, _: &MergeHookContext) -> Result<()> {
        Ok(())
    }
}

/// Env-var context carried to pre-merge / post-merge hooks.
///
/// Constructed once per merge (after target resolution and pre-flight) and
/// injected into the hook environment via
/// [`HookContext::with_extra_env`](crate::hooks::HookContext::with_extra_env)
/// alongside the universal `DAFT_*` vars. Kept as a flat `BTreeMap` rather
/// than a typed struct so adding new env vars is a one-line change and hook
/// scripts see them via the same mechanism as every other DAFT_* variable.
///
/// `BTreeMap` rather than `HashMap` so ordering is deterministic — tests and
/// debug logs read the same every run.
#[derive(Debug, Clone)]
pub struct MergeHookContext {
    pub env: BTreeMap<String, String>,
}

/// Outcome of a merge operation as carried to `post-merge`.
///
/// Captures just enough to populate `DAFT_MERGE_RESULT`, `DAFT_MERGE_COMMIT_SHA`,
/// `DAFT_MERGE_CONFLICTED_FILES`, and `DAFT_MERGE_PROMOTED_FROM_EPHEMERAL`.
/// `AlreadyUpToDate` is included for completeness even though the current
/// merge flow short-circuits on that check before firing pre-merge — a future
/// slice that moves hooks earlier will want this variant.
#[derive(Debug, Clone)]
pub enum PostOutcome {
    Success {
        commit_sha: String,
    },
    Conflict {
        files: Vec<String>,
        promoted_from_ephemeral: bool,
    },
    AlreadyUpToDate,
    /// The squash-commit step was aborted (editor empty, pre-commit hook refused,
    /// GPG-sign fail, etc.). Squash changes remain staged on the target.
    /// `DAFT_MERGE_COMMIT_SHA` is empty for this variant.
    Aborted,
}

impl MergeHookContext {
    /// Build the pre-merge env-var set from the resolved merge plan.
    ///
    /// `mode` derives from the flag combination (octopus wins over squash
    /// wins over ff-only; single-source merges without those flags are
    /// `"merge"`). `is_ephemeral` is set by the caller when the merge will
    /// be performed in an ephemeral worktree. `cross_worktree` is true when
    /// the target worktree is not the current worktree.
    ///
    /// `source_shas` carries the resolved SHAs for each source (same order as
    /// `sources`), exposed as `DAFT_MERGE_SOURCE_SHAS` (newline-separated).
    /// Pass an empty slice when SHAs were not captured (e.g. up-to-date
    /// short-circuit that fires no hooks).
    pub fn for_pre_with_shas(
        sources: &[String],
        target: &ResolvedTarget,
        flags: &EffectiveFlags,
        is_ephemeral: bool,
        cross_worktree: bool,
        source_shas: &[String],
    ) -> Self {
        let mode = if sources.len() >= 2 {
            "octopus"
        } else if flags.squash == Some(true) {
            "squash"
        } else if flags.ff == Some(FfMode::Only) {
            "ff"
        } else {
            "merge"
        };
        let mut env = BTreeMap::new();
        env.insert("DAFT_MERGE_SOURCES".into(), sources.join(" "));
        env.insert("DAFT_MERGE_SOURCE_SHAS".into(), source_shas.join("\n"));
        env.insert("DAFT_MERGE_TARGET_BRANCH".into(), target.branch.clone());
        env.insert(
            "DAFT_MERGE_TARGET_PATH".into(),
            target
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        );
        env.insert("DAFT_MERGE_MODE".into(), mode.into());
        env.insert(
            "DAFT_MERGE_STRATEGY".into(),
            flags.strategy.clone().unwrap_or_default(),
        );
        env.insert("DAFT_MERGE_EPHEMERAL".into(), is_ephemeral.to_string());
        env.insert(
            "DAFT_MERGE_CROSS_WORKTREE".into(),
            cross_worktree.to_string(),
        );
        Self { env }
    }

    /// Build the pre-merge env-var set without source SHAs.
    ///
    /// Delegates to [`for_pre_with_shas`] with an empty SHA slice. Callers
    /// that have already captured SHAs should use `for_pre_with_shas` directly
    /// so the `DAFT_MERGE_SOURCE_SHAS` env var is populated for hook scripts.
    pub fn for_pre(
        sources: &[String],
        target: &ResolvedTarget,
        flags: &EffectiveFlags,
        is_ephemeral: bool,
        cross_worktree: bool,
    ) -> Self {
        Self::for_pre_with_shas(sources, target, flags, is_ephemeral, cross_worktree, &[])
    }

    /// Extend a pre context with post-outcome fields, consuming `self`.
    ///
    /// Overrides any pre-existing values for the post-only keys; other keys
    /// (sources, target, mode, etc.) pass through untouched so hook scripts
    /// can correlate pre and post.
    pub fn extend_for_post(mut self, outcome: PostOutcome) -> Self {
        let (result, sha, files, promoted) = match outcome {
            PostOutcome::Success { commit_sha } => {
                ("success", commit_sha, String::new(), "false".to_string())
            }
            PostOutcome::Conflict {
                files,
                promoted_from_ephemeral,
            } => (
                "conflict",
                String::new(),
                files.join("\n"),
                promoted_from_ephemeral.to_string(),
            ),
            PostOutcome::AlreadyUpToDate => (
                "already-up-to-date",
                String::new(),
                String::new(),
                "false".to_string(),
            ),
            PostOutcome::Aborted => ("aborted", String::new(), String::new(), "false".to_string()),
        };
        self.env.insert("DAFT_MERGE_RESULT".into(), result.into());
        self.env.insert("DAFT_MERGE_COMMIT_SHA".into(), sha);
        self.env.insert("DAFT_MERGE_CONFLICTED_FILES".into(), files);
        self.env
            .insert("DAFT_MERGE_PROMOTED_FROM_EPHEMERAL".into(), promoted);
        self
    }
}

/// Resolve the merge target.
///
/// * `Some(t)` — try [`GitCommand::resolve_worktree_path`] first (which
///   matches `t` as a relative path, then a branch name, then a worktree
///   directory name). If that finds a worktree, return it with its current
///   branch via [`branch_at_path`]. If no worktree matches, fall back to
///   checking whether `t` exists as a branch ref (`refs/heads/<t>`): if so,
///   return `path: None, branch: t` (ref-only target). Otherwise error.
/// * `None` — the target is the current worktree: path from
///   [`GitCommand::get_current_worktree_path`], branch via the same
///   [`branch_at_path`] helper so both arms produce identical error
///   formatting for the same failure modes (detached HEAD, read failure).
///   `path` is always `Some` in this arm since the CWD is inside a worktree.
///
/// Fails loudly on detached HEAD (worktree case). Merging into a detached
/// HEAD would fail downstream anyway; the explicit error here surfaces the
/// problem earlier.
pub fn resolve_target(
    target: Option<&str>,
    git: &GitCommand,
    project_root: &Path,
) -> Result<ResolvedTarget> {
    match target {
        Some(t) => match git.resolve_worktree_path(t, project_root) {
            Ok(path) => {
                let branch = branch_at_path(git, &path)?;
                Ok(ResolvedTarget {
                    branch,
                    path: Some(path),
                })
            }
            Err(_) => {
                // No worktree matched. Fall back to local branch ref resolution.
                //
                // Only `refs/heads/<name>` is accepted. Remote-tracking refs like
                // `origin/main` and tags are deliberately excluded — `daft merge
                // --into` requires a movable local branch head. Merging into a
                // tag or remote-tracking ref is semantically nonsense: tags are
                // immutable and remote-tracking refs are updated only by fetch,
                // so advancing them here would either fail or silently desync
                // the local view from the remote.
                let ref_name = format!("refs/heads/{t}");
                if git.show_ref_exists(&ref_name)? {
                    Ok(ResolvedTarget {
                        branch: t.to_string(),
                        path: None,
                    })
                } else {
                    anyhow::bail!(
                        "no worktree or branch named '{}'; specify an existing target",
                        t
                    )
                }
            }
        },
        None => {
            let path = git.get_current_worktree_path()?;
            let branch = branch_at_path(git, &path)?;
            Ok(ResolvedTarget {
                branch,
                path: Some(path),
            })
        }
    }
}

/// Read the short branch name at `path`.
///
/// Respects [`GitCommand::use_gitoxide`]: when enabled, opens a
/// `gix::ThreadSafeRepository` at `path` and reads `HEAD` through gitoxide;
/// otherwise shells out `git -C <path> symbolic-ref --short HEAD`.
///
/// Both paths emit the same error message on detached HEAD ("detached HEAD")
/// so [`resolve_target`]'s two arms are indistinguishable from the user's
/// point of view for that failure mode.
fn branch_at_path(git: &GitCommand, path: &Path) -> Result<String> {
    if git.use_gitoxide {
        let ts = gix::ThreadSafeRepository::discover(path)
            .with_context(|| format!("failed to open git repo at '{}'", path.display()))?;
        let repo = ts.to_thread_local();
        let head = repo
            .head_ref()
            .with_context(|| format!("failed to read HEAD at '{}'", path.display()))?;
        return match head {
            Some(reference) => Ok(reference.name().shorten().to_string()),
            None => anyhow::bail!(
                "target worktree at '{}' has detached HEAD; checkout a branch first",
                path.display()
            ),
        };
    }

    let output = Command::new("git")
        .args([
            "-C",
            &path.display().to_string(),
            "symbolic-ref",
            "--short",
            "HEAD",
        ])
        .output()
        .with_context(|| format!("failed to read branch at '{}'", path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a symbolic ref") {
            anyhow::bail!(
                "target worktree at '{}' has detached HEAD; checkout a branch first",
                path.display()
            );
        }
        anyhow::bail!(
            "failed to read branch at '{}': {}",
            path.display(),
            stderr.trim()
        );
    }

    String::from_utf8(output.stdout)
        .context("invalid UTF-8 in branch name")
        .map(|s| s.trim().to_string())
}

/// An in-progress git operation detected on a worktree.
///
/// These correspond to the well-known state files git writes into the
/// worktree's `.git` directory when a merge/rebase/cherry-pick/bisect is
/// paused awaiting user input. We refuse to start a new merge against a
/// target in one of these states; stacking operations would bury the
/// user under two layers of conflicts.
#[derive(Debug, PartialEq, Eq)]
pub enum InProgressOp {
    Merge,
    /// `git merge --squash` succeeded but the commit step is still pending:
    /// `SQUASH_MSG` exists, `MERGE_HEAD` does NOT exist, and there are staged
    /// changes.  `--abort` runs `git reset --merge` + removes `SQUASH_MSG`;
    /// `--continue` re-opens the editor (or honors `-m`/`--no-edit`/`-F`).
    SquashStaged,
    Rebase,
    CherryPick,
    Bisect,
}

impl InProgressOp {
    /// Human-readable name, used in refusal messages (e.g. "mid-rebase").
    pub fn description(&self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::SquashStaged => "squash staged",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Bisect => "bisect",
        }
    }
}

/// Resolve the real `.git` directory for `worktree`.
///
/// In the main worktree `.git` is itself a directory. In a linked worktree,
/// `.git` is a file whose first line is `gitdir: <path>` pointing at the
/// per-worktree git dir (e.g. `.git/worktrees/<name>`). Returns an error if
/// the `.git` file is malformed or the resolved directory does not exist.
///
/// This is the canonical resolution used by both [`detect_in_progress`] and the
/// intent-marker read/write paths so the marker location is always consistent
/// with the detection path.
pub fn resolve_worktree_git_dir(worktree: &Path) -> Result<PathBuf> {
    let git_entry = worktree.join(".git");
    let git_dir = if git_entry.is_file() {
        let content = std::fs::read_to_string(&git_entry)
            .with_context(|| format!("failed to read .git at {}", git_entry.display()))?;
        let rel = content
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("gitdir: "))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "malformed .git file at {}: expected 'gitdir: <path>' on first line",
                    git_entry.display()
                )
            })?
            .trim();
        let p = PathBuf::from(rel);
        // Path::join replaces when its argument is absolute, so this is
        // correct whether the pointer is absolute or relative.
        if p.is_absolute() {
            p
        } else {
            worktree.join(p)
        }
    } else {
        git_entry
    };

    if !git_dir.is_dir() {
        anyhow::bail!(
            "target worktree at '{}' has no valid .git directory",
            worktree.display()
        );
    }
    Ok(git_dir)
}

/// Check whether the index of `worktree` has staged (cached) changes.
///
/// Runs `git diff --cached --quiet`; exit code 0 means no staged changes,
/// exit code 1 means there are staged changes. Used by [`detect_in_progress`]
/// to discriminate a real squash-staged state from a stale `SQUASH_MSG` left
/// behind from an earlier operation where the user never committed.
pub fn has_staged_changes(worktree: &Path) -> Result<bool> {
    let status = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(worktree)
        .status()
        .context("failed to invoke `git diff --cached --quiet`")?;
    // exit 0 → no staged changes; exit 1 → staged changes present.
    // Any other exit code is unexpected; treat as "no staged changes" (safe
    // default — false-negative is less dangerous than a false-positive).
    Ok(status.code() == Some(1))
}

/// Detect whether a worktree has an in-progress merge/rebase/cherry-pick/bisect.
///
/// Inspects the worktree's real git directory for the marker files git
/// writes when an operation is paused. In a linked worktree, `.git` is a
/// file with `gitdir: <path>` pointing at the actual per-worktree git dir
/// (e.g. `.git/worktrees/<name>`); in the main worktree, `.git` is itself
/// a directory. Both shapes are handled.
///
/// We intentionally check for directory/file *existence* rather than
/// parsing contents — git populates these atomically and their presence
/// alone is the signal git itself uses (see `git status` output).
pub fn detect_in_progress(worktree: &Path) -> Result<Option<InProgressOp>> {
    let git_dir = resolve_worktree_git_dir(worktree)?;

    if git_dir.join("MERGE_HEAD").exists() {
        return Ok(Some(InProgressOp::Merge));
    }

    // Squash-staged: `git merge --squash` writes SQUASH_MSG but NOT MERGE_HEAD.
    // Require staged changes to avoid false positives on a stale SQUASH_MSG
    // (e.g. a previous squash that was committed successfully but whose
    // SQUASH_MSG somehow remained).
    let squash_msg = git_dir.join("SQUASH_MSG");
    if squash_msg.exists() && has_staged_changes(worktree)? {
        return Ok(Some(InProgressOp::SquashStaged));
    }

    if git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir() {
        return Ok(Some(InProgressOp::Rebase));
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return Ok(Some(InProgressOp::CherryPick));
    }
    if git_dir.join("BISECT_LOG").exists() {
        return Ok(Some(InProgressOp::Bisect));
    }
    Ok(None)
}

/// Resolve each source ref to its full commit SHA via `git rev-parse`.
///
/// Called early in [`execute_start`], after pre-flight checks pass but before
/// the `pre-merge` hook fires. Fails fast with a clear error if any source
/// cannot be resolved — this is stricter than the UTD check (which silently
/// skips failed rev-parse), and that strictness is intentional: if a source
/// ref can't be resolved here, it won't resolve in `git merge` either and we
/// want the error to be legible before any state is touched.
pub fn capture_source_shas(sources: &[String], git: &GitCommand) -> Result<Vec<String>> {
    sources
        .iter()
        .map(|src| {
            git.rev_parse(src)
                .with_context(|| format!("failed to resolve source ref '{}' — does it exist?", src))
        })
        .collect()
}

/// Refuse a merge when the target worktree has uncommitted/untracked changes.
///
/// Delegates to [`GitCommand::has_uncommitted_changes_in`] (`src/git/stash.rs`)
/// so dirtiness is decided exactly the same way the rest of daft decides it
/// (via `git status --porcelain`, which treats untracked files as dirty).
///
/// The refusal message names the canonical remediation options.
///
/// Gated at the call site by `StartParams::require_clean_target`, which is
/// sourced from `daft.merge.requireCleanTarget` (default `true`). When that
/// key is `false`, the command layer skips this check entirely — git's own
/// `git merge` will then take over the dirty-tree decision (it stashes/merges
/// when non-conflicting, errors out when conflicting).
pub fn validate_clean_target(git: &GitCommand, path: &Path) -> Result<()> {
    if git.has_uncommitted_changes_in(path)? {
        anyhow::bail!(
            "target worktree '{}' has uncommitted changes; commit or stash them before merging",
            path.display()
        );
    }
    Ok(())
}

/// Refuse merging a branch into itself.
///
/// A `git merge X` run from a worktree whose branch is `X` is always a
/// semantic no-op at best and an ambiguous user mistake at worst. Failing
/// here — before we touch git — gives a clear, actionable error instead of
/// a cryptic "Already up to date." when the user likely meant to target a
/// different branch via `--into`.
///
/// Note: comparison is nominal, not OID-based. `origin/main` and commit SHAs
/// are not normalized. Stronger resolution may land in a later slice.
pub fn validate_distinct(sources: &[String], target: &ResolvedTarget) -> Result<()> {
    let target_normalized = strip_refs_heads(&target.branch);
    for src in sources {
        if strip_refs_heads(src) == target_normalized {
            anyhow::bail!("cannot merge branch '{}' into the same branch", src);
        }
    }
    Ok(())
}

fn strip_refs_heads(s: &str) -> &str {
    s.strip_prefix("refs/heads/").unwrap_or(s)
}

/// Returns the octopus announcement for `sources` merging into `target_branch`,
/// or `None` for a single source.
///
/// Format: `"Merging N sources into <target> via octopus strategy"`.
///
/// Pure function — no I/O. The caller decides how to surface the message
/// (stderr, logger, TUI). [`execute_start`] prints it to stderr before invoking
/// `git merge` so the announcement is visible even if git's octopus strategy
/// refuses with a conflict.
pub fn announcement(sources: &[String], target_branch: &str) -> Option<String> {
    if sources.len() >= 2 {
        Some(format!(
            "Merging {} sources into {} via octopus strategy",
            sources.len(),
            target_branch
        ))
    } else {
        None
    }
}

/// Result of a merge-start operation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartOutcome {
    /// The target branch was already up to date with the sources; nothing to do.
    pub already_up_to_date: bool,
    /// True if `git merge` exited non-zero for any reason (conflict, unknown ref, bad state),
    /// OR if the squash-commit step was aborted (editor empty, pre-commit hook fail, etc.).
    /// Slice 5+ will refine this into `conflicted` vs other failure modes via stderr parsing
    /// or `.git/MERGE_HEAD` inspection.
    pub failed: bool,
    /// Path of the target worktree, when one exists. Populated on success,
    /// conflict, and regular merge paths. The ref-only FF and ref-only
    /// up-to-date paths return an empty PathBuf because no worktree is
    /// involved — callers in those paths must check `emitted_terminal_message`
    /// or `already_up_to_date` before formatting the path.
    pub target_path: PathBuf,
    /// Files flagged with a conflict XY code by `git status --porcelain` in the
    /// target worktree after a failed `git merge`. Non-empty only when
    /// [`StartOutcome::failed`] is true and git actually left the worktree in a
    /// conflicted state; other failure modes (unknown ref, bad flags) leave
    /// this empty because `git status` reports no conflicts.
    pub conflicted_files: Vec<String>,
    /// True if core has already emitted a terminal status line (e.g.,
    /// "Already up to date." from the up-to-date short-circuit, or
    /// "Fast-forwarded X to Y (no worktree)" from the ref-only FF plumbing
    /// path). The command layer uses this to suppress its default
    /// "Merge complete." print so a single successful merge produces a single
    /// status line on stdout.
    pub emitted_terminal_message: bool,
    /// True if the conflict path promoted an ephemeral worktree to its
    /// layout-resolved sibling path. The command layer uses this to fire the
    /// `worktree-post-create` hook for the newly-registered worktree (see
    /// [`promote_ephemeral_to_layout`]). Always false outside the ephemeral
    /// conflict path.
    pub ephemeral_promoted: bool,
    /// Set when `--squash` was used with `--no-commit` (or `commit=false`):
    /// the squash changes are staged but no commit was made. The command
    /// layer uses this to print the "Squash staged on <target>. Commit when
    /// ready." line instead of the squash-committed line.
    pub squash_staged_only: bool,
    /// The SHA of the squash commit, when `--squash` succeeded and committed.
    /// `None` in all other cases: non-squash merges, `--no-commit`, or abort.
    pub squash_commit_sha: Option<String>,
    /// True if the squash-commit step was explicitly aborted (editor empty,
    /// pre-commit hook fail, GPG-sign fail, etc.). The command layer uses this
    /// to print the abort message and skip cleanup.
    ///
    /// When this is true, `failed` is also true and `squash_commit_sha` is
    /// `None`. Slice 6 will wire `post-merge` with `RESULT=aborted` when this
    /// is set; for now just the plumbing.
    pub commit_aborted: bool,
    /// The resolved target branch name, populated on every non-error path.
    /// Used by the command layer for state-aware terminal messages (e.g.
    /// "Squash merged <source> into <target> as <sha>"). Empty string on
    /// already-up-to-date and ref-only FF paths that emit their own line.
    pub target_branch: String,
    /// The resolved SHAs of each source ref, in order, captured before any
    /// merge work begins. Used by the cleanup stability check (Slice 4) to
    /// detect if any source ref moved during the editor session. Empty on
    /// already-up-to-date and other short-circuit paths that don't capture.
    pub source_shas: Vec<String>,

    /// Combined stdout+stderr captured from `git merge` /
    /// `git merge --squash` / `git commit` invocations during the merge
    /// phase. Empty on the no-merge-work paths (already-up-to-date, etc.).
    /// Suppressed by the command layer on success; dumped to stderr on
    /// failure (after the spinner stops) and on `--verbose` regardless.
    pub captured_git_output: Vec<u8>,

    /// True iff the regular (non-squash) merge fast-forwarded the target.
    /// `false` for non-FF merge commits, squash, conflict, AUTD, and any
    /// failure path. Used by the command layer to render
    /// `Fast-forwarded <target> to <sha>` instead of
    /// `Merged <source> into <target> (commit <sha>)`.
    pub was_fast_forward: bool,

    /// The SHA of the resulting commit on `target_branch` for non-squash
    /// merges. `Some` on both FF and merge-commit success paths. `None`
    /// for squash (use `squash_commit_sha`), AUTD, conflict, and failure.
    pub merge_commit_sha: Option<String>,
}

/// Parse `git status --porcelain` output for conflict entries.
///
/// Kept as a pure string function (no IO) so it can be unit-tested without a
/// real repository. The XY code in columns 1–2 encodes index/working-tree
/// state; any of `UU`, `AA`, `DD`, `AU`, `UA`, `DU`, `UD` marks an unmerged
/// entry ([Git docs][porcelain]). Everything else — `?? `, ` M `, ` A `, etc.
/// — is a normal index/worktree diff and is ignored.
///
/// [porcelain]: https://git-scm.com/docs/git-status#_porcelain_format_version_1
fn parse_conflicted_files_from_porcelain(porcelain: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in porcelain.lines() {
        let code: String = line.chars().take(2).collect();
        let is_conflict = matches!(
            code.as_str(),
            "UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD"
        );
        if is_conflict && line.len() > 3 {
            files.push(line[3..].to_string());
        }
    }
    files
}

/// Return the list of files with merge conflicts in the given worktree.
///
/// Reads `git status --porcelain` and filters via
/// [`parse_conflicted_files_from_porcelain`]. Returns an empty list — not an
/// error — if status itself fails (a broken worktree shouldn't surface a
/// second error on top of the merge conflict the caller is already reporting).
pub fn conflicted_files(worktree: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args([
            "-C",
            &worktree.display().to_string(),
            "status",
            "--porcelain",
        ])
        .output()
        .with_context(|| format!("failed to read status in '{}'", worktree.display()))?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let porcelain = String::from_utf8_lossy(&output.stdout);
    Ok(parse_conflicted_files_from_porcelain(&porcelain))
}

/// Execute a merge against the resolved target.
///
/// Resolves the target (explicit `--into` value or current worktree) through
/// [`resolve_target`], then dispatches `git merge <sources...>` with the
/// target worktree as CWD so git updates the correct worktree's index and
/// working tree.
///
/// Returns [`StartOutcome`] describing the result. In this Slice-3 form we
/// still detect failure solely via git's exit status; `already_up_to_date` is
/// always reported as `false` here and will be upgraded in later slices.
///
/// # Signature stability
///
/// Taking `git: &GitCommand` and `project_root: &Path` lets later slices add
/// flag passthrough (Slice 6, via `StartParams`) and ref-only targets
/// (Slice 9, via changes to [`ResolvedTarget`]) without another signature
/// churn.
pub fn execute_start(
    params: &StartParams,
    git: &GitCommand,
    project_root: &Path,
    hooks: &mut dyn HookRunner,
) -> Result<StartOutcome> {
    let resolved = resolve_target(params.target.as_deref(), git, project_root)?;

    // Pre-flight safety rails. Cheapest purely-syntactic check first (source
    // vs. target branch name, which needs no path).
    validate_distinct(&params.sources, &resolved)?;

    // Filesystem-state checks only apply when a worktree exists. For a
    // ref-only target there is no MERGE_HEAD to find and no working tree to
    // be dirty, so these rails are skipped — Slice 9's FF-plumbing path
    // operates on the ref alone.
    if let Some(ref path) = resolved.path {
        if let Some(op) = detect_in_progress(path)? {
            anyhow::bail!(
                "target worktree '{}' is mid-{}; finish or abort it first",
                resolved.branch,
                op.description()
            );
        }
        // `daft.merge.requireCleanTarget` (default true) gates the dirty
        // check. Opt-out matches `git merge`'s own behavior, which happily
        // stashes/merges with uncommitted changes if they don't conflict.
        if params.require_clean_target {
            validate_clean_target(git, path)?;
        }
    }

    // Detect "already up to date" via plumbing before invoking `git merge`, so
    // we don't need to capture stdout. Capturing stdout would break the
    // interactive editor path for non-FF merges without `-m`: git launches
    // `$EDITOR` for the commit message, which requires stdio to be inherited
    // from the parent TTY. Running `merge-base --is-ancestor` per source is
    // equivalent to git's own "Already up to date." outcome: if every source
    // is already reachable from the target tip, there is nothing to merge.
    //
    // Checked before the octopus announcement so an up-to-date multi-source
    // invocation doesn't herald a strategy we won't actually run. The check
    // operates on refs only (rev-parse + merge-base), so it works uniformly
    // for both worktree-backed and ref-only targets.
    let target_sha = git
        .rev_parse(&format!("refs/heads/{}", resolved.branch))
        .with_context(|| format!("failed to resolve target branch '{}'", resolved.branch))?;
    let all_sources_are_ancestors = params.sources.iter().all(|src| {
        // If rev-parse fails (invalid ref), treat as "not an ancestor" and let
        // `git merge` surface the real error with its usual formatting.
        let src_sha = match git.rev_parse(src) {
            Ok(sha) => sha,
            Err(_) => return false,
        };
        git.merge_base_is_ancestor(&src_sha, &target_sha)
            .unwrap_or(false)
    });

    if all_sources_are_ancestors {
        // All sources already reachable from the target — equivalent to git's
        // "Already up to date." outcome. Short-circuit without invoking
        // `git merge`, preserving the interactive editor path for real merges.
        //
        // Hooks deliberately do NOT fire on this path. `pre-merge` fires
        // "before any merge operation"; since none runs, neither does the
        // pre hook. `post-merge` stays paired with it — an up-to-date merge
        // is not an observable event that hook scripts need to react to.
        println!("Already up to date.");
        return Ok(StartOutcome {
            already_up_to_date: true,
            failed: false,
            target_path: resolved.path.clone().unwrap_or_default(),
            conflicted_files: Vec::new(),
            emitted_terminal_message: true,
            ephemeral_promoted: false,
            ..StartOutcome::default()
        });
    }

    // Capture source SHAs before any merge work begins (and before the
    // pre-merge hook fires, so DAFT_MERGE_SOURCE_SHAS is available to hook
    // scripts). Fail fast with a clear error if any source can't be resolved.
    // This is intentionally stricter than the UTD check above (which silently
    // treats rev-parse failures as "not an ancestor") — by this point we know
    // at least one source is not already merged, so a resolution failure here
    // is a real problem the user needs to see.
    let source_shas = capture_source_shas(&params.sources, git)?;

    // Announce octopus before invoking git so users see the strategy name even
    // if git's octopus refuses with a conflict. Single-source merges emit
    // nothing — `git merge <source>` is the plain case and needs no herald.
    // Stderr keeps progress output out of stdout (reserved for the final
    // "Merge complete." / "Already up to date." result line).
    if let Some(msg) = announcement(&params.sources, &resolved.branch) {
        eprintln!("{msg}");
    }

    // Cross-worktree detection: target worktree is not the current worktree.
    // Computed once here, surfaced to pre-merge / post-merge via
    // DAFT_MERGE_CROSS_WORKTREE. If the CWD lookup fails (e.g. detached HEAD
    // during test harness setup) default to false — the observable wrongness
    // is cosmetic and the merge itself is unaffected.
    let cross_worktree = match (git.get_current_worktree_path().ok(), &resolved.path) {
        (Some(cwd), Some(target_path)) => cwd != *target_path,
        _ => false,
    };

    match resolved.path.clone() {
        Some(path) => {
            execute_start_in_worktree(params, &resolved, path, cross_worktree, &source_shas, hooks)
        }
        None => execute_start_ref_only(
            params,
            git,
            project_root,
            &resolved,
            &source_shas,
            cross_worktree,
            hooks,
        ),
    }
}

/// Worktree-backed merge: shell out to `git merge` with the target worktree as CWD.
///
/// Stdio is inherited so `$EDITOR` works for non-FF merges without `-m`. On
/// failure, probe the worktree's `git status --porcelain` for conflicted files
/// so the caller can render a conflict report.
///
/// For `--squash` merges, after a successful `git merge --squash`, runs a
/// `git commit` step (unless `--no-commit` / `commit=false` is set). The
/// commit step forwards message-composing flags via [`render_commit_flags`].
/// If the commit step fails (editor aborted, pre-commit hook refused, GPG-sign
/// error), the outcome has `failed=true` and `commit_aborted=true` so the
/// command layer can print the abort message and skip cleanup.
///
/// Fires `pre-merge` before `git merge` (failure aborts) and `post-merge`
/// after the final outcome is determined (failure is only surfaced as a
/// warning to the caller).
fn execute_start_in_worktree(
    params: &StartParams,
    resolved: &ResolvedTarget,
    path: PathBuf,
    cross_worktree: bool,
    source_shas: &[String],
    hooks: &mut dyn HookRunner,
) -> Result<StartOutcome> {
    // Build the pre-merge env-var context. This is the worktree-backed
    // path, so `is_ephemeral = false`.
    let pre_ctx = MergeHookContext::for_pre_with_shas(
        &params.sources,
        resolved,
        &params.flags,
        false,
        cross_worktree,
        source_shas,
    );
    // `fire_pre_merge` failure aborts the merge before any state is touched.
    hooks.fire_pre_merge(&pre_ctx)?;

    let mut argv: Vec<String> = vec!["merge".to_string()];
    argv.extend(render_flags(&params.flags));
    argv.extend(params.sources.iter().cloned());

    let merge_result = Command::new("git")
        .args(&argv)
        .current_dir(&path)
        .output()
        .with_context(|| format!("failed to invoke `git merge` in '{}'", path.display()))?;
    let mut captured: Vec<u8> = Vec::new();
    captured.extend_from_slice(&merge_result.stdout);
    captured.extend_from_slice(&merge_result.stderr);

    let failed = !merge_result.status.success();
    // Only probe `git status` on failure — the success path already left the
    // worktree clean, and conflict state is only meaningful post-failure.
    // `conflicted_files` swallows IO errors into an empty list so a broken
    // status probe never masks the real merge error with a secondary one.
    let files = if failed {
        conflicted_files(&path).unwrap_or_default()
    } else {
        Vec::new()
    };

    if failed {
        let post_ctx = pre_ctx.extend_for_post(PostOutcome::Conflict {
            files: files.clone(),
            promoted_from_ephemeral: false,
        });
        if let Err(e) = hooks.fire_post_merge(&post_ctx) {
            eprintln!("warning: post-merge hook failed: {e}");
        }
        return Ok(StartOutcome {
            already_up_to_date: false,
            failed: true,
            target_path: path,
            conflicted_files: files,
            emitted_terminal_message: false,
            ephemeral_promoted: false,
            source_shas: source_shas.to_vec(),
            captured_git_output: captured,
            ..StartOutcome::default()
        });
    }

    // `git merge --squash` succeeded. Determine whether to commit or stage-only.
    let is_squash = params.flags.squash == Some(true);
    let no_commit = params.flags.commit == Some(false);

    if is_squash && !no_commit {
        // Squash-then-commit: run `git commit` with message-composing flags
        // forwarded via render_commit_flags. Stdio is inherited so the editor
        // opens correctly (TTY guard earlier ensured stdin is a terminal when
        // no message-supplying flag is set).

        // Write the cleanup intent marker BEFORE git commit so that if the
        // editor is aborted, `--continue` can resume cleanup after re-commit.
        // The marker is removed on successful commit or on `--abort`.
        let intent_path_opt = if let Some(ref tmpl) = params.cleanup_intent {
            match resolve_worktree_git_dir(&path) {
                Ok(gd) => {
                    let intent = MergeIntent {
                        sources: params.sources.clone(),
                        source_shas: source_shas.to_vec(),
                        remove_worktree: tmpl.remove_worktree,
                        also_branch: tmpl.also_branch,
                    };
                    let ip = gd.join("daft-merge-intent.json");
                    write_intent_marker(&gd, &intent);
                    Some(ip)
                }
                Err(e) => {
                    eprintln!("warning: failed to resolve git dir for intent marker: {e}");
                    None
                }
            }
        } else {
            None
        };

        let mut commit_argv: Vec<String> = vec!["commit".to_string()];
        commit_argv.extend(render_commit_flags(&params.flags));

        // Editor-opening commit: inherit stdio so $EDITOR gets the terminal.
        // Slice 5 will bracket this with pause_spinner / resume_spinner.
        // Non-editor commit (--no-edit, -m, or -F supplied): capture output.
        let (commit_status, commit_captured) = if params.flags.squash_would_open_editor() {
            let status = Command::new("git")
                .args(&commit_argv)
                .current_dir(&path)
                .status()
                .with_context(|| {
                    format!("failed to invoke `git commit` in '{}'", path.display())
                })?;
            (status, Vec::new())
        } else {
            let result = Command::new("git")
                .args(&commit_argv)
                .current_dir(&path)
                .output()
                .with_context(|| {
                    format!("failed to invoke `git commit` in '{}'", path.display())
                })?;
            let mut buf = Vec::new();
            buf.extend_from_slice(&result.stdout);
            buf.extend_from_slice(&result.stderr);
            (result.status, buf)
        };
        captured.extend_from_slice(&commit_captured);

        if !commit_status.success() {
            // Commit aborted (editor empty, pre-commit hook refused, GPG fail,
            // etc.). Squash changes remain staged; cleanup must be skipped.
            // Intent marker remains for --continue to pick up.
            let post_ctx = pre_ctx.extend_for_post(PostOutcome::Aborted);
            if let Err(e) = hooks.fire_post_merge(&post_ctx) {
                eprintln!("warning: post-merge hook failed: {e}");
            }
            return Ok(StartOutcome {
                already_up_to_date: false,
                failed: true,
                target_path: path,
                conflicted_files: Vec::new(),
                emitted_terminal_message: false,
                ephemeral_promoted: false,
                squash_staged_only: false,
                squash_commit_sha: None,
                commit_aborted: true,
                target_branch: resolved.branch.clone(),
                source_shas: source_shas.to_vec(),
                captured_git_output: captured,
                ..StartOutcome::default()
            });
        }

        // Squash committed successfully. Remove the intent marker (if written).
        if let Some(ref ip) = intent_path_opt {
            let _ = std::fs::remove_file(ip);
        }

        // Squash committed. Read the new HEAD SHA for the status line and hook env.
        let sha = read_head_sha(&path).unwrap_or_default();
        let post_ctx = pre_ctx.extend_for_post(PostOutcome::Success {
            commit_sha: sha.clone(),
        });
        if let Err(e) = hooks.fire_post_merge(&post_ctx) {
            eprintln!("warning: post-merge hook failed: {e}");
        }
        return Ok(StartOutcome {
            already_up_to_date: false,
            failed: false,
            target_path: path,
            conflicted_files: Vec::new(),
            emitted_terminal_message: false,
            ephemeral_promoted: false,
            squash_staged_only: false,
            squash_commit_sha: Some(sha),
            commit_aborted: false,
            target_branch: resolved.branch.clone(),
            source_shas: source_shas.to_vec(),
            captured_git_output: captured,
            ..StartOutcome::default()
        });
    }

    if is_squash && no_commit {
        // Stage-only path: --squash --no-commit. Changes staged, no commit made.
        // Post-merge fires as Success with empty SHA (consistent with how
        // a staged-but-uncommitted squash is observable from hook scripts).
        let post_ctx = pre_ctx.extend_for_post(PostOutcome::Success {
            commit_sha: String::new(),
        });
        if let Err(e) = hooks.fire_post_merge(&post_ctx) {
            eprintln!("warning: post-merge hook failed: {e}");
        }
        return Ok(StartOutcome {
            already_up_to_date: false,
            failed: false,
            target_path: path,
            conflicted_files: Vec::new(),
            emitted_terminal_message: false,
            ephemeral_promoted: false,
            squash_staged_only: true,
            squash_commit_sha: None,
            commit_aborted: false,
            target_branch: resolved.branch.clone(),
            source_shas: source_shas.to_vec(),
            captured_git_output: captured,
            ..StartOutcome::default()
        });
    }

    // Regular (non-squash) merge succeeded. Read the target worktree's HEAD to
    // report the new commit SHA; if that read fails, fall back to an empty SHA
    // (hook scripts can test for `""`).
    let head_sha = read_head_sha(&path).unwrap_or_default();
    let post_ctx = pre_ctx.extend_for_post(PostOutcome::Success {
        commit_sha: head_sha.clone(),
    });
    // post-merge never rolls back the merge — surface errors as warnings at
    // the caller, not Err here.
    if let Err(e) = hooks.fire_post_merge(&post_ctx) {
        eprintln!("warning: post-merge hook failed: {e}");
    }

    // Detect fast-forward by parent count: FF has 1 parent, merge commit has 2+.
    // Best-effort: failure falls back to was_fast_forward=false (safe default).
    let was_ff = count_head_parents(&path).map(|n| n == 1).unwrap_or(false);
    let merge_sha = if head_sha.is_empty() {
        None
    } else {
        Some(head_sha)
    };

    Ok(StartOutcome {
        already_up_to_date: false,
        failed: false,
        target_path: path,
        conflicted_files: Vec::new(),
        emitted_terminal_message: false,
        ephemeral_promoted: false,
        target_branch: resolved.branch.clone(),
        source_shas: source_shas.to_vec(),
        captured_git_output: captured,
        was_fast_forward: was_ff,
        merge_commit_sha: merge_sha,
        ..StartOutcome::default()
    })
}

/// Read a worktree's HEAD SHA via `git rev-parse HEAD`.
///
/// Used to populate `DAFT_MERGE_COMMIT_SHA` in the `post-merge` env after a
/// successful merge. Best-effort: a failed read returns `None` and the
/// caller stamps an empty string so hook scripts can still pattern-match on
/// `DAFT_MERGE_RESULT=success` without relying on the SHA.
fn read_head_sha(worktree: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", &worktree.display().to_string(), "rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Count the number of parents of the current HEAD in the given worktree.
///
/// Used to distinguish fast-forward (1 parent) from merge commits (2+ parents).
/// Best-effort: returns `None` on any error so the caller can safely fall back
/// to `was_fast_forward = false`.
fn count_head_parents(worktree: &Path) -> Option<usize> {
    let output = Command::new("git")
        .args([
            "-C",
            &worktree.display().to_string(),
            "rev-list",
            "--parents",
            "-n",
            "1",
            "HEAD",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // Output is: "<sha> [<parent-sha>...]" — first token is HEAD, rest are parents.
    let s = String::from_utf8_lossy(&output.stdout);
    let tokens: Vec<&str> = s.split_whitespace().collect();
    // tokens[0] is HEAD SHA; tokens[1..] are parent SHAs.
    Some(tokens.len().saturating_sub(1))
}

/// Ref-only merge: the target branch has no worktree.
///
/// Two outcomes are supported:
///
/// * Pure fast-forward — advance the branch ref via `git update-ref`
///   without touching a working tree.
/// * Non-FF — consult the adopt-target decision. On `Yes`, materialize an
///   ephemeral worktree, perform the merge there, and remove it on success
///   (Slice 10). On `No`, bail with guidance. `Ask` means prompt the user
///   interactively via [`ask_adopt_user`].
///
/// Octopus merges (>= 2 sources) into ref-only targets are rejected upfront:
/// octopus always creates a merge commit, which requires a working tree.
fn execute_start_ref_only(
    params: &StartParams,
    git: &GitCommand,
    project_root: &Path,
    resolved: &ResolvedTarget,
    source_shas: &[String],
    cross_worktree: bool,
    hooks: &mut dyn HookRunner,
) -> Result<StartOutcome> {
    let target_branch = resolved.branch.as_str();
    if params.sources.len() != 1 {
        anyhow::bail!(
            "target branch '{}' has no worktree; octopus merges into ref-only targets are not supported. \
             Run `daft checkout {}` first.",
            target_branch,
            target_branch,
        );
    }
    let source = &params.sources[0];
    // Re-resolve the target SHA here rather than receiving it as an argument
    // (avoids pushing the function over clippy's 7-arg limit). The UTD check
    // in `execute_start` already resolved this; a second resolve is cheap and
    // avoids holding stale data through the adopt-target prompt.
    let target_sha = git
        .rev_parse(&format!("refs/heads/{}", target_branch))
        .with_context(|| format!("failed to resolve target branch '{}'", target_branch))?;
    // Reuse the already-captured SHA from `source_shas[0]` rather than
    // re-resolving — avoids drift if the ref moves between calls, and keeps
    // capture strictly "before any merge work" as the spec requires.
    let source_sha = source_shas.first().cloned().unwrap_or_else(|| {
        // Fallback: should never happen because execute_start captured SHAs
        // before dispatching here, but guard defensively.
        git.rev_parse(source).unwrap_or_else(|_| String::new())
    });
    // `merge_base_is_ancestor(target, source)` returns true when `target` is
    // an ancestor of `source` — i.e., the source has only new commits on top
    // of the target tip, so a pure fast-forward is possible.
    let is_ancestor = git.merge_base_is_ancestor(&target_sha, &source_sha)?;

    if is_pure_ff_eligible(params.sources.len(), &params.flags, Some(is_ancestor)) {
        // Pure FF via plumbing — no worktree involvement, no conflicts.
        // Fire pre-merge before the ref moves; post-merge after. The "path"
        // in the env stays empty so scripts can detect the ref-only FF case
        // via `[ -z "$DAFT_MERGE_TARGET_PATH" ]`.
        let pre_ctx = MergeHookContext::for_pre_with_shas(
            &params.sources,
            resolved,
            &params.flags,
            false,
            cross_worktree,
            source_shas,
        );
        hooks.fire_pre_merge(&pre_ctx)?;

        advance_ref_via_plumbing(git, target_branch, &source_sha)?;
        let short = &source_sha[..12.min(source_sha.len())];
        println!("Fast-forwarded {target_branch} to {short} (no worktree)");

        let post_ctx = pre_ctx.extend_for_post(PostOutcome::Success {
            commit_sha: source_sha.clone(),
        });
        if let Err(e) = hooks.fire_post_merge(&post_ctx) {
            eprintln!("warning: post-merge hook failed: {e}");
        }

        return Ok(StartOutcome {
            already_up_to_date: false,
            failed: false,
            target_path: PathBuf::new(),
            conflicted_files: Vec::new(),
            emitted_terminal_message: true,
            ephemeral_promoted: false,
            source_shas: source_shas.to_vec(),
            target_branch: resolved.branch.clone(),
            ..StartOutcome::default()
        });
    }

    // Non-FF path: decide whether to materialize an ephemeral worktree.
    //
    // `-y` coerces to `--adopt-target` when neither adopt flag is explicit
    // (announced on stderr for traceability). The preset comes from
    // `daft.merge.adoptTargetOnDemand` via `AdoptChoice::preset`; defaults to
    // `Prompt`, which means ask in TTY / refuse in non-TTY.
    let (flag_yes, flag_no) = resolve_adopt_flags(&params.adopt);
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let decision = decide_adopt(flag_yes, flag_no, is_tty, params.adopt.preset);

    let should_adopt = match decision {
        AdoptDecision::Yes => true,
        AdoptDecision::No => false,
        AdoptDecision::Ask => ask_adopt_user(target_branch)?,
    };

    if !should_adopt {
        anyhow::bail!(
            "target branch '{}' has no worktree and merge cannot fast-forward (histories diverge, \
             or --no-ff/--squash requested); run `daft checkout {}` first to materialize a worktree, \
             or re-run with --adopt-target (or -y) to create an ephemeral worktree",
            target_branch,
            target_branch,
        );
    }

    execute_ephemeral_merge(
        params,
        git,
        project_root,
        resolved,
        source_shas,
        cross_worktree,
        hooks,
    )
}

/// Materialize an ephemeral worktree for a ref-only target, run the merge
/// there, and — on conflict — promote it to the layout-resolved sibling path.
///
/// Behavior:
/// * Success — remove the temp worktree (the ref has already advanced via
///   the merge commit in the worktree).
/// * Conflict — promote the temp worktree to its layout-resolved sibling path
///   via [`promote_ephemeral_to_layout`], set `ephemeral_promoted: true`, and
///   return the layout path in `target_path`. The command layer fires
///   `worktree-post-create` so hook-installed environment setup (direnv,
///   mise, etc.) is available while the user resolves the conflict.
///
/// On promotion failure the ephemeral worktree is LEFT IN PLACE for manual
/// recovery and the error is surfaced with both paths mentioned.
///
/// Pure failure modes other than conflict (e.g. malformed flags, missing
/// refs) propagate through git's non-zero exit and are surfaced by the
/// caller's existing conflict-report path. `conflicted_files` swallows
/// status errors into an empty list so a broken worktree never masks the
/// real merge error with a secondary one.
fn execute_ephemeral_merge(
    params: &StartParams,
    git: &GitCommand,
    project_root: &Path,
    resolved: &ResolvedTarget,
    source_shas: &[String],
    cross_worktree: bool,
    hooks: &mut dyn HookRunner,
) -> Result<StartOutcome> {
    let target_branch = resolved.branch.as_str();
    let bare_root = crate::get_git_common_dir()
        .context("failed to locate bare git directory for ephemeral worktree")?;
    let temp_path = crate::core::worktree::temp_worktree::create(&bare_root, target_branch)
        .with_context(|| {
            format!(
                "failed to create ephemeral worktree for '{}'",
                target_branch
            )
        })?;

    // Fire pre-merge after the ephemeral worktree exists (so
    // DAFT_MERGE_TARGET_PATH / the hook's cwd can point at it) but before
    // the merge runs. `is_ephemeral = true` so scripts can branch on it.
    //
    // `resolved.path` is None on this path (ref-only target), so we swap in
    // the ephemeral temp_path for env-var purposes — hook scripts operating
    // on DAFT_MERGE_TARGET_PATH get the path that actually contains the
    // in-progress merge.
    let ephemeral_resolved = ResolvedTarget {
        branch: resolved.branch.clone(),
        path: Some(temp_path.clone()),
    };
    let pre_ctx = MergeHookContext::for_pre_with_shas(
        &params.sources,
        &ephemeral_resolved,
        &params.flags,
        true,
        cross_worktree,
        source_shas,
    );
    if let Err(e) = hooks.fire_pre_merge(&pre_ctx) {
        // pre-merge aborted the merge. Clean up the ephemeral worktree we
        // just created so a failed hook doesn't leave state behind.
        // Best-effort: surface the hook error regardless of cleanup result.
        let _ = crate::core::worktree::temp_worktree::remove(&temp_path);
        return Err(e);
    }

    let mut argv: Vec<String> = vec!["merge".to_string()];
    argv.extend(render_flags(&params.flags));
    argv.extend(params.sources.iter().cloned());

    let merge_result = Command::new("git")
        .args(&argv)
        .current_dir(&temp_path)
        .output()
        .with_context(|| {
            format!(
                "failed to invoke git merge in ephemeral worktree at '{}'",
                temp_path.display()
            )
        })?;
    let mut captured: Vec<u8> = Vec::new();
    captured.extend_from_slice(&merge_result.stdout);
    captured.extend_from_slice(&merge_result.stderr);

    if merge_result.status.success() {
        // `git merge --squash` succeeded: run commit step if needed.
        let is_squash = params.flags.squash == Some(true);
        let no_commit = params.flags.commit == Some(false);

        if is_squash && !no_commit {
            // Squash-then-commit in the ephemeral worktree.
            let mut commit_argv: Vec<String> = vec!["commit".to_string()];
            commit_argv.extend(render_commit_flags(&params.flags));

            // Editor-opening commit: inherit stdio so $EDITOR gets the terminal.
            // Slice 5 will bracket this with pause_spinner / resume_spinner.
            // Non-editor commit (--no-edit, -m, or -F supplied): capture output.
            let (commit_status, commit_captured) = if params.flags.squash_would_open_editor() {
                let status = Command::new("git")
                    .args(&commit_argv)
                    .current_dir(&temp_path)
                    .status()
                    .with_context(|| {
                        format!(
                            "failed to invoke `git commit` in ephemeral worktree at '{}'",
                            temp_path.display()
                        )
                    })?;
                (status, Vec::new())
            } else {
                let result = Command::new("git")
                    .args(&commit_argv)
                    .current_dir(&temp_path)
                    .output()
                    .with_context(|| {
                        format!(
                            "failed to invoke `git commit` in ephemeral worktree at '{}'",
                            temp_path.display()
                        )
                    })?;
                let mut buf = Vec::new();
                buf.extend_from_slice(&result.stdout);
                buf.extend_from_slice(&result.stderr);
                (result.status, buf)
            };
            captured.extend_from_slice(&commit_captured);

            if !commit_status.success() {
                // Commit aborted. Leave ephemeral worktree; caller will report.
                let post_ctx = pre_ctx.extend_for_post(PostOutcome::Aborted);
                if let Err(e) = hooks.fire_post_merge(&post_ctx) {
                    eprintln!("warning: post-merge hook failed: {e}");
                }
                return Ok(StartOutcome {
                    already_up_to_date: false,
                    failed: true,
                    target_path: temp_path,
                    conflicted_files: Vec::new(),
                    emitted_terminal_message: false,
                    ephemeral_promoted: false,
                    squash_staged_only: false,
                    squash_commit_sha: None,
                    commit_aborted: true,
                    target_branch: resolved.branch.clone(),
                    source_shas: source_shas.to_vec(),
                    captured_git_output: captured,
                    ..StartOutcome::default()
                });
            }

            // Squash committed in ephemeral. Read SHA, fire post-merge, tear down.
            let sha = read_head_sha(&temp_path).unwrap_or_default();
            let post_ctx = pre_ctx.extend_for_post(PostOutcome::Success {
                commit_sha: sha.clone(),
            });
            if let Err(e) = hooks.fire_post_merge(&post_ctx) {
                eprintln!("warning: post-merge hook failed: {e}");
            }

            crate::core::worktree::temp_worktree::remove(&temp_path).with_context(|| {
                format!(
                    "failed to remove ephemeral worktree at '{}'",
                    temp_path.display()
                )
            })?;
            return Ok(StartOutcome {
                already_up_to_date: false,
                failed: false,
                target_path: PathBuf::new(),
                conflicted_files: Vec::new(),
                emitted_terminal_message: false,
                ephemeral_promoted: false,
                squash_staged_only: false,
                squash_commit_sha: Some(sha),
                commit_aborted: false,
                target_branch: resolved.branch.clone(),
                source_shas: source_shas.to_vec(),
                captured_git_output: captured,
                ..StartOutcome::default()
            });
        }

        if is_squash && no_commit {
            // Stage-only path in ephemeral worktree (--squash --no-commit).
            let post_ctx = pre_ctx.extend_for_post(PostOutcome::Success {
                commit_sha: String::new(),
            });
            if let Err(e) = hooks.fire_post_merge(&post_ctx) {
                eprintln!("warning: post-merge hook failed: {e}");
            }
            crate::core::worktree::temp_worktree::remove(&temp_path).with_context(|| {
                format!(
                    "failed to remove ephemeral worktree at '{}'",
                    temp_path.display()
                )
            })?;
            return Ok(StartOutcome {
                already_up_to_date: false,
                failed: false,
                target_path: PathBuf::new(),
                conflicted_files: Vec::new(),
                emitted_terminal_message: false,
                ephemeral_promoted: false,
                squash_staged_only: true,
                squash_commit_sha: None,
                commit_aborted: false,
                target_branch: resolved.branch.clone(),
                source_shas: source_shas.to_vec(),
                captured_git_output: captured,
                ..StartOutcome::default()
            });
        }

        // Regular (non-squash) ephemeral merge succeeded.
        // Fire post-merge BEFORE tearing down the ephemeral worktree so
        // scripts still see DAFT_MERGE_TARGET_PATH pointing at a live dir.
        let head_sha = read_head_sha(&temp_path).unwrap_or_default();
        let post_ctx = pre_ctx.extend_for_post(PostOutcome::Success {
            commit_sha: head_sha.clone(),
        });
        if let Err(e) = hooks.fire_post_merge(&post_ctx) {
            eprintln!("warning: post-merge hook failed: {e}");
        }

        // Detect fast-forward by parent count.
        let was_ff = count_head_parents(&temp_path)
            .map(|n| n == 1)
            .unwrap_or(false);
        let merge_sha = if head_sha.is_empty() {
            None
        } else {
            Some(head_sha)
        };

        // Ref advanced inside the temp worktree via the merge commit; the
        // worktree itself is no longer needed. `temp_worktree::remove` does
        // best-effort cleanup (falls back to rm -rf + prune) so a stale
        // pointer from a half-removed worktree doesn't block future merges.
        crate::core::worktree::temp_worktree::remove(&temp_path).with_context(|| {
            format!(
                "failed to remove ephemeral worktree at '{}'",
                temp_path.display()
            )
        })?;
        return Ok(StartOutcome {
            already_up_to_date: false,
            failed: false,
            target_path: PathBuf::new(),
            conflicted_files: Vec::new(),
            emitted_terminal_message: false,
            ephemeral_promoted: false,
            source_shas: source_shas.to_vec(),
            captured_git_output: captured,
            was_fast_forward: was_ff,
            merge_commit_sha: merge_sha,
            ..StartOutcome::default()
        });
    }

    // Conflict path: promote the ephemeral worktree to its layout-resolved
    // sibling path so subsequent `daft list` / `git worktree list` surface it
    // and the user can resolve the conflict in a canonical location.
    //
    // `git worktree move` (wrapped by `GitCommand::worktree_move`) rewrites
    // git's internal gitdir pointer and the worktree's `.git` link file
    // atomically, so no bookkeeping is needed beyond the single call.
    let layout_path =
        match promote_ephemeral_to_layout(&temp_path, target_branch, git, project_root) {
            Ok(p) => p,
            Err(e) => {
                return Err(e.context(format!(
                    "ephemeral merge conflicted but promotion failed; \
                 ephemeral worktree remains at '{}' for manual recovery",
                    temp_path.display()
                )));
            }
        };

    // Probe conflicts at the PROMOTED path — the temp path no longer exists.
    let files = conflicted_files(&layout_path).unwrap_or_default();

    // Fire post-merge after promotion so DAFT_MERGE_TARGET_PATH points at
    // the canonical sibling location, and `promoted_from_ephemeral=true`
    // lets scripts react to the promotion (e.g., notify the user).
    let post_ctx = pre_ctx.extend_for_post(PostOutcome::Conflict {
        files: files.clone(),
        promoted_from_ephemeral: true,
    });
    if let Err(e) = hooks.fire_post_merge(&post_ctx) {
        eprintln!("warning: post-merge hook failed: {e}");
    }

    Ok(StartOutcome {
        already_up_to_date: false,
        failed: true,
        target_path: layout_path,
        conflicted_files: files,
        emitted_terminal_message: false,
        ephemeral_promoted: true,
        source_shas: source_shas.to_vec(),
        captured_git_output: captured,
        ..StartOutcome::default()
    })
}

/// Move an ephemeral worktree (created by `temp_worktree::create`) to the
/// layout-resolved sibling path for `target_branch`, updating git's internal
/// tracking so the worktree is discoverable by `git worktree list` and
/// `daft list`.
///
/// Called on conflict from [`execute_ephemeral_merge`]. Called nowhere else.
/// Firing of the `worktree-post-create` hook happens in the command layer
/// (see `commands::merge`) so this module does not take on `Output` and
/// `JobPresenter` dependencies.
///
/// Layout resolution mirrors the chain used by `checkout` (repo store → yaml
/// → global config → filesystem detection → default). The merge command
/// doesn't expose `--layout`, so the CLI slot stays empty.
///
/// Returns the layout-resolved path on success. On any failure the ephemeral
/// worktree is left in place and the error is returned so the caller can
/// surface both paths for manual recovery.
pub fn promote_ephemeral_to_layout(
    ephemeral_path: &Path,
    target_branch: &str,
    git: &GitCommand,
    project_root: &Path,
) -> Result<PathBuf> {
    use crate::core::global_config::GlobalConfig;
    use crate::core::layout::resolver::{resolve_layout, LayoutResolutionContext};
    use crate::core::multi_remote::path::build_template_context;
    use crate::hooks::TrustDatabase;

    // TODO(refactor): share this resolution block with
    // `commands::checkout::resolve_checkout_layout`. Duplicated here to keep
    // Slice 11 focused; extracting cleanly would expand its scope.
    let global_config = GlobalConfig::load().unwrap_or_default();
    let git_dir = crate::get_git_common_dir().ok();
    let trust_db = TrustDatabase::load().unwrap_or_default();

    let yaml_layout: Option<String> = crate::get_current_worktree_path()
        .ok()
        .and_then(|wt| {
            crate::hooks::yaml_config_loader::load_merged_config(&wt)
                .ok()
                .flatten()
        })
        .and_then(|cfg| cfg.layout);

    let repo_store_layout = git_dir
        .as_ref()
        .and_then(|d| trust_db.get_layout(d).map(String::from));

    let detection = if repo_store_layout.is_none() && yaml_layout.is_none() {
        git_dir
            .as_ref()
            .map(|d| crate::core::layout::detect::detect_layout(d, &global_config))
    } else {
        None
    };

    let (layout, _source) = resolve_layout(&LayoutResolutionContext {
        cli_layout: None,
        repo_store_layout: repo_store_layout.as_deref(),
        yaml_layout: yaml_layout.as_deref(),
        global_config: &global_config,
        detection,
    });

    // For wrapped non-bare layouts (e.g. `contained-classic`), worktree
    // template paths are resolved relative to the wrapper, which is the
    // parent of the current non-bare repo root. Mirrors `checkout.rs:...`.
    let effective_root: PathBuf = if layout.needs_wrapper() {
        project_root
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| project_root.to_path_buf())
    } else {
        project_root.to_path_buf()
    };
    let tctx = build_template_context(&effective_root, target_branch);
    let layout_path = layout.worktree_path(&tctx)?;

    if layout_path.exists() {
        anyhow::bail!(
            "cannot promote ephemeral worktree: destination '{}' already exists",
            layout_path.display()
        );
    }

    if let Some(parent) = layout_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory '{}'", parent.display()))?;
    }

    // `git worktree move` updates git's internal gitdir pointer and the
    // worktree's `.git` link atomically, so the moved worktree is immediately
    // discoverable by `git worktree list`.
    git.worktree_move(ephemeral_path, &layout_path)
        .with_context(|| {
            format!(
                "failed to move ephemeral worktree from '{}' to '{}'",
                ephemeral_path.display(),
                layout_path.display()
            )
        })?;

    Ok(layout_path)
}

/// Returns true when a merge can be a pure fast-forward of the target ref to
/// the source commit.
///
/// Purely fast-forwarding means advancing the target's branch ref to the
/// source SHA without recording a merge commit. That's only possible when:
///
/// * Exactly one source (octopus merges always create a merge commit).
/// * `--squash` is not requested (squash intentionally creates a new commit).
/// * `--no-ff` is not requested (it forces a merge commit).
/// * The target is an ancestor of the source (`is_ancestor == Some(true)`).
///
/// `is_ancestor == None` is treated conservatively as "not known to be an
/// ancestor", returning false so the caller never advances a ref without
/// proof.
pub fn is_pure_ff_eligible(
    sources_len: usize,
    flags: &EffectiveFlags,
    is_ancestor: Option<bool>,
) -> bool {
    if sources_len != 1 {
        return false;
    }
    if flags.squash == Some(true) {
        return false;
    }
    if flags.ff == Some(FfMode::Never) {
        return false;
    }
    is_ancestor == Some(true)
}

/// Advance a branch ref to a new commit via `git update-ref`.
///
/// Caller must have verified (via [`is_pure_ff_eligible`]) that the move is
/// a valid fast-forward. `update-ref` by itself does no such check — it
/// blindly rewrites the ref — so callers must not invoke this without that
/// pre-flight.
pub fn advance_ref_via_plumbing(
    _git: &GitCommand,
    target_branch: &str,
    source_sha: &str,
) -> Result<()> {
    let target_ref = format!("refs/heads/{target_branch}");
    let status = Command::new("git")
        .args(["update-ref", &target_ref, source_sha])
        .status()
        .with_context(|| format!("failed to invoke git update-ref for '{}'", target_ref))?;
    if !status.success() {
        anyhow::bail!("git update-ref failed for '{}'", target_ref);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────
// Daft merge intent marker (Slice 5 Task 5.3)
//
// When `--squash` is committed as part of a cleanup flow (`-r`/`-rb`), daft
// writes a small JSON marker file inside the worktree's git dir BEFORE running
// `git commit`. The marker persists across the editor session so that if the
// editor is aborted and the user later runs `--continue`, daft can resume the
// cleanup step with the original intent (source names, captured SHAs,
// cleanup flags).
//
// Path: `<git-dir>/daft-merge-intent.json`
// ─────────────────────────────────────────────────────────────────────────

/// Cleanup intent persisted across the squash-commit editor session.
///
/// Serialized to `<git-dir>/daft-merge-intent.json` when a squash commit is
/// about to run AND cleanup was requested. Read by `--continue` on the
/// squash-staged state to resume cleanup after the editor session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MergeIntent {
    /// Source branch names (for `classify_source` / `execute_cleanup`).
    pub sources: Vec<String>,
    /// Source SHAs captured at merge start (for stability check before `-D`).
    pub source_shas: Vec<String>,
    /// Whether cleanup should remove the source worktree.
    pub remove_worktree: bool,
    /// Whether cleanup should also delete the source branch (requires
    /// `remove_worktree`).
    pub also_branch: bool,
}

/// Write the merge intent marker file to `git_dir/daft-merge-intent.json`.
///
/// Failures are best-effort: if the write fails, a warning is printed but the
/// operation continues. A missing marker causes `--continue` to skip cleanup
/// (safe degradation) rather than hard-failing.
pub fn write_intent_marker(git_dir: &Path, intent: &MergeIntent) {
    let path = git_dir.join("daft-merge-intent.json");
    match serde_json::to_string(intent) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("warning: failed to write merge intent marker: {e}");
            }
        }
        Err(e) => eprintln!("warning: failed to serialize merge intent: {e}"),
    }
}

/// Read the merge intent marker file from `path`.
///
/// Returns `None` on any error (file absent, unreadable, malformed JSON) — the
/// caller should degrade gracefully (skip cleanup) rather than hard-fail.
pub fn read_intent_marker(path: &Path) -> Option<MergeIntent> {
    let json = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Which finish operation to execute.
///
/// Each variant maps one-to-one to a git-merge subflag (`--abort`,
/// `--continue`, `--quit`). Distinguished here rather than threading raw
/// strings through the call graph so the caller's intent is type-checked and
/// exhaustive matches catch future variants at compile time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishMode {
    Abort,
    Continue,
    Quit,
}

/// Inputs to a merge-finish operation.
///
/// `worktree` mirrors [`StartParams::target`]: `None` means the current
/// worktree, `Some(t)` is resolved via [`resolve_target`] (branch name, dir
/// name, or relative path).
///
/// `commit_flags` carries the message-composing and signing flags supplied on
/// the `--continue` invocation (e.g. `--no-edit`, `-m <msg>`, `-F <file>`,
/// `--signoff`, `--gpg-sign`). Only consulted on the `--continue` + `SquashStaged`
/// path; ignored for `Merge`-state continues (those go to `git merge --continue`
/// which handles the message step itself).
#[derive(Debug, Clone)]
pub struct FinishParams {
    pub worktree: Option<String>,
    pub mode: FinishMode,
    /// Commit-composing flags for `--continue` on squash-staged state.
    /// Defaults to all-None/false (no flags forwarded) when not supplied.
    pub commit_flags: EffectiveFlags,
}

/// Verify the given worktree has an in-progress merge (regular or squash-staged).
///
/// Returns `Ok(())` iff `.git/MERGE_HEAD` is present (regular in-progress
/// merge) **or** the worktree is in the squash-staged state (`SQUASH_MSG`
/// present without `MERGE_HEAD`, with staged changes). Fails with
/// "no in-progress merge in worktree '<path>'" for all other states (clean,
/// mid-rebase, mid-cherry-pick, mid-bisect). Callers that want to surface
/// candidate worktrees on failure should use [`execute_finish`]; this helper
/// is the bare predicate, kept public for direct unit testing.
pub fn ensure_merge_in_progress(worktree: &Path) -> Result<()> {
    match detect_in_progress(worktree)? {
        Some(InProgressOp::Merge) | Some(InProgressOp::SquashStaged) => Ok(()),
        _ => anyhow::bail!("no in-progress merge in worktree '{}'", worktree.display()),
    }
}

/// Enumerate worktrees in the project that have an in-progress merge.
///
/// Used by [`execute_finish`] to produce a helpful "merges in progress
/// elsewhere" hint when the user runs a finish command against a worktree
/// without one. Parses `git worktree list --porcelain` for worktree paths
/// (each stanza begins with a `worktree <path>` line) and probes each with
/// [`detect_in_progress`]. Paths whose probe errors (malformed or missing
/// `.git`) are silently skipped — listing is best-effort, not authoritative.
fn list_worktrees_with_in_progress_merges(git: &GitCommand) -> Result<Vec<PathBuf>> {
    let porcelain = git.worktree_list_porcelain()?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for line in porcelain.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            paths.push(PathBuf::from(p));
        }
    }
    let mut matches = Vec::new();
    for path in paths {
        match detect_in_progress(&path).ok().flatten() {
            Some(InProgressOp::Merge) | Some(InProgressOp::SquashStaged) => {
                matches.push(path);
            }
            _ => {}
        }
    }
    Ok(matches)
}

/// Execute a finish command (abort/continue/quit) against the resolved target.
///
/// Resolves the target via [`resolve_target`] (same logic as
/// [`execute_start`]), then verifies the target has an in-progress merge. If
/// it doesn't, enumerates sibling worktrees with in-progress merges so the
/// error message points the user at where the state actually lives.
///
/// On the happy path, dispatches the appropriate operation:
/// * Regular `Merge` state → `git merge --abort|--continue|--quit`.
/// * `SquashStaged` state:
///   - `Abort` / `Quit` → `git reset --merge` to undo staged changes and
///     remove `SQUASH_MSG`; also removes the daft intent marker if present.
///   - `Continue` → re-opens the editor on `SQUASH_MSG` via `git commit`
///     (honoring commit flags from `params.commit_flags`); on success, reads
///     the intent marker to resume cleanup if originally requested.
///
/// Stdio is inherited so `--continue`'s commit-message editor works
/// interactively.
pub fn execute_finish(params: &FinishParams, git: &GitCommand, project_root: &Path) -> Result<()> {
    let resolved = resolve_target(params.worktree.as_deref(), git, project_root)?;

    // Finish commands require an on-disk worktree: --abort/--continue/--quit
    // act on MERGE_HEAD in the worktree's git dir, and a ref-only target has
    // none. Bail with a clear pointer so the user knows the merge can't be
    // in-progress on a branch without a worktree.
    let path = resolved.path.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "target branch '{}' has no worktree; finish commands require an existing worktree",
            resolved.branch
        )
    })?;

    // Detect the current in-progress state. Route the "no merge in progress"
    // check through the public predicate so behaviour stays in sync with direct
    // callers of `ensure_merge_in_progress`. When the predicate fails, we swap
    // its bare error for a richer message listing candidate worktrees and a
    // concrete retry hint.
    let op = if let Some(op @ (InProgressOp::Merge | InProgressOp::SquashStaged)) =
        detect_in_progress(&path)?
    {
        op
    } else {
        let candidates = list_worktrees_with_in_progress_merges(git).unwrap_or_default();
        let mut msg = format!("no in-progress merge in worktree '{}'", path.display());
        if !candidates.is_empty() {
            msg.push_str("\n\nmerges in progress elsewhere:");
            for c in &candidates {
                // Best-effort branch resolution: if we can read the current
                // branch at the candidate path, show it alongside the path
                // so the user can paste it straight into the retry hint.
                match branch_at_path(git, c) {
                    Ok(branch) => {
                        msg.push_str(&format!("\n  {} (branch: {})", c.display(), branch))
                    }
                    Err(_) => msg.push_str(&format!("\n  {}", c.display())),
                }
            }
            let flag = match params.mode {
                FinishMode::Abort => "abort",
                FinishMode::Continue => "continue",
                FinishMode::Quit => "quit",
            };
            // Single candidate → concrete retry command; multiple
            // candidates or resolution failure → placeholder.
            let retry_target = if candidates.len() == 1 {
                branch_at_path(git, &candidates[0]).ok()
            } else {
                None
            };
            match retry_target {
                Some(branch) => {
                    msg.push_str(&format!("\n\nretry with: daft merge --{flag} {branch}"))
                }
                None => msg.push_str(&format!("\n\nretry with: daft merge --{flag} <branch>")),
            }
        }
        anyhow::bail!(msg);
    };

    // Dispatch based on detected state.
    let target_branch = resolved.branch.clone();
    match op {
        InProgressOp::SquashStaged => {
            finish_squash_staged(params, git, project_root, &path, &target_branch)?;
        }
        InProgressOp::Merge => {
            let flag = match params.mode {
                FinishMode::Abort => "--abort",
                FinishMode::Continue => "--continue",
                FinishMode::Quit => "--quit",
            };

            let status = Command::new("git")
                .args(["merge", flag])
                .current_dir(&path)
                .status()
                .with_context(|| {
                    format!(
                        "failed to invoke git merge {} in '{}'",
                        flag,
                        path.display()
                    )
                })?;

            if !status.success() {
                anyhow::bail!("git merge {} failed in '{}'", flag, path.display());
            }
        }
        // Other states (Rebase, CherryPick, Bisect) are filtered out above;
        // this branch is unreachable but required for exhaustiveness.
        _ => unreachable!("non-merge op passed detect filter"),
    }
    Ok(())
}

/// Handle a finish command (abort/continue/quit) for the squash-staged state.
///
/// This state arises when `git merge --squash` ran successfully but the
/// commit step is pending: `MERGE_MSG` exists, `MERGE_HEAD` does not.
///
/// - Abort/Quit: `git reset --merge` resets the index back to HEAD and
///   removes `MERGE_MSG`. The daft intent marker (if present) is also removed.
/// - Continue: runs `git commit` with commit flags from `params.commit_flags`
///   (e.g. `--no-edit`, `-m`). On success, reads the intent marker to resume
///   cleanup if the original merge requested it.
fn finish_squash_staged(
    params: &FinishParams,
    git: &GitCommand,
    project_root: &Path,
    path: &Path,
    target_branch: &str,
) -> Result<()> {
    let git_dir = resolve_worktree_git_dir(path)?;
    // `git merge --squash` writes SQUASH_MSG as the pre-populated commit
    // message template; git does NOT write MERGE_MSG in the squash path.
    let squash_msg_path = git_dir.join("SQUASH_MSG");
    let intent_path = git_dir.join("daft-merge-intent.json");

    match params.mode {
        FinishMode::Abort | FinishMode::Quit => {
            // `git reset --merge` resets staged changes back to HEAD.
            let status = Command::new("git")
                .args(["reset", "--merge"])
                .current_dir(path)
                .status()
                .context("failed to invoke `git reset --merge`")?;
            if !status.success() {
                anyhow::bail!("`git reset --merge` failed in '{}'", path.display());
            }
            // Explicitly remove SQUASH_MSG (git reset --merge may or may not
            // remove it depending on the git version; be explicit for test
            // determinism).
            let _ = std::fs::remove_file(&squash_msg_path);
            // Remove the daft intent marker so a subsequent --abort doesn't
            // find stale cleanup intent.
            let _ = std::fs::remove_file(&intent_path);
            println!("Aborted.");
        }
        FinishMode::Continue => {
            // Run git commit; git will use SQUASH_MSG as the editor template
            // if no explicit message is provided (-m / -F). --no-edit uses
            // the SQUASH_MSG content verbatim without opening an editor.
            let mut commit_argv: Vec<String> = vec!["commit".to_string()];
            commit_argv.extend(render_commit_flags(&params.commit_flags));

            let status = Command::new("git")
                .args(&commit_argv)
                .current_dir(path)
                .status()
                .context("failed to invoke `git commit` for squash-staged continue")?;

            if !status.success() {
                // Editor aborted or commit refused; leave staged state + marker
                // intact so the user can retry with --continue or --abort.
                anyhow::bail!(
                    "commit aborted; squash changes are still staged on '{}'. \
                     Use `daft merge --continue` to retry or `daft merge --abort` to discard.",
                    path.display()
                );
            }

            // Commit succeeded. Read the intent marker (if present) to decide
            // whether to run post-merge cleanup.
            let intent = read_intent_marker(&intent_path);

            // Remove the marker now that we've consumed it.
            let _ = std::fs::remove_file(&intent_path);

            // Run cleanup if the original merge requested it and the marker
            // was successfully read.
            if let Some(intent) = intent {
                if intent.remove_worktree {
                    // Stability check: re-resolve each source SHA and compare
                    // to the captured SHA before running cleanup.
                    for (source, captured_sha) in
                        intent.sources.iter().zip(intent.source_shas.iter())
                    {
                        let current_sha = git
                            .rev_parse(source)
                            .with_context(|| {
                                format!("failed to resolve source ref '{source}' before cleanup")
                            })
                            .unwrap_or_default();
                        if !current_sha.is_empty() && current_sha != *captured_sha {
                            anyhow::bail!(
                                "cleanup refused: source '{}' moved during merge \
                                 (was {}, now {}); skipping cleanup to avoid losing work. \
                                 Re-run cleanup manually if you have reconciled.",
                                source,
                                &captured_sha[..12.min(captured_sha.len())],
                                &current_sha[..12.min(current_sha.len())]
                            );
                        }
                    }

                    let cleanup_opts = CleanupOptions {
                        remove_worktree: intent.remove_worktree,
                        also_branch: intent.also_branch,
                        squash_committed: true,
                    };
                    execute_cleanup(
                        &intent.sources,
                        &cleanup_opts,
                        git,
                        project_root,
                        target_branch,
                    )?;
                    println!(
                        "Squash merged and cleaned up {}.",
                        intent.sources.join(", ")
                    );
                } else {
                    // Cleanup not requested — just confirm the commit.
                    let sha = git.rev_parse("HEAD").unwrap_or_default();
                    println!(
                        "Squash merged {} into {} as {}.",
                        intent.sources.join(", "),
                        resolved_branch_or_unknown(path, git),
                        &sha[..12.min(sha.len())]
                    );
                }
            } else {
                // No intent marker (e.g. squash was done with --no-commit, or
                // the marker was lost). Just confirm the commit succeeded.
                let sha = git.rev_parse("HEAD").unwrap_or_default();
                println!("Squash committed {}.", &sha[..12.min(sha.len())]);
            }
        }
    }
    Ok(())
}

/// Best-effort: read the current branch of the worktree at `path`. Falls back
/// to `"<unknown>"` if reading fails.
fn resolved_branch_or_unknown(path: &Path, git: &GitCommand) -> String {
    branch_at_path(git, path).unwrap_or_else(|_| "<unknown>".to_string())
}

// ─────────────────────────────────────────────────────────────────────────
// Post-merge cleanup (Slice 12): `-r` removes the source worktree; `-rb` also
// deletes the source branch via `git branch -d` semantics (no force).
// ─────────────────────────────────────────────────────────────────────────

/// Options for post-merge cleanup.
///
/// Constructed from the `-r`/`--remove` and `-b`/`--and-branch` CLI flags
/// by the command layer. `also_branch` is only meaningful when
/// `remove_worktree` is true (clap enforces `-b` requires `-r`).
#[derive(Debug, Clone, Default)]
pub struct CleanupOptions {
    /// Remove the source worktree (if one exists).
    pub remove_worktree: bool,
    /// Also delete the source branch via `git branch -d`. Requires
    /// `remove_worktree`.
    pub also_branch: bool,
    /// When true, the caller has direct first-party evidence that all content
    /// on the source branch was captured in a daft-driven squash + commit
    /// (Slice 4). Under this flag, branch deletion uses `-D` (force-delete)
    /// instead of `-d` and the branch-deletion validation uses SHA stability
    /// rather than reachability. For Slice 3, callers set this to `false`;
    /// Slice 4 wires `true` for the squash-committed path.
    pub squash_committed: bool,
}

/// Classification of a source reference for cleanup purposes.
///
/// Each variant captures just enough state for [`execute_cleanup`] to decide
/// what to do: which worktree path to remove (if any) and which branch name
/// to delete (if `also_branch` is requested). Commit SHAs, tags, and other
/// non-branch refs fall into `CommitOrOther`, for which no cleanup is
/// possible and the source is silently skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceClass {
    /// Source is a branch that has a worktree in the project.
    BranchWithWorktree {
        worktree_path: PathBuf,
        branch: String,
    },
    /// Source is a branch that does NOT have a worktree.
    BranchNoWorktree { branch: String },
    /// Source is not a branch (commit SHA, tag, etc.) — nothing to clean up.
    CommitOrOther,
}

/// Classify a source ref for cleanup.
///
/// Probes [`GitCommand::resolve_worktree_path`] first, which matches `source`
/// against worktree paths, branch names, and directory names. On a hit, the
/// worktree's current branch is read via [`branch_at_path`]; if that read
/// fails (e.g. detached HEAD at the worktree path), we fall back to using
/// `source` itself as the branch name so `also_branch` deletion still has
/// something to target — `git branch -d` will produce its own error in that
/// case.
///
/// On miss, we fall back to [`GitCommand::show_ref_exists`] on
/// `refs/heads/<source>` to detect plain branch refs. Anything else (commit
/// SHAs, tags, remote-tracking refs) returns `CommitOrOther` — no cleanup
/// work is possible and no error is raised here; the caller silently skips.
pub fn classify_source(source: &str, git: &GitCommand, project_root: &Path) -> SourceClass {
    if let Ok(worktree_path) = git.resolve_worktree_path(source, project_root) {
        let branch = branch_at_path(git, &worktree_path).unwrap_or_else(|_| source.to_string());
        return SourceClass::BranchWithWorktree {
            worktree_path,
            branch,
        };
    }
    if git
        .show_ref_exists(&format!("refs/heads/{source}"))
        .unwrap_or(false)
    {
        return SourceClass::BranchNoWorktree {
            branch: source.to_string(),
        };
    }
    SourceClass::CommitOrOther
}

/// One unit of cleanup work, produced by [`plan_cleanup`] after
/// pre-validation. Items where both `worktree_path` and `branch_name`
/// would be `None` are dropped during planning.
#[derive(Debug, Clone)]
pub struct CleanupItem {
    pub source: String,
    pub worktree_path: Option<PathBuf>,
    pub branch_name: Option<String>,
    pub force_delete: bool,
}

/// Pre-validate cleanup for `sources` and return the items to mutate.
/// Pure: never touches the filesystem or git refs beyond reads.
pub fn plan_cleanup(
    sources: &[String],
    options: &CleanupOptions,
    git: &GitCommand,
    project_root: &Path,
    target_branch: &str,
) -> Result<Vec<CleanupItem>> {
    let mut items: Vec<CleanupItem> = Vec::with_capacity(sources.len());
    let mut validation_errors: Vec<String> = Vec::new();

    for src in sources {
        let class = classify_source(src, git, project_root);
        match &class {
            SourceClass::BranchWithWorktree {
                worktree_path,
                branch,
            } => {
                if options.remove_worktree {
                    match git.has_uncommitted_changes_in(worktree_path) {
                        Ok(true) => validation_errors.push(format!(
                            "source worktree '{}' has uncommitted changes; \
                             commit or stash them before cleanup",
                            worktree_path.display()
                        )),
                        Ok(false) => {}
                        Err(e) => validation_errors.push(format!(
                            "failed to check cleanliness of '{}': {}",
                            worktree_path.display(),
                            e
                        )),
                    }
                }
                if options.also_branch
                    && !options.squash_committed
                    && !is_branch_merged_into(git, branch, target_branch)
                {
                    validation_errors.push(format!(
                        "source branch '{}' is not fully merged into '{}'; \
                         cleanup pre-validation refused branch deletion",
                        branch, target_branch
                    ));
                }
                items.push(CleanupItem {
                    source: src.clone(),
                    worktree_path: options.remove_worktree.then(|| worktree_path.clone()),
                    branch_name: options.also_branch.then(|| branch.clone()),
                    force_delete: options.squash_committed,
                });
            }
            SourceClass::BranchNoWorktree { branch } => {
                if options.also_branch
                    && !options.squash_committed
                    && !is_branch_merged_into(git, branch, target_branch)
                {
                    validation_errors.push(format!(
                        "source branch '{}' is not fully merged into '{}'; \
                         cleanup pre-validation refused branch deletion",
                        branch, target_branch
                    ));
                }
                if options.also_branch {
                    items.push(CleanupItem {
                        source: src.clone(),
                        worktree_path: None,
                        branch_name: Some(branch.clone()),
                        force_delete: options.squash_committed,
                    });
                }
            }
            SourceClass::CommitOrOther => {
                // Nothing to clean up.
            }
        }
    }

    if !validation_errors.is_empty() {
        anyhow::bail!(
            "cleanup pre-validation failed:\n  {}",
            validation_errors.join("\n  ")
        );
    }
    Ok(items)
}

/// Execute post-merge cleanup. Called only after a successful merge (not on
/// conflict, not on already-up-to-date, not on failure).
///
/// **Two-phase transactional execution:**
///
/// Phase 1 — validate every step that would mutate state. For each source:
/// * `BranchWithWorktree` with `remove_worktree`:
///   - Check that the source worktree has no uncommitted changes. If dirty,
///     abort with a pre-validation error before touching anything.
///   - If `also_branch` and `squash_committed == false`: verify that the
///     source branch tip is reachable from `target_branch` (i.e. `branch -d`
///     would succeed). If not reachable, abort with a pre-validation error.
/// * `BranchNoWorktree` with `also_branch` and `squash_committed == false`:
///   - Apply the same reachability check.
/// * `CommitOrOther` — silently skip; no cleanup is possible.
///
/// Phase 2 — mutate (only reached if Phase 1 passes completely):
/// * Remove each worktree, then delete each branch, in source order.
/// * If a Phase 2 step fails after Phase 1 passed (concurrent modification),
///   bail with a transactional failure message that names any already-mutated
///   state.
///
/// `target_branch` is used for the branch-reachability check (resolves the
/// target tip explicitly, not via process CWD, so cross-worktree merges work
/// correctly).
pub(crate) fn execute_cleanup(
    sources: &[String],
    options: &CleanupOptions,
    git: &GitCommand,
    project_root: &Path,
    target_branch: &str,
) -> Result<()> {
    let plan = plan_cleanup(sources, options, git, project_root, target_branch)?;

    // ── Phase 2: mutate ───────────────────────────────────────────────────────
    //
    // All validation passed. Perform mutations in order: remove worktrees
    // first, then delete branches. Track completed steps for the error message
    // if a Phase 2 step fails unexpectedly (race condition).
    let mut completed: Vec<String> = Vec::new();

    for item in &plan {
        if let Some(ref wt_path) = item.worktree_path {
            println!("Removing worktree at {}...", wt_path.display());
            git.worktree_remove(wt_path, false).with_context(|| {
                let done = if completed.is_empty() {
                    "nothing removed yet".to_string()
                } else {
                    format!("already removed: {}", completed.join(", "))
                };
                format!(
                    "cleanup partially failed: failed to remove worktree '{}' \
                     (source '{}'); {}",
                    wt_path.display(),
                    item.source,
                    done
                )
            })?;
            completed.push(format!("worktree '{}'", wt_path.display()));
        }
    }

    for item in &plan {
        if let Some(ref branch) = item.branch_name {
            println!("Deleting branch {}...", branch);
            git.branch_delete(branch, item.force_delete)
                .with_context(|| {
                    let done = if completed.is_empty() {
                        "nothing removed yet".to_string()
                    } else {
                        format!("already removed: {}", completed.join(", "))
                    };
                    format!(
                        "cleanup partially failed: failed to delete branch '{}' \
                         (source '{}'); {}",
                        branch, item.source, done
                    )
                })?;
            completed.push(format!("branch '{}'", branch));
        }
    }

    Ok(())
}

/// Return `true` if `branch` tip is reachable from `target_branch` (i.e.
/// `git branch -d <branch>` would succeed from a safety perspective).
///
/// Uses `git merge-base --is-ancestor <branch> <target_branch>` — the
/// same check `branch -d` performs internally. Returns `false` on any
/// error (including unknown refs) so the caller surfaces a validation
/// failure rather than silently skipping.
fn is_branch_merged_into(git: &GitCommand, branch: &str, target_branch: &str) -> bool {
    // Resolve branch tip to SHA to avoid ambiguity between branch names and
    // commit-ishes in merge-base --is-ancestor.
    let branch_sha = match git.rev_parse(&format!("refs/heads/{}", branch)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let target_sha = match git.rev_parse(&format!("refs/heads/{}", target_branch)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    git.merge_base_is_ancestor(&branch_sha, &target_sha)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    //! Test coverage notes for [`resolve_target`]:
    //!
    //! * `branch_at_path` is covered directly against a real `git init`ed
    //!   temp directory.
    //! * The happy-path for `resolve_target(Some(...), ...)` and
    //!   `resolve_target(None, ...)` requires a multi-worktree fixture
    //!   (either setting CWD to exercise the `None` branch, or using
    //!   `git worktree add` to exercise `Some(...)`). Both are expensive
    //!   to stand up here and fragile in parallel tests because
    //!   `get_current_worktree_path` and `symbolic_ref_short_head` read
    //!   the process CWD. End-to-end coverage lives in the YAML scenario
    //!   `tests/manual/scenarios/merge/cross-worktree.yml`.
    //! * The error-path `resolve_target(Some("bogus"), ...)` bubbles up
    //!   from `GitCommand::resolve_worktree_path`, which is exercised by
    //!   its own tests and by the `carry` scenarios.
    use super::*;
    use serial_test::serial;
    use std::process::Command as ShellCommand;

    /// RAII helper: saves the current working directory on construction and
    /// restores it on drop. Tests that call `std::env::set_current_dir` use
    /// this to avoid leaving cwd pointing at a deleted tempdir for the next
    /// test (which would panic in `std::env::current_dir`).
    struct CwdGuard {
        original: PathBuf,
    }

    impl CwdGuard {
        fn new() -> Self {
            Self {
                original: std::env::current_dir().expect("cwd readable at test start"),
            }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            // Best-effort: if the original cwd no longer exists, fall back to
            // the system temp dir so subsequent tests can at least read cwd.
            if std::env::set_current_dir(&self.original).is_err() {
                let _ = std::env::set_current_dir(std::env::temp_dir());
            }
        }
    }

    fn init_repo(path: &Path) {
        ShellCommand::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .status()
            .unwrap();
        // Identity via env avoids any global config dependency.
        ShellCommand::new("git")
            .args(["commit", "--allow-empty", "-q", "-m", "init"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap();
    }

    #[test]
    fn branch_at_path_reads_current_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        let git = GitCommand::new(true);
        let branch = branch_at_path(&git, tmp.path()).unwrap();
        assert_eq!(branch, "main");
    }

    #[test]
    fn branch_at_path_reads_via_gitoxide() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        let git = GitCommand::new(true).with_gitoxide(true);
        let branch = branch_at_path(&git, tmp.path()).unwrap();
        assert_eq!(branch, "main");
    }

    #[test]
    fn branch_at_path_fails_on_detached_head() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        // Detach HEAD at the current commit.
        ShellCommand::new("git")
            .args(["checkout", "--detach", "-q"])
            .current_dir(tmp.path())
            .status()
            .unwrap();

        let git = GitCommand::new(true);
        let err = branch_at_path(&git, tmp.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("detached HEAD"), "unexpected error: {msg}");
    }

    #[test]
    fn start_params_holds_sources() {
        let params = StartParams {
            sources: vec!["feature/x".to_string(), "feature/y".to_string()],
            target: None,
            ..StartParams::default()
        };
        assert_eq!(params.sources.len(), 2);
        assert_eq!(params.sources[0], "feature/x");
        assert!(params.target.is_none());
    }

    #[test]
    fn start_params_holds_target() {
        let params = StartParams {
            sources: vec!["feature/x".to_string()],
            target: Some("main".to_string()),
            ..StartParams::default()
        };
        assert_eq!(params.target.as_deref(), Some("main"));
    }

    #[test]
    fn render_flags_empty_returns_empty_vec() {
        let flags = EffectiveFlags::default();
        assert!(render_flags(&flags).is_empty());
    }

    #[test]
    fn render_flags_message_and_file_paired() {
        let flags = EffectiveFlags {
            message: Some("hello".into()),
            file: Some(PathBuf::from("/tmp/msg.txt")),
            ..EffectiveFlags::default()
        };
        assert_eq!(
            render_flags(&flags),
            vec!["-m", "hello", "-F", "/tmp/msg.txt"]
        );
    }

    #[test]
    fn render_flags_ff_auto() {
        let flags = EffectiveFlags {
            ff: Some(FfMode::Auto),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_flags(&flags), vec!["--ff"]);
    }

    #[test]
    fn render_flags_ff_only() {
        let flags = EffectiveFlags {
            ff: Some(FfMode::Only),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_flags(&flags), vec!["--ff-only"]);
    }

    #[test]
    fn render_flags_ff_never() {
        let flags = EffectiveFlags {
            ff: Some(FfMode::Never),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_flags(&flags), vec!["--no-ff"]);
    }

    #[test]
    fn render_flags_multiple_strategy_options_separate_pairs() {
        let flags = EffectiveFlags {
            strategy_options: vec!["ours".into(), "ignore-space-at-eol".into()],
            ..EffectiveFlags::default()
        };
        assert_eq!(
            render_flags(&flags),
            vec!["-X", "ours", "-X", "ignore-space-at-eol"]
        );
    }

    #[test]
    fn render_flags_gpg_default_emits_bare_dash_s() {
        let flags = EffectiveFlags {
            gpg_sign: Some(GpgSign::Default),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_flags(&flags), vec!["-S"]);
    }

    #[test]
    fn render_flags_gpg_keyid_emits_concatenated() {
        let flags = EffectiveFlags {
            gpg_sign: Some(GpgSign::KeyId("ABC123".into())),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_flags(&flags), vec!["-SABC123"]);
    }

    #[test]
    fn render_flags_gpg_disabled_emits_no_gpg_sign() {
        let flags = EffectiveFlags {
            gpg_sign: Some(GpgSign::Disabled),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_flags(&flags), vec!["--no-gpg-sign"]);
    }

    #[test]
    fn render_flags_edit_variants() {
        let yes = EffectiveFlags {
            edit: Some(true),
            ..EffectiveFlags::default()
        };
        let no = EffectiveFlags {
            edit: Some(false),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_flags(&yes), vec!["--edit"]);
        assert_eq!(render_flags(&no), vec!["--no-edit"]);
    }

    #[test]
    fn render_flags_combination_preserves_declared_order_non_squash() {
        // Non-squash: all flags emitted as before. Lock down the canonical
        // emission order (message, edit, cleanup, ff, commit, signoff,
        // strategy, strategy-options, gpg, verify, allow-unrelated, stat).
        let flags = EffectiveFlags {
            message: Some("m".into()),
            edit: Some(false),
            cleanup: Some("strip".into()),
            ff: Some(FfMode::Never),
            squash: None,
            commit: None,
            signoff: Some(true),
            strategy: Some("recursive".into()),
            strategy_options: vec!["theirs".into()],
            gpg_sign: Some(GpgSign::Default),
            verify_signatures: Some(true),
            allow_unrelated_histories: true,
            stat: Some(false),
            ..EffectiveFlags::default()
        };
        assert_eq!(
            render_flags(&flags),
            vec![
                "-m",
                "m",
                "--no-edit",
                "--cleanup",
                "strip",
                "--no-ff",
                "--signoff",
                "-s",
                "recursive",
                "-X",
                "theirs",
                "-S",
                "--verify-signatures",
                "--allow-unrelated-histories",
                "--no-stat",
            ]
        );
    }

    #[test]
    fn render_flags_combination_squash_strips_message_flags() {
        // Squash path: message-composing flags (-m, --no-edit, --cleanup,
        // --signoff, -S) are stripped from the merge argv (moved to
        // render_commit_flags). Only squash, no-commit, strategy, verify,
        // allow-unrelated, stat remain.
        let flags = EffectiveFlags {
            message: Some("m".into()),
            edit: Some(false),
            cleanup: Some("strip".into()),
            ff: Some(FfMode::Never),
            squash: Some(true),
            commit: Some(false),
            signoff: Some(true),
            strategy: Some("recursive".into()),
            strategy_options: vec!["theirs".into()],
            gpg_sign: Some(GpgSign::Default),
            verify_signatures: Some(true),
            allow_unrelated_histories: true,
            stat: Some(false),
            ..EffectiveFlags::default()
        };
        assert_eq!(
            render_flags(&flags),
            vec![
                "--no-ff",
                "--squash",
                "--no-commit",
                "-s",
                "recursive",
                "-X",
                "theirs",
                "--verify-signatures",
                "--allow-unrelated-histories",
                "--no-stat",
            ]
        );
    }

    #[test]
    fn render_flags_squash_suppresses_cleanup() {
        // --cleanup must NOT be emitted by render_flags when squash is true.
        // git merge --squash ignores cleanup; it must go to render_commit_flags.
        let flags = EffectiveFlags {
            cleanup: Some("strip".into()),
            squash: Some(true),
            ..EffectiveFlags::default()
        };
        let result = render_flags(&flags);
        assert!(
            !result.contains(&"--cleanup".into()),
            "--cleanup must not appear in merge argv under --squash, got: {result:?}"
        );
    }

    #[test]
    fn render_flags_non_squash_emits_cleanup() {
        // --cleanup IS emitted by render_flags on the regular (non-squash) path.
        let flags = EffectiveFlags {
            cleanup: Some("strip".into()),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_flags(&flags), vec!["--cleanup", "strip"]);
    }

    #[test]
    fn render_flags_allow_unrelated_histories_only_when_true() {
        let off = EffectiveFlags::default();
        assert!(render_flags(&off).is_empty());
        let on = EffectiveFlags {
            allow_unrelated_histories: true,
            ..EffectiveFlags::default()
        };
        assert_eq!(render_flags(&on), vec!["--allow-unrelated-histories"]);
    }

    #[test]
    fn start_outcome_default_is_clean() {
        let outcome = StartOutcome::default();
        assert!(!outcome.already_up_to_date);
        assert!(!outcome.failed);
        assert_eq!(outcome.target_path, PathBuf::new());
        assert!(outcome.conflicted_files.is_empty());
        assert!(!outcome.emitted_terminal_message);
        assert!(!outcome.ephemeral_promoted);
        assert!(!outcome.squash_staged_only);
        assert!(outcome.squash_commit_sha.is_none());
        assert!(!outcome.commit_aborted);
    }

    #[test]
    fn parses_uu_conflict_line() {
        assert_eq!(
            parse_conflicted_files_from_porcelain("UU c.txt\n"),
            vec!["c.txt"]
        );
    }

    #[test]
    fn parses_multiple_conflict_types() {
        let input = "UU a.txt\nAA b.txt\n M normal.txt\nDD c.txt\n";
        assert_eq!(
            parse_conflicted_files_from_porcelain(input),
            vec!["a.txt", "b.txt", "c.txt"]
        );
    }

    #[test]
    fn parses_all_conflict_xy_codes() {
        // Every code listed in the porcelain docs as unmerged.
        let input = "UU a\nAA b\nDD c\nAU d\nUA e\nDU f\nUD g\n";
        assert_eq!(
            parse_conflicted_files_from_porcelain(input),
            vec!["a", "b", "c", "d", "e", "f", "g"]
        );
    }

    #[test]
    fn skips_non_conflict_status_codes() {
        let input = " M modified.txt\n?? untracked.txt\n A added.txt\n M  deleted.txt\n";
        assert!(parse_conflicted_files_from_porcelain(input).is_empty());
    }

    #[test]
    fn handles_empty_porcelain() {
        assert!(parse_conflicted_files_from_porcelain("").is_empty());
    }

    #[test]
    fn conflicted_files_on_non_git_dir_returns_empty() {
        // `git status` fails on a plain dir with no .git; helper swallows that
        // into an empty list rather than bubbling an error up.
        let tmp = tempfile::tempdir().unwrap();
        let files = conflicted_files(tmp.path()).unwrap_or_default();
        assert!(files.is_empty());
    }

    #[test]
    fn refuses_when_source_equals_target() {
        let target = ResolvedTarget {
            branch: "main".into(),
            path: Some(PathBuf::from("/repo/main")),
        };
        let err = validate_distinct(&["main".to_string()], &target).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("same branch"), "unexpected error: {msg}");
    }

    #[test]
    fn allows_distinct_source_and_target() {
        let target = ResolvedTarget {
            branch: "main".into(),
            path: Some(PathBuf::from("/repo/main")),
        };
        assert!(validate_distinct(&["feat".to_string()], &target).is_ok());
    }

    #[test]
    fn refuses_when_target_matches_later_source() {
        let target = ResolvedTarget {
            branch: "main".into(),
            path: Some(PathBuf::from("/tmp")),
        };
        let result = validate_distinct(&["feat".into(), "main".into()], &target);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("same branch"));
    }

    #[test]
    fn refuses_when_source_uses_refs_heads_prefix() {
        let target = ResolvedTarget {
            branch: "main".into(),
            path: Some(PathBuf::from("/tmp")),
        };
        let result = validate_distinct(&["refs/heads/main".into()], &target);
        assert!(result.is_err());
    }

    #[test]
    fn detects_in_progress_merge() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git/MERGE_HEAD"), "deadbeef").unwrap();
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::Merge)
        );
    }

    #[test]
    fn detects_in_progress_rebase() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git/rebase-merge")).unwrap();
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::Rebase)
        );
    }

    #[test]
    fn detects_in_progress_rebase_apply_variant() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git/rebase-apply")).unwrap();
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::Rebase)
        );
    }

    #[test]
    fn detects_in_progress_cherry_pick() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git/CHERRY_PICK_HEAD"), "c0ffee").unwrap();
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::CherryPick)
        );
    }

    #[test]
    fn detects_in_progress_bisect() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git/BISECT_LOG"), "").unwrap();
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::Bisect)
        );
    }

    #[test]
    fn clean_worktree_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        assert_eq!(detect_in_progress(tmp.path()).unwrap(), None);
    }

    #[test]
    fn validate_clean_target_ok_on_clean_repo() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let git = GitCommand::new(true);
        assert!(validate_clean_target(&git, tmp.path()).is_ok());
    }

    #[test]
    fn validate_clean_target_refuses_untracked_file() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        // Untracked file — `git status --porcelain` reports it as `?? path`.
        std::fs::write(tmp.path().join("dirty.txt"), "hello\n").unwrap();
        let git = GitCommand::new(true);
        let err = validate_clean_target(&git, tmp.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("uncommitted changes"),
            "unexpected error: {msg}"
        );
        assert!(
            msg.contains("commit or stash"),
            "expected remediation hint in error: {msg}"
        );
    }

    #[test]
    fn announces_octopus_for_multi_source() {
        let sources = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let msg = announcement(&sources, "main").expect("multi-source should announce");
        assert!(msg.contains("3 sources"), "unexpected message: {msg}");
        assert!(msg.contains("octopus"), "unexpected message: {msg}");
        assert!(msg.contains("main"), "unexpected message: {msg}");
    }

    #[test]
    fn no_announcement_for_single_source() {
        let sources = vec!["feat".to_string()];
        assert!(announcement(&sources, "main").is_none());
    }

    #[test]
    fn ensure_merge_ok_when_head_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git/MERGE_HEAD"), "deadbeef").unwrap();
        assert!(ensure_merge_in_progress(tmp.path()).is_ok());
    }

    #[test]
    fn ensure_merge_errors_when_no_merge() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let err = ensure_merge_in_progress(tmp.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("no in-progress merge"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn follows_linked_worktree_gitdir_pointer() {
        // Simulate a linked worktree layout: .git is a file whose first line
        // reads `gitdir: <relative-path>` pointing at the per-worktree dir.
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        let real_gitdir = tmp.path().join("real-gitdir");
        std::fs::create_dir_all(real_gitdir.join("rebase-merge")).unwrap();
        // .git is a file pointing at the real gitdir (using an absolute path
        // here exercises the is_absolute() branch in detect_in_progress).
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", real_gitdir.display()),
        )
        .unwrap();

        assert_eq!(
            detect_in_progress(&worktree).unwrap(),
            Some(InProgressOp::Rebase)
        );
    }

    // ── squash-staged detection tests (Task 5.1) ──────────────────────────

    /// Helper: create a staged change in the given repo (needs at least one commit).
    fn stage_file(path: &Path) {
        std::fs::write(path.join("staged.txt"), "content\n").unwrap();
        ShellCommand::new("git")
            .args(["add", "staged.txt"])
            .current_dir(path)
            .status()
            .unwrap();
    }

    #[test]
    fn detect_in_progress_squash_staged() {
        // SQUASH_MSG present (written by git merge --squash), MERGE_HEAD absent,
        // staged changes → SquashStaged.
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        stage_file(tmp.path());
        std::fs::write(
            tmp.path().join(".git/SQUASH_MSG"),
            "Squashed commit of ...\n",
        )
        .unwrap();
        // Explicitly absent MERGE_HEAD (not written).
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::SquashStaged)
        );
    }

    #[test]
    fn detect_in_progress_no_squash_when_no_staged() {
        // SQUASH_MSG present but no staged changes → None (false-positive guard).
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        // Do NOT stage anything.
        std::fs::write(
            tmp.path().join(".git/SQUASH_MSG"),
            "Squashed commit of ...\n",
        )
        .unwrap();
        assert_eq!(detect_in_progress(tmp.path()).unwrap(), None);
    }

    #[test]
    fn detect_in_progress_merge_wins_over_squash_staged() {
        // MERGE_HEAD present (real in-progress merge) → Merge is detected first
        // even when SQUASH_MSG is also present; SquashStaged is not returned.
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        stage_file(tmp.path());
        std::fs::write(tmp.path().join(".git/MERGE_HEAD"), "deadbeef").unwrap();
        std::fs::write(
            tmp.path().join(".git/SQUASH_MSG"),
            "Squashed commit of ...\n",
        )
        .unwrap();
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::Merge)
        );
    }

    // ── end squash-staged detection tests ──────────────────────────────────

    #[test]
    fn ff_eligible_for_single_ancestor_default_flags() {
        assert!(is_pure_ff_eligible(
            1,
            &EffectiveFlags::default(),
            Some(true)
        ));
    }

    #[test]
    fn ff_not_eligible_for_multi_source() {
        // Octopus merges always create a merge commit — never a pure FF.
        assert!(!is_pure_ff_eligible(
            2,
            &EffectiveFlags::default(),
            Some(true)
        ));
    }

    #[test]
    fn ff_not_eligible_when_not_ancestor() {
        // Divergent histories: target is NOT an ancestor of source.
        assert!(!is_pure_ff_eligible(
            1,
            &EffectiveFlags::default(),
            Some(false)
        ));
    }

    #[test]
    fn ff_not_eligible_when_squash() {
        let flags = EffectiveFlags {
            squash: Some(true),
            ..EffectiveFlags::default()
        };
        assert!(!is_pure_ff_eligible(1, &flags, Some(true)));
    }

    #[test]
    fn ff_not_eligible_when_no_ff() {
        let flags = EffectiveFlags {
            ff: Some(FfMode::Never),
            ..EffectiveFlags::default()
        };
        assert!(!is_pure_ff_eligible(1, &flags, Some(true)));
    }

    #[test]
    fn ff_not_eligible_when_is_ancestor_unknown() {
        // Conservative: without proof, never advance a ref.
        assert!(!is_pure_ff_eligible(1, &EffectiveFlags::default(), None));
    }

    #[test]
    fn ff_eligible_allows_explicit_ff_auto_and_only() {
        // --ff (Auto) and --ff-only (Only) do not forbid FF — they require it.
        let auto = EffectiveFlags {
            ff: Some(FfMode::Auto),
            ..EffectiveFlags::default()
        };
        let only = EffectiveFlags {
            ff: Some(FfMode::Only),
            ..EffectiveFlags::default()
        };
        assert!(is_pure_ff_eligible(1, &auto, Some(true)));
        assert!(is_pure_ff_eligible(1, &only, Some(true)));
    }

    // --- decide_adopt -------------------------------------------------------

    #[test]
    fn decide_adopt_flag_yes_wins_over_preset() {
        // Even a No preset loses to an explicit --adopt-target flag.
        assert_eq!(
            decide_adopt(true, false, false, AdoptPreset::No),
            AdoptDecision::Yes
        );
        assert_eq!(
            decide_adopt(true, false, true, AdoptPreset::Prompt),
            AdoptDecision::Yes
        );
    }

    #[test]
    fn decide_adopt_flag_no_wins_over_preset() {
        // Even a Yes preset loses to an explicit --no-adopt-target flag.
        assert_eq!(
            decide_adopt(false, true, true, AdoptPreset::Yes),
            AdoptDecision::No
        );
        assert_eq!(
            decide_adopt(false, true, false, AdoptPreset::Prompt),
            AdoptDecision::No
        );
    }

    #[test]
    fn decide_adopt_preset_yes_no_flag() {
        // Preset::Yes is final regardless of TTY state.
        assert_eq!(
            decide_adopt(false, false, true, AdoptPreset::Yes),
            AdoptDecision::Yes
        );
        assert_eq!(
            decide_adopt(false, false, false, AdoptPreset::Yes),
            AdoptDecision::Yes
        );
    }

    #[test]
    fn decide_adopt_preset_no_no_flag() {
        // Preset::No is final regardless of TTY state.
        assert_eq!(
            decide_adopt(false, false, true, AdoptPreset::No),
            AdoptDecision::No
        );
        assert_eq!(
            decide_adopt(false, false, false, AdoptPreset::No),
            AdoptDecision::No
        );
    }

    #[test]
    fn decide_adopt_preset_prompt_tty_asks() {
        // Prompt preset + TTY → Ask (caller must run ask_adopt_user).
        assert_eq!(
            decide_adopt(false, false, true, AdoptPreset::Prompt),
            AdoptDecision::Ask
        );
    }

    #[test]
    fn decide_adopt_preset_prompt_no_tty_refuses() {
        // Prompt preset + non-TTY → No. Piped/CI invocations never hang.
        assert_eq!(
            decide_adopt(false, false, false, AdoptPreset::Prompt),
            AdoptDecision::No
        );
    }

    // --- resolve_adopt_flags (-y coercion) ---------------------------------

    #[test]
    fn resolve_adopt_flags_yes_coerces_when_neither_set() {
        // `-y` alone means "--adopt-target" for the adopt axis.
        let adopt = AdoptChoice {
            yes: true,
            ..AdoptChoice::default()
        };
        assert_eq!(resolve_adopt_flags(&adopt), (true, false));
    }

    #[test]
    fn resolve_adopt_flags_yes_is_noop_when_adopt_target_already_set() {
        // Explicit --adopt-target already covers the decision; `-y` is a
        // no-op on this axis (no announcement, no change).
        let adopt = AdoptChoice {
            adopt_target: true,
            yes: true,
            ..AdoptChoice::default()
        };
        assert_eq!(resolve_adopt_flags(&adopt), (true, false));
    }

    #[test]
    fn resolve_adopt_flags_yes_is_noop_when_no_adopt_target_set() {
        // Explicit --no-adopt-target beats `-y`: user asked to refuse, and
        // `-y` is only future-proofing for _prompts_, not an override of
        // explicit refusal.
        let adopt = AdoptChoice {
            no_adopt_target: true,
            yes: true,
            ..AdoptChoice::default()
        };
        assert_eq!(resolve_adopt_flags(&adopt), (false, true));
    }

    #[test]
    fn resolve_adopt_flags_passthrough_when_no_yes_flag() {
        // Without `-y`, resolve is a pure passthrough.
        let neither = AdoptChoice::default();
        assert_eq!(resolve_adopt_flags(&neither), (false, false));
        let only_adopt = AdoptChoice {
            adopt_target: true,
            ..AdoptChoice::default()
        };
        assert_eq!(resolve_adopt_flags(&only_adopt), (true, false));
        let only_no_adopt = AdoptChoice {
            no_adopt_target: true,
            ..AdoptChoice::default()
        };
        assert_eq!(resolve_adopt_flags(&only_no_adopt), (false, true));
    }

    // --- CleanupOptions (Slice 12) -----------------------------------------
    //
    // Most of the cleanup machinery — `classify_source` and `execute_cleanup`
    // — interacts with real git (`resolve_worktree_path`, `show_ref_exists`,
    // `worktree_remove`, `branch_delete`) and is verified end-to-end by the
    // YAML scenarios in `tests/manual/scenarios/merge/remove-source*.yml`.
    // The static shape is worth a dedicated assertion so changes to the
    // struct shape surface here and not only in integration.

    #[test]
    fn cleanup_options_default_is_noop() {
        let opts = CleanupOptions::default();
        assert!(!opts.remove_worktree);
        assert!(!opts.also_branch);
    }

    #[test]
    fn cleanup_options_construction_preserves_fields() {
        let opts = CleanupOptions {
            remove_worktree: true,
            also_branch: false,
            squash_committed: false,
        };
        assert!(opts.remove_worktree);
        assert!(!opts.also_branch);
        assert!(!opts.squash_committed);

        let opts_rb = CleanupOptions {
            remove_worktree: true,
            also_branch: true,
            squash_committed: false,
        };
        assert!(opts_rb.remove_worktree);
        assert!(opts_rb.also_branch);
        assert!(!opts_rb.squash_committed);
    }

    // ─────────────────────────────────────────────────────────────────
    // MergeHookContext — env-var construction for pre-merge / post-merge.
    // These tests lock the exact string values hook scripts will observe.
    // ─────────────────────────────────────────────────────────────────

    fn target_with_worktree(branch: &str, path: &str) -> ResolvedTarget {
        ResolvedTarget {
            branch: branch.into(),
            path: Some(PathBuf::from(path)),
        }
    }

    #[test]
    fn pre_context_octopus_mode() {
        let flags = EffectiveFlags::default();
        let target = target_with_worktree("main", "/p/main");
        let sources = vec!["feat-a".into(), "feat-b".into(), "feat-c".into()];
        let ctx = MergeHookContext::for_pre(&sources, &target, &flags, false, false);
        assert_eq!(
            ctx.env.get("DAFT_MERGE_MODE").map(String::as_str),
            Some("octopus")
        );
        assert_eq!(
            ctx.env.get("DAFT_MERGE_SOURCES").map(String::as_str),
            Some("feat-a feat-b feat-c")
        );
        assert_eq!(
            ctx.env.get("DAFT_MERGE_TARGET_BRANCH").map(String::as_str),
            Some("main")
        );
        assert_eq!(
            ctx.env.get("DAFT_MERGE_TARGET_PATH").map(String::as_str),
            Some("/p/main")
        );
    }

    #[test]
    fn pre_context_squash_mode() {
        let flags = EffectiveFlags {
            squash: Some(true),
            ..EffectiveFlags::default()
        };
        let target = target_with_worktree("main", "/p/main");
        let ctx = MergeHookContext::for_pre(&["feat".into()], &target, &flags, false, false);
        assert_eq!(
            ctx.env.get("DAFT_MERGE_MODE").map(String::as_str),
            Some("squash")
        );
    }

    #[test]
    fn pre_context_ff_mode() {
        let flags = EffectiveFlags {
            ff: Some(FfMode::Only),
            ..EffectiveFlags::default()
        };
        let target = target_with_worktree("main", "/p/main");
        let ctx = MergeHookContext::for_pre(&["feat".into()], &target, &flags, false, false);
        assert_eq!(
            ctx.env.get("DAFT_MERGE_MODE").map(String::as_str),
            Some("ff")
        );
    }

    #[test]
    fn pre_context_merge_mode_default() {
        // Single source, no squash, no ff-only → plain "merge".
        let flags = EffectiveFlags::default();
        let target = target_with_worktree("main", "/p/main");
        let ctx = MergeHookContext::for_pre(&["feat".into()], &target, &flags, false, false);
        assert_eq!(
            ctx.env.get("DAFT_MERGE_MODE").map(String::as_str),
            Some("merge")
        );
    }

    #[test]
    fn pre_context_cross_worktree_true() {
        let flags = EffectiveFlags::default();
        let target = target_with_worktree("main", "/p/main");
        let ctx = MergeHookContext::for_pre(&["feat".into()], &target, &flags, false, true);
        assert_eq!(
            ctx.env.get("DAFT_MERGE_CROSS_WORKTREE").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            ctx.env.get("DAFT_MERGE_EPHEMERAL").map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn pre_context_ephemeral_true() {
        let flags = EffectiveFlags::default();
        let target = ResolvedTarget {
            branch: "main".into(),
            path: None,
        };
        let ctx = MergeHookContext::for_pre(&["feat".into()], &target, &flags, true, false);
        assert_eq!(
            ctx.env.get("DAFT_MERGE_EPHEMERAL").map(String::as_str),
            Some("true")
        );
        // Ref-only target → target path is empty string (consistent; scripts
        // can test `[ -z "$DAFT_MERGE_TARGET_PATH" ]`).
        assert_eq!(
            ctx.env.get("DAFT_MERGE_TARGET_PATH").map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn pre_context_strategy_passthrough() {
        let flags = EffectiveFlags {
            strategy: Some("ours".into()),
            ..EffectiveFlags::default()
        };
        let target = target_with_worktree("main", "/p/main");
        let ctx = MergeHookContext::for_pre(&["feat".into()], &target, &flags, false, false);
        assert_eq!(
            ctx.env.get("DAFT_MERGE_STRATEGY").map(String::as_str),
            Some("ours")
        );
    }

    #[test]
    fn post_context_success_sets_sha() {
        let flags = EffectiveFlags::default();
        let target = target_with_worktree("main", "/p/main");
        let pre = MergeHookContext::for_pre(&["feat".into()], &target, &flags, false, false);
        let post = pre.extend_for_post(PostOutcome::Success {
            commit_sha: "deadbeef1234".into(),
        });
        assert_eq!(
            post.env.get("DAFT_MERGE_RESULT").map(String::as_str),
            Some("success")
        );
        assert_eq!(
            post.env.get("DAFT_MERGE_COMMIT_SHA").map(String::as_str),
            Some("deadbeef1234")
        );
        assert_eq!(
            post.env
                .get("DAFT_MERGE_CONFLICTED_FILES")
                .map(String::as_str),
            Some("")
        );
        assert_eq!(
            post.env
                .get("DAFT_MERGE_PROMOTED_FROM_EPHEMERAL")
                .map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn post_context_conflict_sets_files_and_promoted() {
        let flags = EffectiveFlags::default();
        let target = target_with_worktree("main", "/p/main");
        let pre = MergeHookContext::for_pre(&["feat".into()], &target, &flags, true, false);
        let post = pre.extend_for_post(PostOutcome::Conflict {
            files: vec!["a.txt".into(), "b.txt".into()],
            promoted_from_ephemeral: true,
        });
        assert_eq!(
            post.env.get("DAFT_MERGE_RESULT").map(String::as_str),
            Some("conflict")
        );
        assert_eq!(
            post.env.get("DAFT_MERGE_COMMIT_SHA").map(String::as_str),
            Some("")
        );
        // Files are newline-joined so scripts can iterate with `while read`.
        assert_eq!(
            post.env
                .get("DAFT_MERGE_CONFLICTED_FILES")
                .map(String::as_str),
            Some("a.txt\nb.txt")
        );
        assert_eq!(
            post.env
                .get("DAFT_MERGE_PROMOTED_FROM_EPHEMERAL")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn post_context_already_up_to_date() {
        let flags = EffectiveFlags::default();
        let target = target_with_worktree("main", "/p/main");
        let pre = MergeHookContext::for_pre(&["feat".into()], &target, &flags, false, false);
        let post = pre.extend_for_post(PostOutcome::AlreadyUpToDate);
        assert_eq!(
            post.env.get("DAFT_MERGE_RESULT").map(String::as_str),
            Some("already-up-to-date")
        );
        assert_eq!(
            post.env.get("DAFT_MERGE_COMMIT_SHA").map(String::as_str),
            Some("")
        );
        assert_eq!(
            post.env
                .get("DAFT_MERGE_CONFLICTED_FILES")
                .map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn extend_for_post_aborted_sets_result_aborted_and_empty_sha() {
        let flags = EffectiveFlags::default();
        let target = target_with_worktree("main", "/p/main");
        let pre = MergeHookContext::for_pre(&["feat".into()], &target, &flags, false, false);
        let post = pre.extend_for_post(PostOutcome::Aborted);
        assert_eq!(
            post.env.get("DAFT_MERGE_RESULT").map(String::as_str),
            Some("aborted")
        );
        assert_eq!(
            post.env.get("DAFT_MERGE_COMMIT_SHA").map(String::as_str),
            Some("")
        );
        assert_eq!(
            post.env
                .get("DAFT_MERGE_CONFLICTED_FILES")
                .map(String::as_str),
            Some("")
        );
        assert_eq!(
            post.env
                .get("DAFT_MERGE_PROMOTED_FROM_EPHEMERAL")
                .map(String::as_str),
            Some("false")
        );
    }

    // ── squash_would_open_editor ──────────────────────────────────────────

    #[test]
    fn squash_would_open_editor_true_when_no_message_flags() {
        let flags = EffectiveFlags {
            squash: Some(true),
            ..EffectiveFlags::default()
        };
        assert!(flags.squash_would_open_editor());
    }

    #[test]
    fn squash_would_open_editor_false_when_no_squash() {
        let flags = EffectiveFlags::default(); // squash is None
        assert!(!flags.squash_would_open_editor());
    }

    #[test]
    fn squash_would_open_editor_false_when_no_commit() {
        let flags = EffectiveFlags {
            squash: Some(true),
            commit: Some(false), // --no-commit
            ..EffectiveFlags::default()
        };
        assert!(!flags.squash_would_open_editor());
    }

    #[test]
    fn squash_would_open_editor_false_when_message_set() {
        let flags = EffectiveFlags {
            squash: Some(true),
            message: Some("my message".into()),
            ..EffectiveFlags::default()
        };
        assert!(!flags.squash_would_open_editor());
    }

    #[test]
    fn squash_would_open_editor_false_when_no_edit() {
        let flags = EffectiveFlags {
            squash: Some(true),
            edit: Some(false), // --no-edit
            ..EffectiveFlags::default()
        };
        assert!(!flags.squash_would_open_editor());
    }

    #[test]
    fn squash_would_open_editor_false_when_file_set() {
        let flags = EffectiveFlags {
            squash: Some(true),
            file: Some(std::path::PathBuf::from("/tmp/msg.txt")),
            ..EffectiveFlags::default()
        };
        assert!(!flags.squash_would_open_editor());
    }

    // ── render_commit_flags ──────────────────────────────────────────────────

    #[test]
    fn render_commit_flags_empty_for_default_flags() {
        let flags = EffectiveFlags::default();
        assert!(render_commit_flags(&flags).is_empty());
    }

    #[test]
    fn render_commit_flags_message() {
        let flags = EffectiveFlags {
            message: Some("the commit message".into()),
            ..EffectiveFlags::default()
        };
        assert_eq!(
            render_commit_flags(&flags),
            vec!["-m", "the commit message"]
        );
    }

    #[test]
    fn render_commit_flags_file() {
        let flags = EffectiveFlags {
            file: Some(PathBuf::from("/tmp/commit-msg.txt")),
            ..EffectiveFlags::default()
        };
        assert_eq!(
            render_commit_flags(&flags),
            vec!["-F", "/tmp/commit-msg.txt"]
        );
    }

    #[test]
    fn render_commit_flags_no_edit() {
        let flags = EffectiveFlags {
            edit: Some(false),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_commit_flags(&flags), vec!["--no-edit"]);
    }

    #[test]
    fn render_commit_flags_edit() {
        let flags = EffectiveFlags {
            edit: Some(true),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_commit_flags(&flags), vec!["--edit"]);
    }

    #[test]
    fn render_commit_flags_signoff() {
        let flags = EffectiveFlags {
            signoff: Some(true),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_commit_flags(&flags), vec!["--signoff"]);
    }

    #[test]
    fn render_commit_flags_gpg_default() {
        let flags = EffectiveFlags {
            gpg_sign: Some(GpgSign::Default),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_commit_flags(&flags), vec!["-S"]);
    }

    #[test]
    fn render_commit_flags_gpg_keyid() {
        let flags = EffectiveFlags {
            gpg_sign: Some(GpgSign::KeyId("DEADBEEF".into())),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_commit_flags(&flags), vec!["-SDEADBEEF"]);
    }

    #[test]
    fn render_commit_flags_gpg_disabled() {
        let flags = EffectiveFlags {
            gpg_sign: Some(GpgSign::Disabled),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_commit_flags(&flags), vec!["--no-gpg-sign"]);
    }

    #[test]
    fn render_commit_flags_cleanup() {
        let flags = EffectiveFlags {
            cleanup: Some("verbatim".into()),
            ..EffectiveFlags::default()
        };
        assert_eq!(render_commit_flags(&flags), vec!["--cleanup", "verbatim"]);
    }

    #[test]
    fn render_commit_flags_combination() {
        // --cleanup appears after --edit and before --signoff, mirroring the
        // order in render_flags.
        let flags = EffectiveFlags {
            message: Some("squash msg".into()),
            cleanup: Some("strip".into()),
            signoff: Some(true),
            gpg_sign: Some(GpgSign::Default),
            ..EffectiveFlags::default()
        };
        assert_eq!(
            render_commit_flags(&flags),
            vec!["-m", "squash msg", "--cleanup", "strip", "--signoff", "-S"]
        );
    }

    // ── render_flags squash-aware: message-flags stripped when squash=true ──

    #[test]
    fn render_flags_squash_excludes_message_flag() {
        let flags = EffectiveFlags {
            squash: Some(true),
            message: Some("my msg".into()),
            ..EffectiveFlags::default()
        };
        let rendered = render_flags(&flags);
        // --squash present, -m NOT present (moved to render_commit_flags)
        assert!(rendered.contains(&"--squash".to_string()));
        assert!(!rendered.contains(&"-m".to_string()));
    }

    #[test]
    fn render_flags_squash_excludes_file_flag() {
        let flags = EffectiveFlags {
            squash: Some(true),
            file: Some(PathBuf::from("/tmp/f.txt")),
            ..EffectiveFlags::default()
        };
        let rendered = render_flags(&flags);
        assert!(rendered.contains(&"--squash".to_string()));
        assert!(!rendered.contains(&"-F".to_string()));
    }

    #[test]
    fn render_flags_squash_excludes_edit_flag() {
        let flags = EffectiveFlags {
            squash: Some(true),
            edit: Some(false),
            ..EffectiveFlags::default()
        };
        let rendered = render_flags(&flags);
        assert!(rendered.contains(&"--squash".to_string()));
        assert!(!rendered.contains(&"--no-edit".to_string()));
    }

    #[test]
    fn render_flags_squash_excludes_signoff() {
        let flags = EffectiveFlags {
            squash: Some(true),
            signoff: Some(true),
            ..EffectiveFlags::default()
        };
        let rendered = render_flags(&flags);
        assert!(rendered.contains(&"--squash".to_string()));
        assert!(!rendered.contains(&"--signoff".to_string()));
    }

    #[test]
    fn render_flags_squash_excludes_gpg_sign() {
        let flags = EffectiveFlags {
            squash: Some(true),
            gpg_sign: Some(GpgSign::Default),
            ..EffectiveFlags::default()
        };
        let rendered = render_flags(&flags);
        assert!(rendered.contains(&"--squash".to_string()));
        assert!(!rendered.contains(&"-S".to_string()));
    }

    #[test]
    fn render_flags_non_squash_keeps_message_flag() {
        let flags = EffectiveFlags {
            message: Some("keep me".into()),
            ..EffectiveFlags::default()
        };
        let rendered = render_flags(&flags);
        assert!(rendered.contains(&"-m".to_string()));
        assert!(rendered.contains(&"keep me".to_string()));
    }

    // ── Task 3.1: source SHA capture ─────────────────────────────────────────

    /// Helper: create a branch at `path` starting from the initial commit.
    fn create_branch(path: &Path, branch: &str) {
        ShellCommand::new("git")
            .args(["checkout", "-q", "-b", branch])
            .current_dir(path)
            .status()
            .unwrap();
        // Add a commit so the branch tip is distinct from main.
        ShellCommand::new("git")
            .args(["commit", "--allow-empty", "-q", "-m", branch])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap();
        // Return to main.
        ShellCommand::new("git")
            .args(["checkout", "-q", "main"])
            .current_dir(path)
            .status()
            .unwrap();
    }

    #[test]
    #[serial]
    fn capture_source_shas_returns_sha_for_known_branch() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        create_branch(tmp.path(), "feat-a");

        let git = GitCommand::new(false);
        // Set cwd so GitCommand runs in the right repo.
        std::env::set_current_dir(tmp.path()).unwrap();

        let sha = capture_source_shas(&["feat-a".to_string()], &git).unwrap();
        assert_eq!(sha.len(), 1);
        // SHA should be a 40-char hex string.
        assert_eq!(sha[0].len(), 40, "expected a full SHA, got '{}'", sha[0]);
    }

    #[test]
    #[serial]
    fn capture_source_shas_errors_on_unknown_ref() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        let git = GitCommand::new(false);
        std::env::set_current_dir(tmp.path()).unwrap();

        let result = capture_source_shas(&["no-such-branch".to_string()], &git);
        assert!(result.is_err(), "expected error for unknown ref");
    }

    #[test]
    fn pre_context_includes_source_shas() {
        let flags = EffectiveFlags::default();
        let target = target_with_worktree("main", "/p/main");
        let shas = vec!["abc123".to_string(), "def456".to_string()];
        let ctx = MergeHookContext::for_pre_with_shas(
            &["feat-a".into(), "feat-b".into()],
            &target,
            &flags,
            false,
            false,
            &shas,
        );
        assert_eq!(
            ctx.env.get("DAFT_MERGE_SOURCE_SHAS").map(String::as_str),
            Some("abc123\ndef456")
        );
    }

    // ── Task 3.2: transactional cleanup ──────────────────────────────────────

    /// Set up a repo at `root` with a second commit on `main`, then add a
    /// linked worktree at `wt_path` on a new branch `branch`. The worktree
    /// starts clean (no uncommitted changes).
    fn setup_worktree(root: &Path, branch: &str, wt_path: &Path) {
        ShellCommand::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                &wt_path.display().to_string(),
                "-b",
                branch,
            ])
            .current_dir(root)
            .status()
            .unwrap();
    }

    #[test]
    #[serial]
    fn execute_cleanup_refuses_dirty_source_worktree() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        // Add a second commit on main so feature branch has a parent distinct from
        // the initial commit (needed so cleanup can check merge-base properly).
        ShellCommand::new("git")
            .args(["commit", "--allow-empty", "-q", "-m", "second"])
            .current_dir(tmp.path())
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap();

        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature/test", &feat_wt);

        // Make the source worktree dirty.
        std::fs::write(feat_wt.join("dirty.txt"), b"dirty").unwrap();
        ShellCommand::new("git")
            .args(["add", "dirty.txt"])
            .current_dir(&feat_wt)
            .status()
            .unwrap();

        let git = GitCommand::new(false);
        std::env::set_current_dir(tmp.path()).unwrap();

        let opts = CleanupOptions {
            remove_worktree: true,
            also_branch: true,
            squash_committed: false,
        };
        let result = execute_cleanup(
            &["feature/test".to_string()],
            &opts,
            &git,
            tmp.path(),
            "main",
        );
        assert!(result.is_err(), "expected error for dirty source worktree");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("uncommitted changes"),
            "expected dirty-worktree message, got: {msg}"
        );
        // Worktree must NOT have been removed.
        assert!(
            feat_wt.exists(),
            "worktree should still exist after validation failure"
        );
    }

    #[test]
    #[serial]
    fn execute_cleanup_refuses_unmerged_branch_when_not_squash() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature/test", &feat_wt);

        // Add a commit on the feature branch so it's ahead of main (unmerged).
        ShellCommand::new("git")
            .args(["commit", "--allow-empty", "-q", "-m", "feature work"])
            .current_dir(&feat_wt)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap();

        let git = GitCommand::new(false);
        std::env::set_current_dir(tmp.path()).unwrap();

        let opts = CleanupOptions {
            remove_worktree: true,
            also_branch: true,
            squash_committed: false,
        };
        let result = execute_cleanup(
            &["feature/test".to_string()],
            &opts,
            &git,
            tmp.path(),
            "main",
        );
        assert!(result.is_err(), "expected error for unmerged branch");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not fully merged")
                || msg.contains("unmerged")
                || msg.contains("not reachable"),
            "expected unmerged-branch message, got: {msg}"
        );
        // Worktree must NOT have been removed.
        assert!(
            feat_wt.exists(),
            "worktree should still exist after validation failure"
        );
    }

    #[test]
    #[serial]
    fn execute_cleanup_succeeds_when_all_validates_pass() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature/test", &feat_wt);

        // Merge the feature branch into main so branch -d would succeed.
        ShellCommand::new("git")
            .args(["merge", "--no-ff", "--no-edit", "feature/test"])
            .current_dir(tmp.path())
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap();

        let git = GitCommand::new(false);
        std::env::set_current_dir(tmp.path()).unwrap();

        let opts = CleanupOptions {
            remove_worktree: true,
            also_branch: true,
            squash_committed: false,
        };
        let result = execute_cleanup(
            &["feature/test".to_string()],
            &opts,
            &git,
            tmp.path(),
            "main",
        );
        assert!(
            result.is_ok(),
            "expected cleanup to succeed, got: {result:?}"
        );
        // Worktree should be gone.
        assert!(!feat_wt.exists(), "worktree should have been removed");
        // Branch should be gone.
        let branch_exists = ShellCommand::new("git")
            .args(["show-ref", "--verify", "--quiet", "refs/heads/feature/test"])
            .current_dir(tmp.path())
            .status()
            .unwrap()
            .success();
        assert!(!branch_exists, "branch should have been deleted");
    }

    #[test]
    fn merge_intent_marker_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();

        let intent = MergeIntent {
            sources: vec!["feat/x".to_string()],
            source_shas: vec!["abc123def456".to_string()],
            remove_worktree: true,
            also_branch: true,
        };

        write_intent_marker(&git_dir, &intent);

        let marker_path = git_dir.join("daft-merge-intent.json");
        let read = read_intent_marker(&marker_path).expect("marker should be readable after write");

        assert_eq!(read.sources, intent.sources);
        assert_eq!(read.source_shas, intent.source_shas);
        assert_eq!(read.remove_worktree, intent.remove_worktree);
        assert_eq!(read.also_branch, intent.also_branch);
    }

    // ── plan_cleanup tests ────────────────────────────────────────────────────

    #[test]
    #[serial]
    fn plan_cleanup_returns_item_for_branch_with_worktree() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature/test", &feat_wt);
        std::env::set_current_dir(tmp.path()).unwrap();

        let git = GitCommand::new(false);
        let opts = CleanupOptions {
            remove_worktree: true,
            also_branch: true,
            squash_committed: false,
        };
        let plan = plan_cleanup(
            &["feature/test".to_string()],
            &opts,
            &git,
            tmp.path(),
            "main",
        )
        .expect("plan_cleanup should succeed when branch is mergeable");
        assert_eq!(plan.len(), 1);
        let item = &plan[0];
        assert_eq!(item.source, "feature/test");
        assert!(item.worktree_path.is_some());
        assert_eq!(item.branch_name.as_deref(), Some("feature/test"));
        assert!(!item.force_delete);
    }

    #[test]
    #[serial]
    fn plan_cleanup_drops_commit_or_other_silently() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature/test", &feat_wt);
        std::env::set_current_dir(tmp.path()).unwrap();

        let git = GitCommand::new(false);
        let opts = CleanupOptions {
            remove_worktree: true,
            also_branch: true,
            squash_committed: false,
        };
        // A commit SHA / non-branch arg → no items planned.
        let plan = plan_cleanup(&["HEAD~0".to_string()], &opts, &git, tmp.path(), "main")
            .expect("plan_cleanup should succeed for non-branch source");
        assert!(plan.is_empty());
    }

    #[test]
    #[serial]
    fn plan_cleanup_force_delete_set_when_squash_committed() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature/test", &feat_wt);
        std::env::set_current_dir(tmp.path()).unwrap();

        let git = GitCommand::new(false);
        let opts = CleanupOptions {
            remove_worktree: true,
            also_branch: true,
            squash_committed: true,
        };
        let plan = plan_cleanup(
            &["feature/test".to_string()],
            &opts,
            &git,
            tmp.path(),
            "main",
        )
        .expect("squash-committed plan should bypass reachability check");
        assert_eq!(plan.len(), 1);
        assert!(plan[0].force_delete);
    }

    #[test]
    #[serial]
    fn plan_cleanup_remove_only_no_branch() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature/test", &feat_wt);
        std::env::set_current_dir(tmp.path()).unwrap();

        let git = GitCommand::new(false);
        let opts = CleanupOptions {
            remove_worktree: true,
            also_branch: false,
            squash_committed: false,
        };
        let plan = plan_cleanup(
            &["feature/test".to_string()],
            &opts,
            &git,
            tmp.path(),
            "main",
        )
        .expect("remove-only plan should succeed regardless of merge state");
        assert_eq!(plan.len(), 1);
        assert!(plan[0].worktree_path.is_some());
        assert!(plan[0].branch_name.is_none());
    }

    #[test]
    #[serial]
    fn plan_cleanup_validation_errors_short_circuit() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature/test", &feat_wt);
        // Make the feature worktree dirty.
        std::fs::write(feat_wt.join("uncommitted.txt"), "dirty\n").unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let git = GitCommand::new(false);
        let opts = CleanupOptions {
            remove_worktree: true,
            also_branch: true,
            squash_committed: false,
        };
        let err = plan_cleanup(
            &["feature/test".to_string()],
            &opts,
            &git,
            tmp.path(),
            "main",
        )
        .expect_err("dirty source must fail pre-validation");
        let msg = err.to_string();
        assert!(msg.contains("cleanup pre-validation failed"));
        assert!(msg.contains("uncommitted changes"));
    }

    // ── Slice 3: captured_git_output / was_fast_forward / merge_commit_sha ────

    #[test]
    #[serial]
    fn execute_start_populates_captured_git_output_on_success() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);
        // Add a commit on feature so we have something to merge.
        ShellCommand::new("git")
            .args(["commit", "--allow-empty", "-q", "-m", "feature work"])
            .current_dir(&feat_wt)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let git = GitCommand::new(false);
        let mut runner = NullHookRunner;
        let params = StartParams {
            sources: vec!["feature".to_string()],
            target: None,
            flags: EffectiveFlags::default(),
            adopt: AdoptChoice::default(),
            require_clean_target: false,
            cleanup_intent: None,
        };
        let outcome =
            execute_start(&params, &git, tmp.path(), &mut runner).expect("merge succeeds");

        // Captured buffer should be non-empty (git produces "Updating ...",
        // "Fast-forward", etc. even on success).
        assert!(
            !outcome.captured_git_output.is_empty(),
            "captured_git_output must be populated on FF success path"
        );
        // Single-commit feature branch → FF.
        assert!(
            outcome.was_fast_forward,
            "single-commit feature branch should fast-forward"
        );
        // Commit SHA recorded.
        assert!(
            outcome.merge_commit_sha.is_some(),
            "merge_commit_sha must be Some after FF"
        );
    }
}
