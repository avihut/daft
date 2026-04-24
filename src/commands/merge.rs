//! git-worktree-merge - Merge branches across worktrees
//!
//! Mirrors git merge semantics when --into is omitted; enables
//! cross-worktree merges (merge <source>... into <target> from any
//! worktree) when --into is supplied. Finish commands (--abort,
//! --continue, --quit) take an optional positional <worktree|branch>
//! argument, default to CWD.

use crate::{
    core::worktree::merge::{HookRunner, MergeHookContext},
    executor::cli_presenter::CliPresenter,
    get_current_worktree_path, get_git_common_dir, get_project_root,
    git::GitCommand,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::{DaftSettings, HookOutputConfig},
};
use anyhow::Result;
use clap::Parser;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "git-worktree-merge")]
#[command(version = crate::VERSION)]
#[command(about = "Merge branches across worktrees")]
#[command(long_about = r#"
Merges one or more source branches into a target worktree's branch.

When --into is omitted, the target is the current worktree's branch,
mirroring `git merge`. When --into <target> is supplied, the merge is
performed against that worktree's branch from wherever you are.

Multiple sources invoke git's octopus strategy, announced explicitly.

Finish commands (--abort, --continue, --quit) take an optional positional
<worktree|branch>; default to the current worktree's branch.
"#)]
pub struct Args {
    /// Source branches/commits to merge (start mode), OR optional target worktree/branch
    /// for --abort / --continue / --quit (finish mode; max one positional).
    #[arg(value_name = "SOURCE_OR_TARGET", num_args = 0..)]
    pub sources: Vec<String>,

    /// Target worktree/branch. Defaults to the current worktree's branch.
    #[arg(long = "into", value_name = "TARGET")]
    pub into: Option<String>,

