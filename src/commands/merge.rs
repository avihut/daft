//! git-worktree-merge - Merge branches across worktrees
//!
//! Mirrors git merge semantics when --into is omitted; enables
//! cross-worktree merges (merge <source>... into <target> from any
//! worktree) when --into is supplied. Finish commands (--abort,
//! --continue, --quit) take an optional positional <worktree|branch>
//! argument, default to CWD.

use crate::{
    core::{
        worktree::merge::{HookRunner, MergeHookContext},
        CommandBridge,
    },
    executor::cli_presenter::CliPresenter,
    get_current_worktree_path, get_git_common_dir, get_project_root,
    git::GitCommand,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::{load_hooks_config, DaftSettings, HookOutputConfig},
};
use anyhow::Result;
use clap::Parser;
use std::io::IsTerminal;
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
            "style_merge", "squash", "rebase", "rebase_merge",
            "commit", "no_commit",
            "signoff", "no_signoff",
            "strategy", "strategy_options",
            "gpg_sign", "no_gpg_sign",
            "verify_signatures", "no_verify_signatures",
            "allow_unrelated_histories",
            "stat", "no_stat",
            "adopt_target", "no_adopt_target", "yes",
            "remove_branch", "keep_branch", "set_default",
        ],
    )]
    pub abort: bool,

    /// Continue an in-progress merge in the named worktree (defaults to CWD).
    ///
    /// When continuing a squash-staged state, the commit-message flags
    /// (`--no-edit`, `-m`, `-F`, `--signoff`, `--gpg-sign`, `--cleanup`) may
    /// be supplied to control the commit step. They are forwarded to
    /// `git commit` and are *not* treated as start-mode flags.
    #[arg(
        long = "continue",
        conflicts_with_all = [
            "abort", "quit",
            "style_merge", "squash", "rebase", "rebase_merge",
            "commit", "no_commit",
            "strategy", "strategy_options",
            "verify_signatures", "no_verify_signatures",
            "allow_unrelated_histories",
            "stat", "no_stat",
            "adopt_target", "no_adopt_target", "yes",
            "remove_branch", "keep_branch", "set_default",
        ],
    )]
    pub continue_merge: bool,

    /// Quit an in-progress merge without resetting the index (defaults to CWD).
    #[arg(
        long = "quit",
        conflicts_with_all = [
            "abort", "continue_merge",
            "message", "file", "edit", "no_edit", "cleanup",
            "style_merge", "squash", "rebase", "rebase_merge",
            "commit", "no_commit",
            "signoff", "no_signoff",
            "strategy", "strategy_options",
            "gpg_sign", "no_gpg_sign",
            "verify_signatures", "no_verify_signatures",
            "allow_unrelated_histories",
            "stat", "no_stat",
            "adopt_target", "no_adopt_target", "yes",
            "remove_branch", "keep_branch", "set_default",
        ],
    )]
    pub quit: bool,

    // --- Commit message and editor ---
    /// Commit message for the merge commit (mirrors `git merge -m`).
    #[arg(short = 'm', value_name = "MSG", conflicts_with = "rebase")]
    pub message: Option<String>,
    /// Read the commit message from FILE (mirrors `git merge -F`).
    #[arg(
        short = 'F',
        long = "file",
        value_name = "FILE",
        conflicts_with = "rebase"
    )]
    pub file: Option<std::path::PathBuf>,
    /// Launch the editor to edit the merge commit message.
    #[arg(long = "edit", conflicts_with_all = ["no_edit", "rebase"])]
    pub edit: bool,
    /// Accept the auto-generated merge commit message without editing.
    #[arg(long = "no-edit", conflicts_with_all = ["edit", "rebase"])]
    pub no_edit: bool,
    /// Message cleanup mode (mirrors `git merge --cleanup`).
    #[arg(long = "cleanup", value_name = "MODE", conflicts_with = "rebase")]
    pub cleanup: Option<String>,

    // --- Merge style (mutually exclusive; default = merge) ---
    /// Explicit merge style — always create a merge commit. This is the default;
    /// the flag exists for canceling a config-set default style.
    #[arg(
        long = "merge",
        conflicts_with_all = ["squash", "rebase", "rebase_merge"],
    )]
    pub style_merge: bool,

    /// Squash style — collapse source's commits into one squashed commit on target.
    #[arg(
        long = "squash",
        conflicts_with_all = ["style_merge", "rebase", "rebase_merge"],
    )]
    pub squash: bool,

    /// Rebase style — rebase source onto target, then fast-forward (linear, preserves commits).
    #[arg(
        long = "rebase",
        conflicts_with_all = ["style_merge", "squash", "rebase_merge"],
    )]
    pub rebase: bool,

    /// Rebase-merge style — rebase source onto target, then create a merge commit.
    #[arg(
        long = "rebase-merge",
        conflicts_with_all = ["style_merge", "squash", "rebase"],
    )]
    pub rebase_merge: bool,

    // --- Commit control ---
    /// Automatically create the merge commit after a successful merge.
    #[arg(long = "commit", conflicts_with = "no_commit")]
    pub commit: bool,
    /// Leave the merge staged without committing.
    #[arg(long = "no-commit", conflicts_with_all = ["commit", "remove_branch"])]
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
    #[arg(long = "allow-unrelated-histories", conflicts_with_all = ["rebase", "rebase_merge"])]
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
    /// Remove the source worktree and delete the source branch. The local/remote
    /// behavior follows `branch.deleteRemote` (defaults to local-only).
    #[arg(short = 'r', long = "remove-branch", conflicts_with = "keep_branch")]
    pub remove_branch: bool,

    /// Explicit keep — for canceling a config-set `merge.cleanup = remove-branch`.
    #[arg(long = "keep-branch", conflicts_with = "remove_branch")]
    pub keep_branch: bool,

    // --- Defaults persistence ---
    /// Write the resolved style/cleanup choices to `git config --local` after
    /// the merge succeeds.
    #[arg(long = "set-default")]
    pub set_default: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    pub verbose: bool,
}

