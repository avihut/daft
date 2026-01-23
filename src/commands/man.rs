/// Man page generation for daft commands
///
/// Generates man pages from clap command definitions using clap_mangen.
/// Man pages can be output to stdout or written to files for installation.
use anyhow::{Context, Result};
use clap::{Command, CommandFactory, Parser};
use clap_mangen::Man;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// Available daft commands that need man pages
const COMMANDS: &[&str] = &[
    "git-worktree-clone",
    "git-worktree-init",
    "git-worktree-checkout",
    "git-worktree-checkout-branch",
    "git-worktree-checkout-branch-from-default",
    "git-worktree-prune",
    "git-worktree-carry",
    "git-worktree-fetch",
];

/// Get the clap Command for a given command name by using CommandFactory
fn get_command_for_name(command_name: &str) -> Option<Command> {
    match command_name {
        "git-worktree-clone" => Some(crate::commands::clone::Args::command()),
        "git-worktree-init" => Some(crate::commands::init::Args::command()),
        "git-worktree-checkout" => Some(crate::commands::checkout::Args::command()),
        "git-worktree-checkout-branch" => Some(crate::commands::checkout_branch::Args::command()),
        "git-worktree-checkout-branch-from-default" => {
            Some(crate::commands::checkout_branch_from_default::Args::command())
        }
        "git-worktree-prune" => Some(crate::commands::prune::Args::command()),
        "git-worktree-carry" => Some(crate::commands::carry::Args::command()),
        "git-worktree-fetch" => Some(crate::commands::fetch::Args::command()),
        _ => None,
    }
}

#[derive(Parser)]
#[command(name = "daft-man")]
#[command(about = "Generate man pages for daft commands")]
struct Args {
    #[arg(
        short,
        long,
        help = "Specific command to generate man page for (default: all commands)"
    )]
    command: Option<String>,

    #[arg(
        short,
        long,
        help = "Output directory for man pages (default: print to stdout)"
    )]
    output_dir: Option<PathBuf>,

    #[arg(short, long, help = "Install man pages to standard system location")]
    install: bool,
}

pub fn run() -> Result<()> {
    // When called as a subcommand, skip "daft" and "man" from args
    let mut args_vec: Vec<String> = std::env::args().collect();

    // If args start with [daft, man, ...], keep only [daft, ...]
    // to make clap parse correctly
    if args_vec.len() >= 2 && args_vec[1] == "man" {
        args_vec.remove(1); // Remove "man", keep "daft" for clap
    }

    let args = Args::parse_from(&args_vec);

    if args.install {
        install_man_pages()?;
    } else if let Some(output_dir) = args.output_dir {
        generate_man_pages_to_dir(&output_dir, args.command.as_deref())?;
    } else if let Some(command) = args.command {
        generate_man_page_to_stdout(&command)?;
    } else {
        // Generate all to stdout, separated by form feeds
        for (i, command) in COMMANDS.iter().enumerate() {
            if i > 0 {
                println!("\n\x0c\n"); // Form feed separator
            }
            generate_man_page_to_stdout(command)?;
        }
    }

    Ok(())
}

/// Generate a man page for a specific command and print to stdout
fn generate_man_page_to_stdout(command_name: &str) -> Result<()> {
    let cmd =
        get_command_for_name(command_name).context(format!("Unknown command: {command_name}"))?;

    let man = Man::new(cmd);
    let mut buffer = Vec::new();
    man.render(&mut buffer)?;

    std::io::stdout().write_all(&buffer)?;
    Ok(())
}

/// Generate man pages and write to a directory
fn generate_man_pages_to_dir(output_dir: &PathBuf, command: Option<&str>) -> Result<()> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    let commands_to_generate: Vec<&str> = if let Some(cmd) = command {
        vec![cmd]
    } else {
        COMMANDS.to_vec()
    };

    for command_name in commands_to_generate {
        let cmd = get_command_for_name(command_name)
            .context(format!("Unknown command: {command_name}"))?;

        let man = Man::new(cmd);
        let mut buffer = Vec::new();
        man.render(&mut buffer)?;

        let filename = format!("{command_name}.1");
        let file_path = output_dir.join(&filename);

        fs::write(&file_path, &buffer)
            .with_context(|| format!("Failed to write man page: {}", file_path.display()))?;

        eprintln!("Generated: {}", file_path.display());
    }

    eprintln!("\nMan pages generated in: {}", output_dir.display());
    Ok(())
}

/// Install man pages to standard system location
fn install_man_pages() -> Result<()> {
    let install_dir = get_man_install_dir()?;

    fs::create_dir_all(&install_dir)
        .with_context(|| format!("Failed to create man directory: {}", install_dir.display()))?;

    eprintln!("Installing man pages to: {}", install_dir.display());

    for command_name in COMMANDS {
        let cmd = get_command_for_name(command_name)
            .context(format!("Unknown command: {command_name}"))?;

        let man = Man::new(cmd);
        let mut buffer = Vec::new();
        man.render(&mut buffer)?;

        let filename = format!("{command_name}.1");
        let file_path = install_dir.join(&filename);

        fs::write(&file_path, &buffer)
            .with_context(|| format!("Failed to write man page: {}", file_path.display()))?;

        eprintln!("  Installed: {filename}");
    }

    eprintln!("\nMan pages installed successfully!");
    eprintln!("\nYou can now use:");
    for command_name in COMMANDS {
        eprintln!("  man {command_name}");
    }

    Ok(())
}

/// Get the standard man page installation directory
fn get_man_install_dir() -> Result<PathBuf> {
    // Try user-local first, fallback to system location
    let home = dirs::home_dir().context("Could not determine home directory")?;

    // Use ~/.local/share/man/man1 for user-local installation
    let user_dir = home.join(".local/share/man/man1");

    // If we can write to /usr/local/share/man/man1, prefer that
    let system_dir = PathBuf::from("/usr/local/share/man/man1");

    // Prefer user directory to avoid needing sudo
    if user_dir.exists() || fs::create_dir_all(&user_dir).is_ok() {
        Ok(user_dir)
    } else if system_dir.exists() {
        Ok(system_dir)
    } else {
        Ok(user_dir) // Default to user dir, will create it
    }
}
