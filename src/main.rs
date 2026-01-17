/// daft - Git Extensions Toolkit
///
/// A multicall binary that provides Git extensions through symlinks.
/// Detects how it was invoked (via argv[0]) and routes to the appropriate command.
use anyhow::Result;
use std::path::Path;

mod commands;

fn main() -> Result<()> {
    // Detect how we were invoked by checking argv[0]
    let program_path = std::env::args()
        .next()
        .unwrap_or_else(|| "daft".to_string());
    let program_name = Path::new(&program_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("daft");

    // Route to the appropriate command based on invocation name
    match program_name {
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

        // Documentation command (via git-daft symlink or direct invocation)
        "git-daft" => commands::docs::run(),

        // Main daft command - check for subcommands
        "daft" => {
            // Check if a subcommand was provided
            let args: Vec<String> = std::env::args().collect();
            if args.len() > 1 {
                match args[1].as_str() {
                    "completions" => commands::completions::run(),
                    "__complete" => commands::complete::run(),
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
