//! daft - Git Extensions Toolkit
//!
//! A multicall binary that provides Git extensions through symlinks.
//! Detects how it was invoked (via argv[0]) and routes to the appropriate command.

#![forbid(unsafe_code)]

use anyhow::Result;
use daft::commands;
use daft::shortcuts;
use std::path::Path;

fn main() -> Result<()> {
    // Parse and apply top-level flags (currently just `-C <path>`) before any
    // other work. This MUST happen before `should_skip_background_tasks` below
    // — its argv-side gate (`skip_startup_tasks_for`) inspects argv[1] to detect
    // `shell-init`/`__*` invocations that must skip background spawns, and if
    // `-C` is still in argv at that point the gate fails open (see the
    // fork-bomb warning further down).
    let raw_argv: Vec<String> = std::env::args().collect();
    daft::cli::install_and_apply(raw_argv)?;
    let argv = daft::cli::argv();

    // Detect how we were invoked by checking argv[0]
    let program_path = argv.first().cloned().unwrap_or_else(|| "daft".to_string());
    let program_name = Path::new(&program_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("daft");

    // Resolve shortcut aliases to full command names
    let resolved = shortcuts::resolve(program_name);

    // Handle --version/-V flag for the main daft/git-daft command
    if (resolved == "daft" || resolved == "git-daft")
        && argv.len() == 2
        && (argv[1] == "--version" || argv[1] == "-V")
    {
        println!("daft {}", daft::VERSION_DISPLAY);
        return Ok(());
    }

    // Skip startup-time background work for invocations that must stay lean.
    // Three independent gates compose here; see
    // `daft::should_skip_background_tasks` for the per-gate rationale.
    //
    // If a fork bomb occurs (hundreds of daft processes consuming all CPU):
    //   pkill -9 -f "daft.*__check-update"
    //   pkill -9 -f "daft.*__prune-trust"
    // Or more broadly:
    //   pkill -9 -f "<worktree-path>/target/release"
    // Repeat until `ps aux | grep __check-update | wc -l` returns 0.
    let skip_background = daft::should_skip_background_tasks(argv);

    // Warn if config directory is overridden (security measure against trust DB hijacking).
    // Only in dev builds — release builds ignore DAFT_CONFIG_DIR entirely.
    if cfg!(daft_dev_build)
        && let Ok(dir) = std::env::var(daft::CONFIG_DIR_ENV)
        && !dir.is_empty()
        && !skip_background
    {
        eprintln!("warning: config directory overridden via DAFT_CONFIG_DIR");
        eprintln!("  -> {dir}");
    }

    // Check for updates (reads cache, spawns background check if stale)
    let update_notification = if !skip_background {
        daft::update_check::maybe_check_for_update()
    } else {
        None
    };

    // Prune stale trust entries (background, once per 24h)
    if !skip_background {
        daft::trust_prune::maybe_prune_trust();
    }

    // Clean up hook job logs (background, once per 24h)
    if !skip_background {
        daft::log_clean::maybe_clean_logs();
    }

    // Route to the appropriate command based on invocation name
    let result = match resolved {
        // Git worktree extension commands (via symlinks)
        "git-worktree-clone" => commands::clone::run(),
        "git-worktree-init" => commands::init::run(),
        "git-worktree-checkout" => commands::checkout::run(),

        "git-worktree-prune" => commands::prune::run(),
        "git-worktree-carry" => commands::carry::run(),
        "git-worktree-branch" => commands::worktree_branch::run(),
        "git-worktree-branch-delete" => commands::branch_delete::run(),
        "git-worktree-fetch" => commands::fetch::run(),
        "git-worktree-flow-adopt" => commands::flow_adopt::run(),
        "git-worktree-flow-eject" => commands::flow_eject::run(),
        "git-worktree-list" => commands::list::run(),
        "git-worktree-merge" => commands::merge::run(),
        "git-worktree-sync" => commands::sync::run(),
        "git-worktree-exec" => commands::exec::run(),

        // Daft-style commands (via symlinks)
        "daft-go" => commands::checkout::run_go(),
        "daft-start" => commands::checkout::run_start(),
        "daft-remove" => commands::worktree_branch::run_remove(),
        "daft-rename" => commands::worktree_branch::run_rename(),

        // Main daft / git-daft command - check for subcommands
        "git-daft" | "daft" => {
            let label = if resolved == "git-daft" {
                "git daft"
            } else {
                "daft"
            };
            // Check if a subcommand was provided
            let args = argv;
            if args.len() > 1 {
                match args[1].as_str() {
                    "--help" | "-h" => commands::docs::run(),
                    "completions" => commands::completions::run(),
                    "doctor" => commands::doctor::run(),
                    "__complete" => commands::complete::run(),
                    "__check-update" => {
                        let _ = daft::update_check::run_check_update();
                        return Ok(());
                    }
                    "__prune-trust" => {
                        let _ = daft::trust_prune::run_prune_trust();
                        return Ok(());
                    }
                    "__clean-logs" => {
                        let _ = daft::log_clean::run_clean_logs();
                        return Ok(());
                    }
                    "__dump-store" => {
                        if let Err(e) = commands::dump_store::run() {
                            eprintln!("daft __dump-store: {e:#}");
                            std::process::exit(1);
                        }
                        return Ok(());
                    }
                    #[cfg(unix)]
                    "__coordinator" => {
                        // Internal: spawned by `spawn_coordinator()`. argv[2]
                        // is the path to a JSON-serialized CoordinatorPayload.
                        let Some(state_file) = args.get(2) else {
                            eprintln!("daft: __coordinator requires a state file path");
                            std::process::exit(2);
                        };
                        let path = std::path::PathBuf::from(state_file);
                        // Surface startup errors with a non-zero exit code.
                        // The parent's spawn redirects stderr to /dev/null,
                        // so the eprintln only matters for direct debugging
                        // (`daft __coordinator <path>`); the exit code is
                        // observable to anyone wrapping this command.
                        if let Err(e) = daft::coordinator::process::run_coordinator(&path) {
                            eprintln!("daft coordinator: startup failed: {e:#}");
                            std::process::exit(1);
                        }
                        return Ok(());
                    }
                    "config" => commands::config::run(),
                    "hooks" => commands::hooks::run(),
                    "install" => commands::install::run(),
                    "layout" => commands::layout::run(),
                    "multi-remote" => commands::multi_remote::run(),
                    "shared" => commands::shared::run(),
                    "release-notes" => commands::release_notes::run(),
                    "repo" => commands::repo::run(),
                    "setup" => {
                        // Check for setup subcommands
                        if args.len() > 2 && args[2] == "shortcuts" {
                            commands::shortcuts::run()
                        } else {
                            commands::setup::run()
                        }
                    }
                    "shell-init" => commands::shell_init::run(),
                    // Daft verb aliases (short names)
                    "clone" => commands::clone::run(),
                    "init" => commands::init::run(),
                    "go" => commands::checkout::run_go(),
                    "start" => commands::checkout::run_start(),
                    "carry" => commands::carry::run(),
                    "update" => commands::fetch::run(),
                    "prune" => commands::prune::run(),
                    "rename" => commands::worktree_branch::run_rename(),
                    "sync" => commands::sync::run(),
                    "list" => commands::list::run(),
                    "merge" => commands::merge::run(),
                    "remove" => commands::worktree_branch::run_remove(),
                    "adopt" => commands::flow_adopt::run(),
                    "eject" => commands::flow_eject::run(),
                    "exec" => commands::exec::run(),
                    // Worktree commands accessible via `daft worktree-<command>`
                    "worktree-clone" => commands::clone::run(),
                    "worktree-init" => commands::init::run(),
                    "worktree-checkout" => commands::checkout::run(),

                    "worktree-prune" => commands::prune::run(),
                    "worktree-carry" => commands::carry::run(),
                    "worktree-fetch" => commands::fetch::run(),
                    "worktree-flow-adopt" => commands::flow_adopt::run(),
                    "worktree-branch" => commands::worktree_branch::run(),
                    "worktree-branch-delete" => commands::branch_delete::run(),
                    "worktree-flow-eject" => commands::flow_eject::run(),
                    "worktree-list" => commands::list::run(),
                    "worktree-merge" => commands::merge::run(),
                    "worktree-sync" => commands::sync::run(),
                    "worktree-exec" => commands::exec::run(),
                    "worktree-shared" => commands::shared::run(),
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