    // --- Finish mode flags (mutually exclusive, and mutually exclusive with
    // every start-only flag so that `daft merge --abort -m msg` etc. errors
    // at parse time instead of silently ignoring the start-mode flag).
    // `into` is NOT included: it is accepted in finish mode as a fallback
    // target when no positional is given (see dispatch in `run()`).
    /// Abort an in-progress merge in the named worktree (defaults to CWD).
    #[arg(
        long = "abort",
        conflicts_with_all = [
            "continue_merge", "quit",
            "message", "file", "edit", "no_edit", "cleanup",
            "ff", "no_ff", "ff_only",
            "squash", "no_squash",
            "commit", "no_commit",
            "signoff", "no_signoff",
            "strategy", "strategy_options",
            "gpg_sign", "no_gpg_sign",
            "verify_signatures", "no_verify_signatures",
            "allow_unrelated_histories",
            "stat", "no_stat",
            "adopt_target", "no_adopt_target", "yes",
            "remove", "and_branch",
        ],
    )]
    pub abort: bool,

    /// Continue an in-progress merge in the named worktree (defaults to CWD).
    #[arg(
        long = "continue",
        conflicts_with_all = [
            "abort", "quit",
            "message", "file", "edit", "no_edit", "cleanup",
            "ff", "no_ff", "ff_only",
            "squash", "no_squash",
            "commit", "no_commit",
            "signoff", "no_signoff",
            "strategy", "strategy_options",
            "gpg_sign", "no_gpg_sign",
            "verify_signatures", "no_verify_signatures",
            "allow_unrelated_histories",
            "stat", "no_stat",
            "adopt_target", "no_adopt_target", "yes",
            "remove", "and_branch",
        ],
    )]
    pub continue_merge: bool,

    /// Quit an in-progress merge without resetting the index (defaults to CWD).
    #[arg(
        long = "quit",
        conflicts_with_all = [
            "abort", "continue_merge",
            "message", "file", "edit", "no_edit", "cleanup",
            "ff", "no_ff", "ff_only",
            "squash", "no_squash",
            "commit", "no_commit",
            "signoff", "no_signoff",
            "strategy", "strategy_options",
            "gpg_sign", "no_gpg_sign",
            "verify_signatures", "no_verify_signatures",
            "allow_unrelated_histories",
            "stat", "no_stat",
            "adopt_target", "no_adopt_target", "yes",
            "remove", "and_branch",
        ],
    )]
    pub quit: bool,

    // --- Commit message and editor ---
    /// Commit message for the merge commit (mirrors `git merge -m`).
    #[arg(short = 'm', value_name = "MSG")]
    pub message: Option<String>,
    /// Read the commit message from FILE (mirrors `git merge -F`).
    #[arg(short = 'F', long = "file", value_name = "FILE")]
    pub file: Option<std::path::PathBuf>,
    /// Launch the editor to edit the merge commit message.
    #[arg(long = "edit", conflicts_with = "no_edit")]
    pub edit: bool,
    /// Accept the auto-generated merge commit message without editing.
    #[arg(long = "no-edit", conflicts_with = "edit")]
    pub no_edit: bool,
    /// Message cleanup mode (mirrors `git merge --cleanup`).
    #[arg(long = "cleanup", value_name = "MODE")]
    pub cleanup: Option<String>,

    // --- Fast-forward control ---
    /// Allow fast-forward merges (git's default behavior).
    #[arg(long = "ff", conflicts_with_all = ["no_ff", "ff_only"])]
    pub ff: bool,
    /// Always create a merge commit, even when fast-forward is possible.
    #[arg(long = "no-ff", conflicts_with_all = ["ff", "ff_only"])]
    pub no_ff: bool,
    /// Refuse to merge if fast-forward is not possible.
    #[arg(long = "ff-only", conflicts_with_all = ["ff", "no_ff"])]
    pub ff_only: bool,

    // --- Squash ---
    /// Squash the source's changes into a single staged diff, without creating a merge commit.
    #[arg(long = "squash", conflicts_with = "no_squash")]
    pub squash: bool,
    /// Explicitly disable squash (cancel a config default of `merge.squash`).
    #[arg(long = "no-squash", conflicts_with = "squash")]
    pub no_squash: bool,

    // --- Commit control ---
    /// Automatically create the merge commit after a successful merge.
    #[arg(long = "commit", conflicts_with = "no_commit")]
    pub commit: bool,
    /// Leave the merge staged without committing.
    #[arg(long = "no-commit", conflicts_with = "commit")]
    pub no_commit: bool,

    // --- Signoff ---
    /// Add a Signed-off-by trailer to the merge commit.
    #[arg(long = "signoff", conflicts_with = "no_signoff")]
    pub signoff: bool,
    /// Explicitly disable signoff (cancel a config default).
    #[arg(long = "no-signoff", conflicts_with = "signoff")]
    pub no_signoff: bool,

    // --- Strategy ---
    /// Merge strategy to use (e.g. `ours`, `recursive`, `octopus`).
    #[arg(short = 's', long = "strategy", value_name = "STRAT")]
    pub strategy: Option<String>,
    /// Strategy-specific option (repeatable; mirrors `git merge -X`).
    #[arg(short = 'X', long = "strategy-option", value_name = "OPT")]
    pub strategy_options: Vec<String>,

    // --- GPG signing ---
    /// GPG-sign the merge commit. Accepts an optional KEYID; omit to use the default key.
    #[arg(
        short = 'S',
        long = "gpg-sign",
        value_name = "KEYID",
        num_args = 0..=1,
        default_missing_value = "",
    )]
    pub gpg_sign: Option<String>,
    /// Do not GPG-sign the merge commit (cancels `commit.gpgsign` config).
    #[arg(long = "no-gpg-sign", conflicts_with = "gpg_sign")]
    pub no_gpg_sign: bool,

    // --- Signature verification ---
    /// Verify that the tip commit of the source is signed with a valid key.
    #[arg(long = "verify-signatures", conflicts_with = "no_verify_signatures")]
    pub verify_signatures: bool,
    /// Do not verify signatures on the source tip commit.
    #[arg(long = "no-verify-signatures", conflicts_with = "verify_signatures")]
    pub no_verify_signatures: bool,

    // --- History ---
    /// Allow merging histories that share no common ancestor.
    #[arg(long = "allow-unrelated-histories")]
    pub allow_unrelated_histories: bool,

    // --- Diffstat ---
    /// Show a diffstat at the end of the merge.
    #[arg(long = "stat", conflicts_with = "no_stat")]
    pub stat: bool,
    /// Suppress the diffstat at the end of the merge.
    #[arg(short = 'n', long = "no-stat", conflicts_with = "stat")]
    pub no_stat: bool,

    // --- Adopt target (ephemeral worktree for ref-only non-FF merges) ---
    /// When the target has no worktree and the merge is not a pure fast-forward,
    /// create an ephemeral worktree to perform the merge without prompting.
    #[arg(long = "adopt-target", conflicts_with = "no_adopt_target")]
    pub adopt_target: bool,

    /// When the target has no worktree and the merge is not a pure fast-forward,
    /// refuse without prompting.
    #[arg(long = "no-adopt-target", conflicts_with = "adopt_target")]
    pub no_adopt_target: bool,

    /// Auto-accept interactive prompts. Implies --adopt-target when neither
    /// --adopt-target nor --no-adopt-target is supplied. Future-proofs any
    /// new prompts we add.
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,

    // --- Post-merge cleanup (start-mode only) ---
    /// Remove the source worktree after a successful merge.
    #[arg(short = 'r', long = "remove")]
    pub remove: bool,

    /// Also delete the source branch (requires --remove). Uses `git branch -d`
    /// semantics; refuses to delete if the branch is not fully merged.
    #[arg(short = 'b', long = "and-branch", requires = "remove")]
    pub and_branch: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    pub verbose: bool,
}

