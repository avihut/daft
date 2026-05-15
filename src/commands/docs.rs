/// Documentation command for `daft` and `git daft`
///
/// Shows daft commands in git-style help format, dynamically extracting
/// descriptions from clap command definitions. Renders a different command
/// surface depending on whether the binary is invoked as `daft` (daft-verb
/// style) or as `git daft` (Git `worktree-<command>` style).
use anyhow::Result;
use clap::{Command, CommandFactory};
use std::path::Path;

use crate::commands::{
    carry, checkout, clone, config, doctor, exec, fetch, flow_adopt, flow_eject, hooks, init,
    install, layout, list, merge, multi_remote, prune, release_notes, repo, shared, shell_init,
    shortcuts, sync, worktree_branch,
};
use crate::styles;

/// Invocation style determines which command surface to render.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Invoked as `daft` — render short daft-verb commands (go, start, list, ...).
    Daft,
    /// Invoked as `git daft` — render Git-style worktree-<command> entries.
    GitDaft,
}

/// How a category's commands are rendered.
enum CategoryLayout {
    /// One command per line with aligned `about` descriptions.
    List,
    /// Commands joined on a single line; `about` strings are omitted.
    Inline,
}

/// A category of commands with a title and list of commands.
struct CommandCategory {
    title: &'static str,
    commands: Vec<CommandEntry>,
    layout: CategoryLayout,
}

/// A single command entry with its display name and clap Command.
struct CommandEntry {
    display_name: &'static str,
    command: Command,
}

/// Get category layout for `daft` invocation — daft-verb style, everyday
/// commands at the top.
fn get_daft_categories() -> Vec<CommandCategory> {
    vec![
        CommandCategory {
            title: "work on branches (each branch gets its own directory)",
            layout: CategoryLayout::List,
            commands: vec![
                CommandEntry {
                    display_name: "go",
                    command: checkout::GoArgs::command(),
                },
                CommandEntry {
                    display_name: "start",
                    command: checkout::StartArgs::command(),
                },
            ],
        },
        CommandCategory {
            title: "maintain your worktrees",
            layout: CategoryLayout::List,
            commands: vec![
                CommandEntry {
                    display_name: "list",
                    command: list::Args::command(),
                },
                CommandEntry {
                    display_name: "rename",
                    command: worktree_branch::RenameArgs::command(),
                },
                CommandEntry {
                    display_name: "remove",
                    command: worktree_branch::RemoveArgs::command(),
                },
                CommandEntry {
                    display_name: "update",
                    command: fetch::Args::command(),
                },
                CommandEntry {
                    display_name: "prune",
                    command: prune::Args::command(),
                },
                CommandEntry {
                    display_name: "sync",
                    command: sync::Args::command(),
                },
            ],
        },
        CommandCategory {
            title: "share changes across worktrees",
            layout: CategoryLayout::List,
            commands: vec![
                CommandEntry {
                    display_name: "carry",
                    command: carry::Args::command(),
                },
                CommandEntry {
                    display_name: "merge",
                    command: merge::Args::command(),
                },
            ],
        },
        CommandCategory {
            title: "run commands across worktrees",
            layout: CategoryLayout::List,
            commands: vec![CommandEntry {
                display_name: "exec",
                command: exec::Args::command(),
            }],
        },
        CommandCategory {
            title: "start a worktree-based repository",
            layout: CategoryLayout::List,
            commands: vec![
                CommandEntry {
                    display_name: "clone",
                    command: clone::Args::command(),
                },
                CommandEntry {
                    display_name: "init",
                    command: init::Args::command(),
                },
            ],
        },
        CommandCategory {
            title: "manage repositories",
            layout: CategoryLayout::List,
            commands: vec![CommandEntry {
                display_name: "repo remove",
                command: repo::remove::Args::command(),
            }],
        },
        CommandCategory {
            title: "share configuration across worktrees",
            layout: CategoryLayout::List,
            commands: vec![CommandEntry {
                display_name: "shared",
                command: shared::Args::command(),
            }],
        },
        CommandCategory {
            title: "manage daft configuration",
            layout: CategoryLayout::Inline,
            commands: vec![
                CommandEntry {
                    display_name: "install",
                    command: install::Args::command(),
                },
                CommandEntry {
                    display_name: "config",
                    command: config::remote_sync::Args::command(),
                },
                CommandEntry {
                    display_name: "hooks",
                    command: hooks::Args::command(),
                },
                CommandEntry {
                    display_name: "layout",
                    command: layout::LayoutArgs::command(),
                },
                CommandEntry {
                    display_name: "multi-remote",
                    command: multi_remote::Args::command(),
                },
                CommandEntry {
                    display_name: "shell-init",
                    command: shell_init::Args::command(),
                },
                CommandEntry {
                    display_name: "setup shortcuts",
                    command: shortcuts::Args::command(),
                },
                CommandEntry {
                    display_name: "doctor",
                    command: doctor::Args::command(),
                },
                CommandEntry {
                    display_name: "release-notes",
                    command: release_notes::Args::command(),
                },
            ],
        },
    ]
}

