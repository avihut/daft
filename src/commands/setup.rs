use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

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
  daft setup --yes        # Skip confirmation prompt
  daft setup --dry-run    # Show what would be done without making changes
"#)]
pub struct Args {
    #[arg(short = 'y', long, help = "Skip confirmation prompt")]
    yes: bool,

    #[arg(long, help = "Show what would be done without making changes")]
    dry_run: bool,

    #[arg(long, help = "Force setup even if already configured")]
    force: bool,
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse_from(args);

    // Detect shell
    let shell = Shell::from_env()
        .ok_or_else(|| anyhow!("Could not detect shell from $SHELL environment variable.\nSupported shells: bash, zsh, fish"))?;

    let config_file = shell.config_file();
    let init_line = shell.init_line();

    println!("Detected shell: {}", shell.name());
    println!("Config file: {}", config_file.display());
    println!();

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
        println!(
            "daft shell integration is already configured in {}.",
            config_file.display()
        );
        println!("Use --force to add it again anyway.");
        return Ok(());
    }

    // Show what will be added
    println!("Will append to {}:", config_file.display());
    println!();
    println!("  # daft shell integration - enables cd into new worktrees");
    println!("  {init_line}");
    println!();

    if args.dry_run {
        println!("[dry-run] No changes made.");
        return Ok(());
    }

    // Confirm unless --yes
    if !args.yes {
        print!("Proceed? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Create backup if file exists
    if config_exists {
        let backup_path = config_file.with_extension("bak");
        fs::copy(&config_file, &backup_path)
            .with_context(|| format!("Failed to create backup at {}", backup_path.display()))?;
        println!("Created backup: {}", backup_path.display());
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

    println!();
    println!("Done! Shell integration added to {}", config_file.display());
    println!();
    println!("To activate, either:");
    println!("  1. Restart your terminal, or");
    println!("  2. Run: source {}", config_file.display());

    Ok(())
}