/// Translate parsed CLI [`Args`] + [`DaftSettings`] into [`EffectiveFlags`].
///
/// Clap's `conflicts_with`/`conflicts_with_all` attrs guarantee at parse time
/// that each paired bool (e.g. `edit`/`no_edit`) has at most one side true,
/// so `else if` chains below are exhaustive in practice.
///
/// Precedence: CLI flags > `daft.merge.*` config > built-in defaults. A
/// paired negation flag (e.g. `--no-ff`, `--no-signoff`) always wins over the
/// config-provided default; that's why each chain checks both the positive
/// and negative CLI flag before consulting settings.
///
/// `-S` has dual semantics in git: bare `-S` means "use the default key",
/// and `-S<KEYID>` binds a specific key. Clap exposes this via
/// `num_args = 0..=1` + `default_missing_value = ""`: an empty string means
/// "passed with no value", a non-empty string means a key ID. The same
/// convention is mirrored in `daft.merge.gpgSign` (`true` → default key,
/// `false` → unset, anything else → KEYID).
fn effective_flags_from_args_and_settings(
    args: &Args,
    settings: &DaftSettings,
) -> crate::core::worktree::merge::EffectiveFlags {
    use crate::core::worktree::merge::{EffectiveFlags, FfMode, GpgSign};

    // ff: CLI wins; else always emit the setting's preference so git sees
    // a concrete flag. Emitting `Some(Auto)` even for the default is
    // deliberate: `render_flags` turns it into `--ff`, which is a no-op for
    // git but makes the invocation self-describing in verbose logs.
    let ff = if args.ff_only {
        Some(FfMode::Only)
    } else if args.no_ff {
        Some(FfMode::Never)
    } else if args.ff {
        Some(FfMode::Auto)
    } else {
        Some(settings.merge_ff)
    };

    // squash: CLI wins; else only emit when settings enables it (the default
    // `merge_squash = false` matches `None = git's default`, so stay `None`
    // to keep the argv minimal).
    let squash = if args.squash {
        Some(true)
    } else if args.no_squash {
        Some(false)
    } else if settings.merge_squash {
        Some(true)
    } else {
        None
    };

    // commit: CLI wins; else only emit when settings overrides to `false`
    // (`merge_commit = true` is git's default, so stay `None`). `--no-commit`
    // and a false config value collapse into the same `Some(false)` outcome —
    // there's no observable difference to git.
    let commit = if args.commit {
        Some(true)
    } else if args.no_commit || !settings.merge_commit {
        Some(false)
    } else {
        None
    };

    // edit: CLI wins; else settings provides `Option<bool>` directly (None
    // = let git decide from TTY).
    let edit = if args.edit {
        Some(true)
    } else if args.no_edit {
        Some(false)
    } else {
        settings.merge_edit
    };

    let signoff = if args.signoff {
        Some(true)
    } else if args.no_signoff {
        Some(false)
    } else if settings.merge_signoff {
        Some(true)
    } else {
        None
    };

    let gpg_sign = if args.no_gpg_sign {
        Some(GpgSign::Disabled)
    } else if let Some(k) = &args.gpg_sign {
        if k.is_empty() {
            Some(GpgSign::Default)
        } else {
            Some(GpgSign::KeyId(k.clone()))
        }
    } else if let Some(k) = &settings.merge_gpg_sign {
        if k.is_empty() {
            Some(GpgSign::Default)
        } else {
            Some(GpgSign::KeyId(k.clone()))
        }
    } else {
        None
    };

    let verify_signatures = if args.verify_signatures {
        Some(true)
    } else if args.no_verify_signatures {
        Some(false)
    } else if settings.merge_verify_signatures {
        Some(true)
    } else {
        None
    };

    // stat has no settings key (deliberately — it's a visual-output flag, not
    // a semantic default worth persisting). Preserve the original behavior.
    let stat = if args.stat {
        Some(true)
    } else if args.no_stat {
        Some(false)
    } else {
        None
    };

    // strategy: CLI wins over config; either may be `None`.
    let strategy = args
        .strategy
        .clone()
        .or_else(|| settings.merge_strategy.clone());

    // strategy_options accumulate: config first, then CLI appended. Duplicate
    // `-X` entries are harmless to git (last wins) and mirror the way
    // strategy options stack on the CLI itself.
    let mut strategy_options = settings.merge_strategy_options.clone();
    strategy_options.extend(args.strategy_options.iter().cloned());

    // allow_unrelated_histories: either side can enable; there is no
    // negating CLI flag, so CLI-false + config-true yields `true`.
    let allow_unrelated_histories =
        args.allow_unrelated_histories || settings.merge_allow_unrelated_histories;

    EffectiveFlags {
        message: args.message.clone(),
        file: args.file.clone(),
        edit,
        cleanup: args.cleanup.clone(),
        ff,
        squash,
        commit,
        signoff,
        strategy,
        strategy_options,
        gpg_sign,
        verify_signatures,
        allow_unrelated_histories,
        stat,
    }
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-merge"));
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    // Finish mode: --abort / --continue / --quit dispatch to execute_finish.
    // Clap's conflicts_with_all guarantees at most one of these is set.
    // In finish mode the positional `sources` is repurposed as an optional
    // target worktree/branch (max one positional). `--into` is also accepted
    // as a fallback target when no positional is given, since both the
    // positional and `--into` name a merge target — but not both at once.
    if args.abort || args.continue_merge || args.quit {
        if !args.sources.is_empty() && args.into.is_some() {
            anyhow::bail!("specify target via positional OR --into, not both");
        }
        let worktree_arg = match args.sources.as_slice() {
            [] => args.into.clone(),
            [one] => Some(one.clone()),
            _ => anyhow::bail!(
                "finish commands (--abort/--continue/--quit) take at most one positional <worktree|branch>"
            ),
        };
        let mode = if args.abort {
            crate::core::worktree::merge::FinishMode::Abort
        } else if args.continue_merge {
            crate::core::worktree::merge::FinishMode::Continue
        } else {
            crate::core::worktree::merge::FinishMode::Quit
        };
        let params = crate::core::worktree::merge::FinishParams {
            worktree: worktree_arg,
            mode,
        };
        return crate::core::worktree::merge::execute_finish(&params, &git, &project_root);
    }

    // Start mode. Clap's `num_args = 0..` on `sources` allows zero positionals
    // (needed for finish mode above); re-assert the start-mode minimum here.
    if args.sources.is_empty() {
        anyhow::bail!("specify at least one source to merge");
    }

    let flags = effective_flags_from_args_and_settings(&args, &settings);
    // Pass the adopt-related CLI flags through verbatim; clap enforces
    // `--adopt-target` vs `--no-adopt-target` mutual exclusion upstream, and
    // `-y`'s coercion to `--adopt-target` (and its announcement) happens in
    // `resolve_adopt_flags` inside the ref-only non-FF branch so the log
    // line fires exactly once, at the point the coercion matters. `preset`
    // comes from `daft.merge.adoptTargetOnDemand`.
    let adopt = crate::core::worktree::merge::AdoptChoice {
        adopt_target: args.adopt_target,
        no_adopt_target: args.no_adopt_target,
        yes: args.yes,
        preset: settings.merge_adopt_target_on_demand,
    };
    // Capture before move: needed for the worktree-post-create hook when the
    // ephemeral-promote path fires. Non-ephemeral paths don't use this.
    let into_branch = args.into.clone();
    let params = crate::core::worktree::merge::StartParams {
        sources: args.sources,
        target: args.into,
        flags,
        adopt,
        require_clean_target: settings.merge_require_clean_target,
    };
    // Build the MergeHookRunner used to fire merge-pre / merge-post hooks
    // from inside `execute_start`. Holding the executor + output here (in
    // the command layer) keeps core free of presenter/output dependencies.
    let git_dir = get_git_common_dir()?;
    let source_worktree = get_current_worktree_path().unwrap_or_else(|_| project_root.clone());
    let mut output = CliOutput::new(OutputConfig::new(false, args.verbose));
    let mut runner = MergeHookRunner::new(
        &mut output,
        project_root.clone(),
        git_dir,
        settings.remote.clone(),
        source_worktree,
    )?;
    let outcome =
        crate::core::worktree::merge::execute_start(&params, &git, &project_root, &mut runner)?;

    if outcome.already_up_to_date {
        // Core already printed "Already up to date." from the up-to-date
        // short-circuit (which also sets `emitted_terminal_message`). No
        // further print here — duplicating the status line would be noise.
        Ok(())
    } else if outcome.failed {
        // Ephemeral-promote path: a ref-only target was adopted into a
        // canonical worktree at its layout-resolved sibling path. Fire
        // `worktree-post-create` so hook-installed environment setup
        // (direnv/mise/etc.) is available while the user resolves conflicts.
        // Best-effort: a hook failure must not replace the conflict report.
        if outcome.ephemeral_promoted {
            if let Some(branch) = into_branch.as_deref() {
                if let Err(e) = fire_worktree_post_create_hook(
                    &outcome.target_path,
                    branch,
                    &project_root,
                    &settings,
                ) {
                    eprintln!("warning: worktree-post-create hook failed: {e}");
                }
            }
        }

        // Print a daft-authored conflict report to stderr and exit non-zero.
        // We bypass the usual `anyhow::bail!` plumbing because anyhow-printed
        // errors get the "Error:" prefix; for a multi-line report we want the
        // user to read verbatim, that prefix would be noise. `std::process::exit`
        // skips the rest of `main` — acceptable here because there's no further
        // cleanup to run: git left the worktree in a conflicted state that the
        // user now owns via --continue or --abort.
        eprintln!("merge conflicted in {}", outcome.target_path.display());
        if !outcome.conflicted_files.is_empty() {
            eprintln!("conflicted files:");
            for f in &outcome.conflicted_files {
                eprintln!("  {}", f);
            }
        }
        eprintln!();
        eprintln!("resolve in the target worktree, then run:");
        eprintln!("  daft merge --continue  # add <branch> if running from a different worktree");
        eprintln!("  daft merge --abort     # add <branch> if running from a different worktree");
        std::process::exit(1);
    } else {
        // Core may have already emitted a terminal status line (e.g.,
        // "Fast-forwarded X to Y (no worktree)" from the ref-only FF path).
        // Suppress the default "Merge complete." print in that case so a
        // single successful merge produces a single stdout line.
        if !outcome.emitted_terminal_message {
            println!("Merge complete.");
        }

        // Post-merge cleanup (Slice 12). Only runs on successful,
        // non-up-to-date merges — the `already_up_to_date` and `failed`
        // arms above have already returned or exited.
        //
        // Precedence: CLI flags can only *add* to the cleanup scope on top
        // of config defaults. Config-driven cleanup is intentionally
        // opt-in via `daft.merge.postMerge.removeSourceWorktree` and
        // `.alsoRemoveSourceBranch`. Clap already enforces that `-b`
        // requires `-r` on the CLI side — that's an interaction check for
        // user-typed flags, not for config defaults — so the code below
        // must also guard `and_branch` behind `effective_remove` or it
        // could request branch deletion without worktree removal when the
        // user sets `alsoRemoveSourceBranch = true` without enabling the
        // worktree-removal key. That's a config misconfiguration; gate it.
        //
        // When cleanup errors happen (e.g. `git branch -d` refusing an
        // unmerged branch after `--squash`), the merge itself succeeded —
        // the cleanup error is surfaced so the caller knows which
        // post-merge step failed, but any earlier successful cleanup step
        // (worktree removal) is not rolled back.
        let effective_remove = args.remove || settings.merge_post_merge_remove_source_worktree;
        let effective_and_branch = effective_remove
            && (args.and_branch || settings.merge_post_merge_also_remove_source_branch);
        if effective_remove {
            let cleanup_opts = crate::core::worktree::merge::CleanupOptions {
                remove_worktree: effective_remove,
                also_branch: effective_and_branch,
            };
            crate::core::worktree::merge::execute_cleanup(
                &params.sources,
                &cleanup_opts,
                &git,
                &project_root,
            )?;
        }
        Ok(())
    }
}