/// Translate parsed CLI [`Args`] + [`DaftSettings`] into [`EffectiveFlags`].
///
/// Clap's `conflicts_with`/`conflicts_with_all` attrs guarantee at parse time
/// that each paired bool (e.g. `edit`/`no_edit`) has at most one side true,
/// so `else if` chains below are exhaustive in practice.
///
/// Precedence: CLI flags > `daft.merge.*` config > built-in defaults.
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
    use crate::core::worktree::merge::{EffectiveFlags, GpgSign, MergeStyle};

    // style: CLI wins; default = settings.merge_style.
    let style = if args.style_merge {
        MergeStyle::Merge
    } else if args.squash {
        MergeStyle::Squash
    } else if args.rebase {
        MergeStyle::Rebase
    } else if args.rebase_merge {
        MergeStyle::RebaseMerge
    } else {
        settings.merge_style
    };

    let commit = if args.commit {
        Some(true)
    } else if args.no_commit || !settings.merge_commit {
        Some(false)
    } else {
        None
    };

    let edit = if args.edit {
        Some(true)
    } else if args.no_edit || args.yes {
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

    let stat = if args.stat {
        Some(true)
    } else if args.no_stat {
        Some(false)
    } else {
        None
    };

    let strategy = args
        .strategy
        .clone()
        .or_else(|| settings.merge_strategy.clone());

    let mut strategy_options = settings.merge_strategy_options.clone();
    strategy_options.extend(args.strategy_options.iter().cloned());

    let allow_unrelated_histories =
        args.allow_unrelated_histories || settings.merge_allow_unrelated_histories;

    EffectiveFlags {
        message: args.message.clone(),
        file: args.file.clone(),
        edit,
        cleanup: args.cleanup.clone(),
        style,
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

/// Resolve the effective cleanup kind from CLI args and settings.
///
/// CLI flags (`--remove-branch`, `--keep-branch`) win over settings.
fn effective_cleanup_from_args_and_settings(
    args: &Args,
    settings: &DaftSettings,
) -> crate::core::worktree::merge::CleanupKind {
    use crate::core::worktree::merge::CleanupKind;
    if args.remove_branch {
        CleanupKind::RemoveBranch
    } else if args.keep_branch {
        CleanupKind::Keep
    } else {
        settings.merge_cleanup
    }
}

/// Returns the first 7 characters of a SHA (or the full SHA if shorter).
fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
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
        // For --continue on squash-staged state, the commit-composing flags
        // (`--no-edit`, `-m`, `-F`, `--signoff`, `--gpg-sign`, `--cleanup`)
        // are forwarded to `git commit`. Build a minimal EffectiveFlags with
        // just those fields; merge-only flags (ff, squash, strategy, etc.)
        // are left at default/None since they aren't used in the commit step.
        let commit_flags = {
            use crate::core::worktree::merge::{EffectiveFlags, GpgSign};
            // -y/--yes implies --no-edit: non-interactive callers shouldn't
            // be dropped into an editor when resuming a squash commit.
            let edit = if args.no_edit || args.yes {
                Some(false)
            } else if args.edit {
                Some(true)
            } else {
                None
            };
            let signoff = if args.signoff { Some(true) } else { None };
            let gpg_sign = if args.no_gpg_sign {
                Some(GpgSign::Disabled)
            } else {
                args.gpg_sign.as_deref().map(|k| {
                    if k.is_empty() {
                        GpgSign::Default
                    } else {
                        GpgSign::KeyId(k.to_string())
                    }
                })
            };
            EffectiveFlags {
                message: args.message.clone(),
                file: args.file.clone(),
                edit,
                cleanup: args.cleanup.clone(),
                signoff,
                gpg_sign,
                ..EffectiveFlags::default()
            }
        };
        let params = crate::core::worktree::merge::FinishParams {
            worktree: worktree_arg,
            mode,
            commit_flags,
        };
        let mut runner = crate::core::worktree::merge::NullHookRunner;
        return crate::core::worktree::merge::execute_finish(
            &params,
            &git,
            &project_root,
            &mut runner,
        );
    }

    // Start mode. Clap's `num_args = 0..` on `sources` allows zero positionals
    // (needed for finish mode above); re-assert the start-mode minimum here.
    if args.sources.is_empty() {
        anyhow::bail!("specify at least one source to merge");
    }

    let flags = effective_flags_from_args_and_settings(&args, &settings);

    // Pre-flight TTY guard: refuse a --squash that would open an editor when
    // stdin is not a terminal. The editor would either hang waiting for input
    // or receive EOF and abort, leaving the worktree in a half-merged state.
    // Callers in non-TTY contexts (CI, piped scripts) should supply
    // --no-edit, -m <msg>, or -F <file> instead.
    if flags.would_open_editor() && !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "No TTY available for the commit-message editor.\n\
             Pass --no-edit to use the auto-generated message, \
             -m <msg> for an explicit message, or -F <file> to read from a file."
        );
    }

    // Resolve cleanup kind from CLI + settings.
    let cleanup_kind = effective_cleanup_from_args_and_settings(&args, &settings);

    // Pre-flight cleanup-vs-no-commit guard: catch the diagonal case where
    // `daft.merge.commit=false` is set in git config (captured as
    // `flags.commit == Some(false)`) while cleanup is requested via CLI flags
    // or config. Clap's `conflicts_with_all` on `--no-commit` catches the
    // pure-CLI case at parse time (exit 2). `validate_merge_settings` in
    // `DaftSettings::load` catches the pure-config case. Neither catches this
    // diagonal: config disables commit while CLI adds cleanup. We check it
    // here, after `effective_flags_from_args_and_settings` has merged both
    // sources, so we see the true effective intent.
    if matches!(flags.commit, Some(false))
        && cleanup_kind == crate::core::worktree::merge::CleanupKind::RemoveBranch
    {
        anyhow::bail!(
            "--no-commit / daft.merge.commit=false is incompatible with cleanup \
             (--remove-branch / daft.merge.cleanup=remove-branch); cleanup requires a committed merge."
        );
    }

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
    // ephemeral-promote path fires, and for the state-aware terminal message.
    let into_branch = args.into.clone();
    let sources_for_message = args.sources.clone();
    // Populate the cleanup intent template when squash + cleanup is requested.
    // This is passed to the core so it can write the daft-merge-intent.json
    // marker BEFORE git commit, enabling --continue to resume cleanup after
    // an editor abort.
    use crate::core::worktree::merge::{CleanupKind, MergeStyle};
    let squash_requested = matches!(flags.style, MergeStyle::Squash);
    let cleanup_requested = cleanup_kind == CleanupKind::RemoveBranch;
    // Capture merge_style before flags is moved into params (below). Used by
    // --set-default to write the invocation's style as a repo default.
    let merge_style = flags.style;
    let cleanup_intent = if squash_requested && cleanup_requested {
        Some(crate::core::worktree::merge::MergeIntentTemplate {
            remove_worktree: true,
            also_branch: true,
        })
    } else {
        None
    };
    let params = crate::core::worktree::merge::StartParams {
        sources: args.sources,
        target: args.into,
        flags,
        adopt,
        require_clean_target: settings.merge_require_clean_target,
        cleanup_intent,
    };
    // Build the MergeHookRunner used to fire pre-merge / post-merge hooks
    // from inside `execute_start`. Holding the executor + output here (in
    // the command layer) keeps core free of presenter/output dependencies.
    let git_dir = get_git_common_dir()?;
    let source_worktree = get_current_worktree_path().unwrap_or_else(|_| project_root.clone());
    let mut output = CliOutput::new(OutputConfig::new(false, args.verbose));
    // Run execute_start inside a nested block so `runner` (which borrows
    // `&mut output`) is dropped before the cleanup phase needs `output` again.
    // `squash_requested` was captured before `flags` was moved into `params`.
    let target_label = into_branch
        .as_deref()
        .unwrap_or("current branch")
        .to_string();
    let spinner_label = if squash_requested {
        format!(
            "Squashing {} into {}...",
            sources_for_message.join(", "),
            target_label
        )
    } else {
        format!(
            "Merging {} into {}...",
            sources_for_message.join(", "),
            target_label
        )
    };
    output.start_spinner(&spinner_label);
    let outcome_result = {
        let mut runner = MergeHookRunner::new(
            &mut output,
            project_root.clone(),
            git_dir,
            settings.remote.clone(),
            source_worktree,
        )?;
        crate::core::worktree::merge::execute_start(&params, &git, &project_root, &mut runner)
    };
    output.finish_spinner();
    // Dump captured git output to stderr after the spinner stops (avoids
    // carriage-return mangling). On failure, always dump; on success, only
    // dump when --verbose is set.
    let outcome = outcome_result?;
    if !outcome.captured_git_output.is_empty() {
        let should_dump = outcome.failed || args.verbose;
        if should_dump {
            eprint!("{}", String::from_utf8_lossy(&outcome.captured_git_output));
        }
    }

    if outcome.already_up_to_date {
        // Core already printed "Already up to date." from the up-to-date
        // short-circuit (which also sets `emitted_terminal_message`). No
        // further print here — duplicating the status line would be noise.
        Ok(())
    } else if outcome.failed {
        // Commit-aborted path: squash staged, `git commit` was aborted (editor
        // empty, pre-commit hook refused, GPG-sign fail, etc.). Changes remain
        // staged on the target. Cleanup is skipped — there is nothing to clean
        // up: the branch still has useful staged content the user wants to commit.
        if outcome.commit_aborted {
            let target = &outcome.target_branch;
            eprintln!(
                "Commit aborted; squash changes are still staged on {target}. Cleanup skipped."
            );
            eprintln!("  Commit manually: git commit");
            eprintln!("  Or reset: git reset --merge");
            std::process::exit(1);
        }

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
        // Squash-cleanup stability check (Slice 4).
        //
        // When a daft-driven squash + commit just landed AND cleanup is
        // requested, re-resolve each source ref and compare to the SHA
        // captured at merge start (stored in `outcome.source_shas`). If any
        // source tip moved during the editor/commit session, refuse cleanup
        // with a clear error — the squash commit stays on the target (the
        // user has a reviewable commit to recover from), but cleanup is
        // skipped to avoid force-deleting a branch that has new work.
        //
        // This check ONLY fires on the squash-committed + cleanup path.
        // Regular merges use git's safe `branch -d` reachability check
        // (already in execute_cleanup Phase 1 when squash_committed=false).
        let squash_cleanup_stable = if outcome.squash_commit_sha.is_some() && cleanup_requested {
            let mut moved_sources: Vec<String> = Vec::new();
            for (src, captured_sha) in params.sources.iter().zip(outcome.source_shas.iter()) {
                match git.rev_parse(src) {
                    Ok(current_sha) => {
                        if current_sha != *captured_sha {
                            moved_sources.push(format!(
                                "source '{}' moved during merge \
                                 (was {}, now {})",
                                src,
                                &captured_sha[..12.min(captured_sha.len())],
                                &current_sha[..12.min(current_sha.len())]
                            ));
                        }
                    }
                    Err(e) => {
                        moved_sources
                            .push(format!("source '{}' could not be re-resolved: {}", src, e));
                    }
                }
            }
            if !moved_sources.is_empty() {
                // Print the squash commit success before the cleanup refusal —
                // the commit did land; the user needs to know that.
                let sources_display = sources_for_message.join(", ");
                let target = &outcome.target_branch;
                let sha = outcome.squash_commit_sha.as_deref().unwrap_or("");
                let short = short_sha(sha);
                println!("Squash merged {sources_display} into {target} as {short}.");
                anyhow::bail!(
                    "cleanup refused: {}\n  \
                     Re-run cleanup manually if you've reconciled \
                     (e.g. `daft merge -rb` after resolving the branch).",
                    moved_sources.join("; ")
                );
            }
            true
        } else {
            false
        };

        // Core may have already emitted a terminal status line (e.g.,
        // "Fast-forwarded X to Y (no worktree)" from the ref-only FF path).
        // Suppress the new step emission in that case to avoid double output.
        if !outcome.emitted_terminal_message {
            // State-aware step messages per the spec. Dispatched from
            // StartOutcome flags; replaces git's suppressed stdout with a
            // single styled line per merge.
            //
            // `target_display` falls back to the CLI --into arg when core
            // omits target_branch (e.g. the ephemeral non-squash success path).
            let target_display = if outcome.target_branch.is_empty() {
                into_branch.as_deref().unwrap_or("").to_string()
            } else {
                outcome.target_branch.clone()
            };
            let sources_display = sources_for_message.join(", ");
            if outcome.squash_staged_only {
                // --squash --no-commit: staged but not committed.
                output.result(&format!("Squash staged on {target_display}"));
            } else if outcome.squash_commit_sha.is_some() && squash_cleanup_stable {
                // Squash + commit + cleanup path: defer message until AFTER
                // cleanup succeeds (emitted below in the cleanup Ok branch).
            } else if let Some(ref sha) = outcome.squash_commit_sha {
                // --squash with commit, no cleanup requested:
                output.result(&format!(
                    "Squashed {} into {} (commit {})",
                    sources_display,
                    target_display,
                    short_sha(sha)
                ));
            } else if outcome.was_fast_forward {
                if let Some(ref sha) = outcome.merge_commit_sha {
                    output.result(&format!(
                        "Fast-forwarded {} to {}",
                        target_display,
                        short_sha(sha)
                    ));
                }
            } else if let Some(ref sha) = outcome.merge_commit_sha {
                output.result(&format!(
                    "Merged {} into {} (commit {})",
                    sources_display,
                    target_display,
                    short_sha(sha)
                ));
            }
            // else: unreachable in practice; let existing downstream lines render.
        }

        // --set-default: persist the invocation's style + cleanup as repo defaults.
        // Best-effort; failure to write surfaces a warning, doesn't fail the merge.
        if args.set_default {
            match crate::core::worktree::merge_set_default::write_default_settings(
                &project_root,
                merge_style,
                cleanup_kind,
            ) {
                Ok(()) => output.defaults_updated(merge_style, cleanup_kind),
                Err(e) => output.warning(&format!("failed to update repository defaults: {e}")),
            }
        }

        // Post-merge cleanup. Only runs on successful, non-up-to-date merges
        // — the `already_up_to_date` and `failed` arms above have already
        // returned or exited.
        //
        // `cleanup_kind` was resolved pre-flight above to allow the
        // no-commit + cleanup guard to fire early. Reuse here.
        //
        // `squash_cleanup_stable` is true when the squash-committed path
        // passed the stability check — in that case we use `branch -D`
        // (justified by content equivalence proof). Otherwise `squash_committed`
        // stays false and plan_cleanup uses the standard reachability check
        // (delegated to branch_delete::execute's keep_local_branch=false path).
        if cleanup_kind == CleanupKind::RemoveBranch {
            let cleanup_opts = crate::core::worktree::merge::CleanupOptions {
                remove_worktree: true,
                also_branch: true,
                squash_committed: squash_cleanup_stable,
            };
            let cleanup_result: Result<()> = (|| {
                let plan = crate::core::worktree::merge::plan_cleanup(
                    &params.sources,
                    &cleanup_opts,
                    &git,
                    &project_root,
                    &outcome.target_branch,
                )?;

                // Hoist HooksConfig construction above the loop so config is
                // resolved once, not N times for N sources. HookExecutor is
                // constructed per-item because CommandBridge takes ownership
                // and HookExecutor is not Clone.
                let hooks_config = HooksConfig::default();

                output.start_spinner(if plan.len() == 1 {
                    "Cleaning up source..."
                } else {
                    "Cleaning up sources..."
                });

                for item in &plan {
                    if item.worktree_path.is_none() && item.branch_name.is_none() {
                        continue;
                    }

                    // keep_local_branch=true when -r was given without -b: the
                    // worktree is removed but the local branch ref is preserved.
                    let keep_local_branch = item.branch_name.is_none();
                    // For worktree-only items (keep_local_branch=true) we still
                    // need a branch name so branch_delete::execute can find the
                    // worktree via its worktree-map lookup. Use the `source` field
                    // which equals the resolved branch name.
                    let branch_for_delete = item
                        .branch_name
                        .clone()
                        .unwrap_or_else(|| item.source.clone());

                    let bd_params = crate::core::worktree::branch_delete::BranchDeleteParams {
                        branches: vec![branch_for_delete],
                        // The planner has already validated reachability against
                        // the actual merge target (which may differ from the
                        // default branch). Setting force=true here bypasses
                        // branch_delete's redundant default-branch reachability
                        // check, which would incorrectly reject cross-target
                        // merges (e.g. `--into develop --remove-branch` when
                        // feature is not yet reachable from main). For
                        // squash-committed items, item.force_delete is already
                        // true, so this is a no-op for that path.
                        force: true,
                        use_gitoxide: settings.use_gitoxide,
                        is_quiet: false,
                        remote_name: settings.remote.clone(),
                        delete_remote: settings.branch_delete_remote,
                        remote_only: false,
                        keep_local_branch,
                        prune_cd_target: settings.prune_cd_target,
                        // Expose DAFT_COMMAND=merge so hook scripts can
                        // distinguish merge cleanup from standalone daft remove.
                        command_label: "merge".to_string(),
                    };

                    let bd_result = {
                        let executor = HookExecutor::new(hooks_config.clone())?;
                        let mut bridge = CommandBridge::new(&mut output, executor);
                        crate::core::worktree::branch_delete::execute(&bd_params, &mut bridge)?
                    };

                    if !bd_result.validation_errors.is_empty() {
                        output.finish_spinner();
                        for err in &bd_result.validation_errors {
                            output.error(&format!(
                                "cleanup of '{}' failed: {}",
                                err.branch, err.message
                            ));
                        }
                        anyhow::bail!(
                            "cleanup pre-validation failed for {} source(s)",
                            bd_result.validation_errors.len()
                        );
                    }

                    // Emit styled "Deleted X (worktree, local branch)" summary lines.
                    for deletion in &bd_result.deletions {
                        let parts = deletion.deleted_parts();
                        if !parts.is_empty() {
                            output.result(&format!("Deleted {} ({})", deletion.branch, parts));
                        }
                    }
                }

                output.finish_spinner();
                Ok(())
            })();

            match cleanup_result {
                Ok(()) => {
                    // After successful squash + cleanup, emit the combined message.
                    if squash_cleanup_stable && !outcome.emitted_terminal_message {
                        let sources_display = sources_for_message.join(", ");
                        output.result(&format!("Squash merged and cleaned up {sources_display}."));
                    }
                }
                Err(e) => {
                    // Squash committed but cleanup failed (e.g. dirty source
                    // worktree). Inform the user the squash landed before
                    // surfacing the cleanup error.
                    if squash_cleanup_stable && !outcome.emitted_terminal_message {
                        let sources_display = sources_for_message.join(", ");
                        let target = &outcome.target_branch;
                        let sha = outcome.squash_commit_sha.as_deref().unwrap_or("");
                        let short = short_sha(sha);
                        println!("Squash merged {sources_display} into {target} as {short}.");
                    }
                    return Err(e);
                }
            }
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
/// can fire `pre-merge` / `post-merge` without pulling `HookExecutor`,
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
        let hooks_config = load_hooks_config()?;
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
        // Abort. pre-merge defaults to Abort, post-merge to Warn, so the
        // trait method's "Err aborts" contract is honored by the
        // executor's own fail-mode plumbing.
        self.executor.execute(&ctx, self.output, presenter)?;
        Ok(())
    }
}

impl<'a> HookRunner for MergeHookRunner<'a> {
    fn fire_pre_merge(&mut self, ctx: &MergeHookContext) -> Result<()> {
        self.fire(HookType::PreMerge, ctx)
    }

    fn fire_post_merge(&mut self, ctx: &MergeHookContext) -> Result<()> {
        // post-merge's fail mode is Warn by default, so executor.execute()
        // won't return Err. If the user has configured it to Abort, we
        // still surface the error here — the core layer will log it and
        // not roll back the merge.
        self.fire(HookType::PostMerge, ctx)
    }

    fn pause_spinner(&mut self) {
        self.output.pause_spinner();
    }

    fn resume_spinner(&mut self) {
        self.output.resume_spinner();
    }
}
