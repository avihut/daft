//! `daft repo install` — canonical name for the daft.yml bootstrap.
//!
//! All behavior lives in [`crate::commands::install`] (the starter template,
//! the refuse-if-exists guard, the write). This module only adapts the argv
//! offset — `daft repo install ...` carries two leading verbs to strip — and
//! supplies the repo-namespaced clap program name so `--help`/man output reads
//! `git-daft-repo-install`. The top-level `daft install` is a thin alias for
//! this command (see [`crate::commands::install::run`]).

use anyhow::Result;
use clap::Parser;

use crate::commands::install::run_with_output;
use crate::output::{CliOutput, OutputConfig};

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-install")]
#[command(version = crate::VERSION)]
#[command(about = "Install a starter daft.yml in the current worktree")]
#[command(long_about = r#"
Creates a starter daft.yml at the current worktree root with a commented
skeleton covering the major sections (hooks, shared, layout). Modeled on
`lefthook install`.

This is the canonical name for the bootstrap; `daft install` is a top-level
alias that runs the same thing (so lefthook-style discovery keeps working).

If daft.yml already exists, the command refuses without modifying anything;
edit the existing file with your editor or a future `daft config` TUI.

No git side effects: daft does not write to .gitignore or .git/info/exclude.
Ignore rules are the user's responsibility.
"#)]
pub struct Args {
    #[arg(short = 'q', long = "quiet", help = "Suppress progress reporting")]
    quiet: bool,

    #[arg(short = 'v', long = "verbose", help = "Show detailed progress")]
    verbose: bool,
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
    run_with_output(&mut output)
}