/// Get category layout for `git daft` invocation — Git-style
/// `worktree-<command>` entries and the short-aliases footer.
fn get_git_daft_categories() -> Vec<CommandCategory> {
    vec![
        CommandCategory {
            title: "start a worktree-based repository",
            layout: CategoryLayout::List,
            commands: vec![
                CommandEntry {
                    display_name: "worktree-clone",
                    command: clone::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-init",
                    command: init::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-flow-adopt",
                    command: flow_adopt::Args::command(),
                },
            ],
        },
        CommandCategory {
            title: "work on branches (each branch gets its own directory)",
            layout: CategoryLayout::List,
            commands: vec![CommandEntry {
                display_name: "worktree-checkout",
                command: checkout::Args::command(),
            }],
        },
        CommandCategory {
            title: "share changes across worktrees",
            layout: CategoryLayout::List,
            commands: vec![
                CommandEntry {
                    display_name: "worktree-carry",
                    command: carry::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-merge",
                    command: merge::Args::command(),
                },
            ],
        },
        CommandCategory {
            title: "run commands across worktrees",
            layout: CategoryLayout::List,
            commands: vec![CommandEntry {
                display_name: "worktree-exec",
                command: exec::Args::command(),
            }],
        },
        CommandCategory {
            title: "maintain your worktrees",
            layout: CategoryLayout::List,
            commands: vec![
                CommandEntry {
                    display_name: "worktree-list",
                    command: list::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-branch",
                    command: worktree_branch::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-prune",
                    command: prune::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-fetch",
                    command: fetch::Args::command(),
                },
                CommandEntry {
                    display_name: "sync",
                    command: sync::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-flow-eject",
                    command: flow_eject::Args::command(),
                },
            ],
        },
        CommandCategory {
            title: "manage repositories",
            layout: CategoryLayout::List,
            commands: vec![CommandEntry {
                display_name: "daft repo remove",
                command: repo::remove::Args::command(),
            }],
        },
        CommandCategory {
            title: "share configuration across worktrees",
            layout: CategoryLayout::List,
            commands: vec![CommandEntry {
                display_name: "daft shared",
                command: shared::Args::command(),
            }],
        },
        CommandCategory {
            title: "manage daft configuration",
            layout: CategoryLayout::List,
            commands: vec![
                CommandEntry {
                    display_name: "daft install",
                    command: install::Args::command(),
                },
                CommandEntry {
                    display_name: "daft hooks",
                    command: hooks::Args::command(),
                },
                CommandEntry {
                    display_name: "daft layout",
                    command: layout::LayoutArgs::command(),
                },
                CommandEntry {
                    display_name: "daft multi-remote",
                    command: multi_remote::Args::command(),
                },
                CommandEntry {
                    display_name: "daft setup shortcuts",
                    command: shortcuts::Args::command(),
                },
                CommandEntry {
                    display_name: "daft shell-init",
                    command: shell_init::Args::command(),
                },
                CommandEntry {
                    display_name: "daft config",
                    command: config::remote_sync::Args::command(),
                },
                CommandEntry {
                    display_name: "daft doctor",
                    command: doctor::Args::command(),
                },
                CommandEntry {
                    display_name: "daft release-notes",
                    command: release_notes::Args::command(),
                },
            ],
        },
    ]
}

/// Extract the short description (about) from a clap Command.
fn get_about(cmd: &Command) -> String {
    cmd.get_about()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "(no description)".to_string())
}

/// Maximum display-name length across `List`-layout categories (used for
/// column alignment). `Inline` categories are skipped — their names are
/// joined on a single line and don't participate in column alignment.
fn max_list_display_name_len(categories: &[CommandCategory]) -> usize {
    categories
        .iter()
        .filter(|c| matches!(c.layout, CategoryLayout::List))
        .flat_map(|c| c.commands.iter())
        .map(|e| e.display_name.len())
        .max()
        .unwrap_or(20)
}

