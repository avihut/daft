//! `daft repo` subcommand category.
//!
//! Verbs: `repo install` (canonical name for the daft.yml bootstrap, also
//! reachable via the top-level `daft install` alias) and `repo remove`. Future
//! verbs (`list`, `info`) will dispatch alongside them from `run()`.

use anyhow::Result;

pub mod install;
pub mod remove;

/// Dispatch entry from the top-level main.
pub fn run() -> Result<()> {
    let args: Vec<String> = crate::cli::argv().to_vec();
    let sub = args.get(2).map(String::as_str).unwrap_or("");
    match sub {
        "install" => install::run(),
        "remove" => remove::run(),
        "" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "--version" | "-V" => {
            println!("daft repo {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => crate::suggest::handle_unknown_subcommand(
            "daft repo",
            other,
            crate::suggest::DAFT_REPO_SUBCOMMANDS,
        ),
    }
}

fn print_help() {
    println!("daft repo — repository-level operations");
    println!();
    println!("Subcommands:");
    println!("  install   Install a starter daft.yml in the current worktree");
    println!("  remove    Remove a repository, including all worktrees");
}
