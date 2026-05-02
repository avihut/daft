//! `daft repo` subcommand category.
//!
//! Currently the only verb is `repo remove`; future verbs (`list`, `info`)
//! will dispatch alongside it from `run()`.

use anyhow::Result;

pub mod remove;

/// Dispatch entry from the top-level main.
pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let sub = args.get(2).map(String::as_str).unwrap_or("");
    match sub {
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
    println!("  remove   Remove a repository, including all worktrees");
}
