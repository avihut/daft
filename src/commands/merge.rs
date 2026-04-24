//! git-worktree-merge - Merge branches across worktrees
//!
//! Mirrors git merge semantics when --into is omitted; enables
//! cross-worktree merges (merge <source>... into <target> from any
//! worktree) when --into is supplied. Finish commands (--abort,
//! --continue, --quit) take an optional positional <worktree|branch>
//! argument, default to CWD.

use crate::{
    get_project_root, git::GitCommand, is_git_repository, logging::init_logging,
    settings::DaftSettings,
};
use anyhow::Result;
use clap::Parser;

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
    /// Source branches/commits to merge. Two or more invoke octopus.
    #[arg(value_name = "SOURCE", num_args = 1..)]
    pub sources: Vec<String>,

    /// Target worktree/branch. Defaults to the current worktree's branch.
    #[arg(long = "into", value_name = "TARGET")]
    pub into: Option<String>,

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
    /// Suppress the diffstat at the end of the merge (also `-n`).
    #[arg(short = 'n', long = "no-stat", conflicts_with = "stat")]
    pub no_stat: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    pub verbose: bool,
}

/// Translate parsed CLI [`Args`] into [`EffectiveFlags`].
///
/// Clap's `conflicts_with`/`conflicts_with_all` attrs guarantee at parse time
/// that each paired bool (e.g. `edit`/`no_edit`) has at most one side true,
/// so `else if` chains below are exhaustive in practice.
///
/// `-S` has dual semantics in git: bare `-S` means "use the default key",
/// and `-S<KEYID>` binds a specific key. Clap exposes this via
/// `num_args = 0..=1` + `default_missing_value = ""`: an empty string means
/// "passed with no value", a non-empty string means a key ID.
fn effective_flags_from_args(args: &Args) -> crate::core::worktree::merge::EffectiveFlags {
    use crate::core::worktree::merge::{EffectiveFlags, FfMode, GpgSign};

    let ff = if args.ff_only {
        Some(FfMode::Only)
    } else if args.no_ff {
        Some(FfMode::Never)
    } else if args.ff {
        Some(FfMode::Auto)
    } else {
        None
    };

    let squash = if args.squash {
        Some(true)
    } else if args.no_squash {
        Some(false)
    } else {
        None
    };

    let commit = if args.commit {
        Some(true)
    } else if args.no_commit {
        Some(false)
    } else {
        None
    };

    let edit = if args.edit {
        Some(true)
    } else if args.no_edit {
        Some(false)
    } else {
        None
    };

    let signoff = if args.signoff {
        Some(true)
    } else if args.no_signoff {
        Some(false)
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
    } else {
        None
    };

    let verify_signatures = if args.verify_signatures {
        Some(true)
    } else if args.no_verify_signatures {
        Some(false)
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

    EffectiveFlags {
        message: args.message.clone(),
        file: args.file.clone(),
        edit,
        cleanup: args.cleanup.clone(),
        ff,
        squash,
        commit,
        signoff,
        strategy: args.strategy.clone(),
        strategy_options: args.strategy_options.clone(),
        gpg_sign,
        verify_signatures,
        allow_unrelated_histories: args.allow_unrelated_histories,
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

    let flags = effective_flags_from_args(&args);
    let params = crate::core::worktree::merge::StartParams {
        sources: args.sources,
        target: args.into,
        flags,
    };
    let outcome = crate::core::worktree::merge::execute_start(&params, &git, &project_root)?;

    if outcome.already_up_to_date {
        println!("Already up to date.");
    } else if outcome.failed {
        anyhow::bail!("merge conflicted — resolve then run `daft merge --continue`");
    } else {
        println!("Merge complete.");
    }
    Ok(())
}
