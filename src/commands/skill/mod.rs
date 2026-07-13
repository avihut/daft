//! `daft skill` subcommand category.
//!
//! Verbs: `install` (write the embedded agent skill to a skills directory —
//! install doubles as update) and `show` (print the embedded skill to
//! stdout). The skill content itself lives in `crate::skill`; this module is
//! only the CLI surface.

use anyhow::Result;

pub mod install;
pub mod show;

/// Dispatch entry from the top-level main.
pub fn run() -> Result<()> {
    let args: Vec<String> = crate::cli::argv().to_vec();
    let sub = args.get(2).map(String::as_str).unwrap_or("");
    match sub {
        "install" => install::run(),
        "show" => show::run(),
        "" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "--version" | "-V" => {
            println!("daft skill {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => crate::suggest::handle_unknown_subcommand(
            "daft skill",
            other,
            crate::suggest::DAFT_SKILL_SUBCOMMANDS,
        ),
    }
}

fn print_help() {
    println!(
        "daft skill — the daft agent skill ({}), embedded v{}",
        crate::skill::SKILL_DIR_NAME,
        crate::skill::embedded_version()
    );
    println!();
    println!("Subcommands:");
    println!("  install   Install or update the agent skill for Claude Code (~/.claude/skills)");
    println!("  show      Print the embedded SKILL.md to stdout");
}
