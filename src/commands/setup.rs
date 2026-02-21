use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use crate::commands::shortcuts::{detect_install_dir, enable_style};
use crate::output::{CliOutput, Output, OutputConfig};
use crate::shortcuts::ShortcutStyle;

#[derive(Debug, Clone, PartialEq)]
enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    fn from_env() -> Option<Self> {
        let shell_path = env::var("SHELL").ok()?;
        if shell_path.contains("zsh") {
            Some(Shell::Zsh)
        } else if shell_path.contains("bash") {
            Some(Shell::Bash)
        } else if shell_path.contains("fish") {
            Some(Shell::Fish)
        } else {
            None
        }
    }

    fn config_file(&self) -> PathBuf {
        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
        match self {
            Shell::Bash => PathBuf::from(home).join(".bashrc"),
            Shell::Zsh => PathBuf::from(home).join(".zshrc"),
            Shell::Fish => PathBuf::from(home)
                .join(".config")
                .join("fish")
                .join("config.fish"),
        }
    }

    fn init_line(&self) -> &'static str {
        match self {
            Shell::Bash => r#"eval "$(daft shell-init bash)""#,
            Shell::Zsh => r#"eval "$(daft shell-init zsh)""#,
            Shell::Fish => "daft shell-init fish | source",
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
        }
    }
}

#[derive(Parser)]
#[command(name = "setup")]
#[command(about = "Add daft shell integration to your shell config")]
#[command(long_about = r#"
Automatically adds the daft shell-init line to your shell configuration file.

This enables automatic cd into new worktrees when using daft commands.

The command will:
  1. Detect your shell (bash, zsh, or fish)
  2. Find the appropriate config file (~/.bashrc, ~/.zshrc, or ~/.config/fish/config.fish)
  3. Check if daft is already configured (won't add duplicates)
  4. Create a backup of your config file
  5. Append the shell-init line

Examples:
  daft setup              # Interactive setup with confirmation
  daft setup --force      # Skip confirmation and re-add if already configured
  daft setup --dry-run    # Show what would be done without making changes
"#)]
pub struct Args {
    #[arg(
        short = 'f',
        long,
        help = "Skip confirmation and re-add if already configured"
    )]
    force: bool,

    #[arg(long, help = "Show what would be done without making changes")]
    dry_run: bool,
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse_from(args);
    let mut output = CliOutput::new(OutputConfig::new(false, false));

    // Detect shell
    let shell = Shell::from_env()
        .ok_or_else(|| anyhow!("Could not detect shell from $SHELL environment variable.\nSupported shells: bash, zsh, fish"))?;

    let config_file = shell.config_file();
    let init_line = shell.init_line();

    output.info(&format!("Detected shell: {}", shell.name()));
    output.info(&format!("Config file: {}", config_file.display()));
    output.info("");

    // Check if config file exists
    let config_exists = config_file.exists();
    let current_content = if config_exists {
        fs::read_to_string(&config_file)
            .with_context(|| format!("Failed to read {}", config_file.display()))?
    } else {
        String::new()
    };

    // Check if already configured
    let already_configured = current_content.contains("daft shell-init");
    if already_configured && !args.force {
        output.info(&format!(
            "daft shell integration is already configured in {}.",
            config_file.display()
        ));
        output.info("");
        output.info("If commands aren't working, activate it by either:");
        output.info("  1. Restarting your terminal, or");
        output.info(&format!("  2. Running: source {}", config_file.display()));
        output.info("");
        output.info("Use --force to add it again anyway.");
        return Ok(());
    }

    // Show what will be added
    output.info(&format!("Will append to {}:", config_file.display()));
    output.info("");
    output.info("  # daft shell integration - enables cd into new worktrees");
    output.info(&format!("  {init_line}"));
    output.info("");

    if args.dry_run {
        output.info("[dry-run] No changes made.");
        return Ok(());
    }

    // Confirm unless --force
    if !args.force {
        print!("Proceed? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            output.info("Aborted.");
            return Ok(());
        }
    }

    // Create backup if file exists
    if config_exists {
        let backup_path = config_file.with_extension("bak");
        fs::copy(&config_file, &backup_path)
            .with_context(|| format!("Failed to create backup at {}", backup_path.display()))?;
        output.info(&format!("Created backup: {}", backup_path.display()));
    }

    // Ensure parent directory exists (for fish)
    if let Some(parent) = config_file.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }
    }

    // Append the init line
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config_file)
        .with_context(|| format!("Failed to open {} for writing", config_file.display()))?;

    // Add newline before if file doesn't end with one
    let needs_newline = !current_content.is_empty() && !current_content.ends_with('\n');
    if needs_newline {
        writeln!(file)?;
    }

    writeln!(file)?;
    writeln!(
        file,
        "# daft shell integration - enables cd into new worktrees"
    )?;
    writeln!(file, "{init_line}")?;

    // Install git-style shortcuts silently
    let shortcuts_installed = if let Ok(install_dir) = detect_install_dir() {
        let mut quiet_output = CliOutput::new(OutputConfig::new(true, false));
        enable_style(ShortcutStyle::Git, &install_dir, false, &mut quiet_output).is_ok()
    } else {
        false
    };

    output.info("");
    output.result(&format!(
        "Done! Shell integration added to {}",
        config_file.display()
    ));
    if shortcuts_installed {
        output.info("      Git-style shortcuts installed (gwtco, gwtcb, etc.)");
    }
    output.info("");
    output.info("To activate, either:");
    output.info("  1. Restart your terminal, or");
    output.info(&format!("  2. Run: source {}", config_file.display()));
    if shortcuts_installed {
        output.info("");
        output.info("To use different shortcut styles:");
        output.info("  daft setup shortcuts only shell   # gwco, gwcob, gwcobd");
        output.info("  daft setup shortcuts only legacy  # gclone, gcw, gcbw, ...");
    }

    Ok(())
}