/// Fire the `worktree-post-create` hook for a worktree promoted from the
/// ephemeral `.daft-tmp/<branch>` path to its layout-resolved sibling path.
///
/// Mirrors `flow_adopt::run_post_adopt_hook` in shape — construct a
/// `HookContext` for the new worktree (`source_worktree` == `worktree_path`
/// because no source worktree is involved in a promotion), run the executor
/// with a plain-CLI presenter, and surface the outcome without blocking.
///
/// Hook failures propagate as `Err` so the caller can print a warning; they
/// do not cause the conflict report to be suppressed.
fn fire_worktree_post_create_hook(
    worktree_path: &Path,
    branch: &str,
    project_root: &Path,
    settings: &DaftSettings,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let git_dir = get_git_common_dir()?;

    let ctx = HookContext::new(
        HookType::PostCreate,
        "merge",
        project_root,
        &git_dir,
        &settings.remote,
        worktree_path,
        worktree_path,
        branch,
    )
    .with_new_branch(false);

    // Minimal CLI output — `CliPresenter::auto` picks a progress style suited
    // to the current TTY. The merge command is already past its own output
    // phase here (about to print a conflict report), so a plain output sink
    // is sufficient.
    let mut output = CliOutput::new(OutputConfig::new(false, false));
    let presenter = CliPresenter::auto(&HookOutputConfig::default());
    executor.execute(&ctx, &mut output, presenter)?;
    Ok(())
}

