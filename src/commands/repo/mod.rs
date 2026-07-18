//! `daft repo` subcommand category.
//!
//! Verbs: the catalog surface (`add`, `list`, `info`), `repo install`
//! (canonical name for the daft.yml bootstrap, also reachable via the
//! top-level `daft install` alias), `repo remove`, and the relations-manifest
//! editors (`link`, `unlink`).

use anyhow::Result;

pub mod add;
pub mod info;
pub mod install;
pub mod link;
pub mod list;
pub mod relation_io;
pub mod remove;
pub mod unlink;

/// Dispatch entry from the top-level main.
pub fn run() -> Result<()> {
    let args: Vec<String> = crate::cli::argv().to_vec();
    let sub = args.get(2).map(String::as_str).unwrap_or("");
    match sub {
        "add" => add::run(),
        "info" => info::run(),
        "install" => install::run(),
        "link" => link::run(),
        "list" => list::run(),
        "remove" => remove::run(),
        "unlink" => unlink::run(),
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
    println!("  add       Register a repository in the repo catalog");
    println!("  info      Show a repository's catalog entry");
    println!("  install   Install a starter daft.yml in the current worktree");
    println!("  link      Declare a relation from this repo to another");
    println!("  list      List repositories in the repo catalog");
    println!("  remove    Remove a repository, including all worktrees");
    println!("  unlink    Remove a relation from this repo");
}
