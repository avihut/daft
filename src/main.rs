/// daft - Git Extensions Toolkit
///
/// A multicall binary that provides Git extensions through symlinks.
/// Detects how it was invoked (via argv[0]) and routes to the appropriate command.
use anyhow::Result;
use daft::commands;
use daft::shortcuts;
use std::path::Path;

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

    // Handle --version/-V flag for the main daft/git-daft command
    if resolved == "daft" || resolved == "git-daft" {
        let args: Vec<String> = std::env::args().collect();
        if args.len() == 2 && (args[1] == "--version" || args[1] == "-V") {
            println!("daft {}", daft::VERSION);
            return Ok(());
        }
    }

    // Check for updates (reads cache, spawns background check if stale)
    let update_notification = daft::update_check::maybe_check_for_update();

    // Route to the appropriate command based on invocation name
    let result = match resolved {
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
        "git-worktree-flow-adopt" => commands::flow_adopt::run(),
        "git-worktree-flow-eject" => commands::flow_eject::run(),

        // Main daft / git-daft command - check for subcommands
        "git-daft" | "daft" => {
            let label = if resolved == "git-daft" {
                "git daft"
            } else {
                "daft"
            };
            // Check if a subcommand was provided
            let args: Vec<String> = std::env::args().collect();
            if args.len() > 1 {
                match args[1].as_str() {
                    "--help" | "-h" => commands::docs::run(),
                    "branch" => commands::branch::run(),
                    "completions" => commands::completions::run(),
                    "doctor" => commands::doctor::run(),
                    "__complete" => commands::complete::run(),
                    "__check-update" => {
                        let _ = daft::update_check::run_check_update();
                        return Ok(());
                    }
                    "hooks" => commands::hooks::run(),
                    "run" => {
                        // daft run <hook-name> [-- args...]
                        if args.len() < 3 {
                            eprintln!("Usage: daft run <hook-name> [-- args...]");
                            std::process::exit(1);
                        }
                        let hook_name = &args[2];
                        let hook_args: Vec<String> =
                            if let Some(pos) = args.iter().position(|a| a == "--") {
                                args[pos + 1..].to_vec()
                            } else {
                                args[3..].to_vec()
                            };
                        commands::hooks::run_hook(hook_name, &hook_args)
                    }
                    "multi-remote" => commands::multi_remote::run(),
                    "release-notes" => commands::release_notes::run(),
                    "setup" => {
                        // Check for setup subcommands
                        if args.len() > 2 && args[2] == "shortcuts" {
                            commands::shortcuts::run()
                        } else {
                            commands::setup::run()
                        }
                    }
                    "shell-init" => commands::shell_init::run(),
                    // Worktree commands accessible via `daft worktree-<command>`
                    "worktree-clone" => commands::clone::run(),
                    "worktree-init" => commands::init::run(),
                    "worktree-checkout" => commands::checkout::run(),
                    "worktree-checkout-branch" => commands::checkout_branch::run(),
                    "worktree-checkout-branch-from-default" => {
                        commands::checkout_branch_from_default::run()
                    }
                    "worktree-prune" => commands::prune::run(),
                    "worktree-carry" => commands::carry::run(),
                    "worktree-fetch" => commands::fetch::run(),
                    "worktree-flow-adopt" => commands::flow_adopt::run(),
                    "worktree-flow-eject" => commands::flow_eject::run(),
                    _ => daft::suggest::handle_unknown_subcommand(
                        label,
                        args[1].as_str(),
                        daft::suggest::DAFT_SUBCOMMANDS,
                    ),
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
    };

    // Show update notification after command output (if available)
    if let Some(ref notification) = update_notification {
        daft::update_check::print_notification(notification);
        daft::update_check::record_notification_shown(&notification.latest_version);
    }

    result
}
