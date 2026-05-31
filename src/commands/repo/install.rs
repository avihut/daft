//! `daft repo install` — canonical name for the daft.yml bootstrap.
//!
//! The repo-aware dispatch lives in [`crate::commands::install`] (argv parsing,
//! position resolution, existing-config guidance); the mechanical install — the
//! starter template, the refuse-if-exists guard, the write — lives in
//! [`crate::core::install`]. This module only adapts the argv offset —
//! `daft repo install ...` carries two leading verbs to strip — and supplies
//! the repo-namespaced clap program name so `--help`/man output reads
//! `git-daft-repo-install`. The top-level `daft install` is a thin alias for
//! this command (see [`crate::commands::install::run`]).

use anyhow::Result;
use clap::Parser;

use crate::commands::install::run_with_output;
use crate::core::install::InstallOptions;
use crate::output::{CliOutput, OutputConfig};

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-install")]
#[command(version = crate::VERSION)]
#[command(about = "Install a starter daft.yml in the current worktree")]
#[command(long_about = r#"
Creates a starter daft.yml at the worktree root with a commented skeleton
covering the major sections (hooks, shared, layout). Modeled on
`lefthook install`.

This is the canonical name for the bootstrap; `daft install` is a top-level
alias that runs the same thing (so lefthook-style discovery keeps working).

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
    // Build clap argv: program name + everything after `daft repo install`.
    // `daft repo` is a subcommand category (like `daft activate shortcuts`), so
    // `crate::get_clap_args` does not recognize it; we rebuild argv manually.
    //
    // The router in `src/main.rs` only dispatches here when argv[1] == "repo"
    // and argv[2] == "install", so `skip(3)` is correct. Assert the invariant
    // in debug builds — if a future shortcut alias or alternative entry path
    // dispatches here without that argv shape, we want a loud failure rather
    // than silently dropping or shifting positional arguments.
    let raw_args: Vec<String> = crate::cli::argv().to_vec();
    debug_assert!(
        raw_args.len() >= 3 && raw_args[1] == "repo" && raw_args[2] == "install",
        "repo::install::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft repo install ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-repo-install".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    let args = Args::parse_from(argv);
    let mut output = CliOutput::new(OutputConfig::new(args.quiet, args.verbose));
    run_with_output(
        &mut output,
        InstallOptions {
            git_exclude: args.git_exclude,
        },
    )
}