/// Wrap text in bold+underline (clap's `header`/`usage` style) when color is enabled.
fn bold_underline(text: &str, use_color: bool) -> String {
    if use_color {
        format!(
            "{}{}{}{}",
            styles::BOLD,
            styles::UNDERLINE,
            text,
            styles::RESET
        )
    } else {
        text.to_string()
    }
}

/// Wrap text in bold (clap's `literal` style) when color is enabled.
fn bold(text: &str, use_color: bool) -> String {
    if use_color {
        styles::bold(text)
    } else {
        text.to_string()
    }
}

pub fn run() -> Result<()> {
    // Detect how we were invoked
    let program_path = crate::cli::argv()
        .first()
        .cloned()
        .unwrap_or_else(|| "daft".to_string());
    let program_name = Path::new(&program_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("daft");

    let mode = if program_name == "git-daft" {
        Mode::GitDaft
    } else {
        Mode::Daft
    };

    match mode {
        Mode::Daft => render_daft(),
        Mode::GitDaft => render_git_daft(),
    }
}

/// Render daft-style help (invoked as `daft`): short verbs, everyday
/// commands first, clap-matching styling.
fn render_daft() -> Result<()> {
    let use_color = styles::colors_enabled();
    let categories = get_daft_categories();
    let max_len = max_list_display_name_len(&categories);

    println!(
        "{} daft <command> [<args>]",
        bold_underline("usage:", use_color)
    );

    println!();
    println!("These are common daft commands used in various situations:");

    for category in &categories {
        println!();
        println!("{}", bold_underline(category.title, use_color));

        match category.layout {
            CategoryLayout::List => {
                for entry in &category.commands {
                    let about = get_about(&entry.command);
                    // Pad from raw display_name length — ANSI escapes are
                    // zero-width visually but would otherwise skew `{:width$}`.
                    let pad = " ".repeat(max_len.saturating_sub(entry.display_name.len()));
                    let name = bold(entry.display_name, use_color);
                    println!("   {name}{pad}   {about}");
                }
            }
            CategoryLayout::Inline => {
                let names: Vec<String> = category
                    .commands
                    .iter()
                    .map(|e| bold(e.display_name, use_color))
                    .collect();
                println!("   {}", names.join(", "));
            }
        }
    }

    println!();
    println!("{}", bold_underline("Global options", use_color));
    println!(
        "   {}      Run as if started in <path>. Composes like git -C.",
        bold("-C <path>", use_color)
    );

    println!();
    println!(
        "'daft {} --help' to read about a specific command.",
        bold("<command>", use_color)
    );
    println!("Equivalent 'git worktree-<command>' forms also exist — run 'git daft' to see them.");
    println!("See https://github.com/avihut/daft for documentation.");

    Ok(())
}

/// Render Git-style help (invoked as `git daft`): `worktree-<command>`
/// entries and the short-aliases footer. Unstyled, like today.
fn render_git_daft() -> Result<()> {
    println!("usage: daft <command> [<args>]");
    println!("   or: git worktree-<command> [<args>]");
    println!("   or: daft worktree-<command> [<args>]");

    println!();
    println!("These are common daft commands used in various situations:");

    let categories = get_git_daft_categories();
    let max_len = max_list_display_name_len(&categories);

    for category in &categories {
        println!();
        println!("{}", category.title);

        match category.layout {
            CategoryLayout::List => {
                for entry in &category.commands {
                    let about = get_about(&entry.command);
                    println!(
                        "   {:width$}   {}",
                        entry.display_name,
                        about,
                        width = max_len
                    );
                }
            }
            CategoryLayout::Inline => {
                let names: Vec<&str> = category.commands.iter().map(|e| e.display_name).collect();
                println!("   {}", names.join(", "));
            }
        }
    }

    println!();
    println!("short aliases (daft <verb>)");
    println!("   go <branch>          Check out an existing branch worktree");
    println!("   start <branch>       Create a new branch worktree (-b)");
    println!(
        "   clone, init, carry, merge, update, list, prune, rename, sync, remove, adopt, eject"
    );

    println!();
    println!("Global options");
    println!("   -C <path>            Run as if started in <path>. Composes like git -C.");

    println!();
    println!("'git worktree-<command> --help' to read about a specific command.");
    println!("See https://github.com/avihut/daft for documentation.");

    Ok(())
}