/// [`HookRunner`] backed by a [`HookExecutor`] and a CLI output sink.
///
/// Built in `run()` before calling `execute_start` so the core merge logic
/// can fire `merge-pre` / `merge-post` without pulling `HookExecutor`,
/// `Output`, or `JobPresenter` into the core crate. The static context
/// (project_root, git_dir, remote, source worktree) is captured at
/// construction; target path + branch come from the `MergeHookContext`'s
/// env vars on each call, since they may differ between pre and post
/// (e.g., ephemeral promotion switches the path mid-merge).
struct MergeHookRunner<'a> {
    executor: HookExecutor,
    output: &'a mut dyn Output,
    project_root: PathBuf,
    git_dir: PathBuf,
    remote: String,
    source_worktree: PathBuf,
}

impl<'a> MergeHookRunner<'a> {
    fn new(
        output: &'a mut dyn Output,
        project_root: PathBuf,
        git_dir: PathBuf,
        remote: String,
        source_worktree: PathBuf,
    ) -> Result<Self> {
        let hooks_config = HooksConfig::default();
        let executor = HookExecutor::new(hooks_config)?;
        Ok(Self {
            executor,
            output,
            project_root,
            git_dir,
            remote,
            source_worktree,
        })
    }

    /// Build a `HookContext` for a merge hook, reading target path/branch
    /// from the `MergeHookContext`'s env vars and attaching the full env
    /// map as `extra_env` so hook scripts observe every `DAFT_MERGE_*`.
    fn build_ctx(&self, hook_type: HookType, merge_ctx: &MergeHookContext) -> HookContext {
        // Ref-only FF path stamps an empty DAFT_MERGE_TARGET_PATH; fall
        // back to the source worktree so the hook cwd is a real dir.
        let target_path = merge_ctx
            .env
            .get("DAFT_MERGE_TARGET_PATH")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.source_worktree.clone());
        let branch = merge_ctx
            .env
            .get("DAFT_MERGE_TARGET_BRANCH")
            .cloned()
            .unwrap_or_default();
        HookContext::new(
            hook_type,
            "merge",
            self.project_root.clone(),
            self.git_dir.clone(),
            self.remote.clone(),
            self.source_worktree.clone(),
            target_path,
            branch,
        )
        .with_extra_env(merge_ctx.env.clone())
    }

    fn fire(&mut self, hook_type: HookType, merge_ctx: &MergeHookContext) -> Result<()> {
        let ctx = self.build_ctx(hook_type, merge_ctx);
        let presenter = CliPresenter::auto(&HookOutputConfig::default());
        // `execute` returns Err when a hook fails AND its fail mode is
        // Abort. merge-pre defaults to Abort, merge-post to Warn, so the
        // trait method's "Err aborts" contract is honored by the
        // executor's own fail-mode plumbing.
        self.executor.execute(&ctx, self.output, presenter)?;
        Ok(())
    }
}

impl<'a> HookRunner for MergeHookRunner<'a> {
    fn fire_merge_pre(&mut self, ctx: &MergeHookContext) -> Result<()> {
        self.fire(HookType::MergePre, ctx)
    }

    fn fire_merge_post(&mut self, ctx: &MergeHookContext) -> Result<()> {
        // merge-post's fail mode is Warn by default, so executor.execute()
        // won't return Err. If the user has configured it to Abort, we
        // still surface the error here — the core layer will log it and
        // not roll back the merge.
        self.fire(HookType::MergePost, ctx)
    }
}
