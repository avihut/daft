/// daft - Git Extensions Toolkit
///
/// A multicall binary that provides Git extensions through symlinks.
/// Detects how it was invoked (via argv[0]) and routes to the appropriate command.
use anyhow::Result;
use std::path::Path;

mod commands;
mod shortcuts;

fn main() -> Result<()> {
    // Detect how we were invoked by checking argv[0]
    let program_path = std::env::args()
        .next()
        .unwrap_or_else(|| "daft".to_string());
    let program_name = Path::new(&program_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("daft");

    // Resolve shortcut aliases to full command names
    let resolved = shortcuts::resolve(program_name);

    // Route to the appropriate command based on invocation name
    match resolved {
        // Git worktree extension commands (via symlinks)
        "git-worktree-clone" => commands::clone::run(),
        "git-worktree-init" => commands::init::run(),
        "git-worktree-checkout" => commands::checkout::run(),
        "git-worktree-checkout-branch" => commands::checkout_branch::run(),
        "git-worktree-checkout-branch-from-default" => {
            commands::checkout_branch_from_default::run()
        }
        "git-worktree-prune" => commands::prune::run(),
        "git-worktree-carry" => commands::carry::run(),
        "git-worktree-fetch" => commands::fetch::run(),

        // Documentation command (via git-daft symlink or direct invocation)
        "git-daft" => {
            // Check for subcommands like `git daft hooks`
            let args: Vec<String> = std::env::args().collect();
            if args.len() > 1 {
                match args[1].as_str() {
                    "hooks" => commands::hooks::run(),
                    _ => commands::docs::run(),
                }
            } else {
                commands::docs::run()
            }
        }

        // Main daft command - check for subcommands
        "daft" => {
            // Check if a subcommand was provided
            let args: Vec<String> = std::env::args().collect();
            if args.len() > 1 {
                match args[1].as_str() {
                    "branch" => commands::branch::run(),
                    "completions" => commands::completions::run(),
                    "__complete" => commands::complete::run(),
                    "hooks" => commands::hooks::run(),
                    "man" => commands::man::run(),
                    "multi-remote" => commands::multi_remote::run(),
                    "setup" => {
                        // Check for setup subcommands
                        if args.len() > 2 && args[2] == "shortcuts" {
                            commands::shortcuts::run()
                        } else {
                            commands::setup::run()
                        }
                    }
                    "shell-init" => commands::shell_init::run(),
                    _ => commands::docs::run(),
                }
            } else {
                commands::docs::run()
            }
        }

        // Fallback: show documentation for unknown invocations
        _ => {
            eprintln!("Unknown command: {program_name}");
            eprintln!("Run 'daft' or 'git daft' for help.");
            std::process::exit(1);
        }
    }
}
