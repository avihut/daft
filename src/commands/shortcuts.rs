//! Shortcut management command for daft.
//!
//! This module provides the `daft setup shortcuts` command for managing
//! command shortcut symlinks.

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};

use crate::output::{CliOutput, Output, OutputConfig};
use crate::shortcuts::{shortcuts_for_style, ShortcutStyle, SHORTCUTS};

#[derive(Parser)]
#[command(name = "shortcuts")]
#[command(about = "Manage command shortcut symlinks")]
#[command(long_about = r#"
Manage shortcut symlinks for daft commands.

Shortcuts provide short aliases for frequently used commands:
  - Git style:    gwtclone, gwtco, gwtcb, gwtprune, gwtcarry, gwtfetch, gwtinit, gwtbd
  - Shell style:  gwco, gwcob
  - Legacy style: gclone, gcw, gcbw, gprune

Default-branch shortcuts (gwtcm, gwtcbm, gwcobd, gcbdw) are available
via shell integration only (daft shell-init).

Examples:
  daft setup shortcuts                    # Show current status
  daft setup shortcuts list               # List all shortcut styles
  daft setup shortcuts enable git         # Enable git-style shortcuts
  daft setup shortcuts disable legacy     # Disable legacy shortcuts
  daft setup shortcuts only shell         # Enable only shell shortcuts
"#)]
pub struct Args {
    #[command(subcommand)]
    command: Option<ShortcutsCommand>,
}

#[derive(Subcommand)]
enum ShortcutsCommand {
    /// List all shortcut styles and their aliases
    List,
    /// Show currently installed shortcuts (default)
    Status,
    /// Enable a shortcut style (creates symlinks)
    Enable {
        /// The style to enable (git, shell, or legacy)
        style: String,
        /// Override installation directory
        #[arg(long)]
        install_dir: Option<PathBuf>,
        /// Preview without making changes
        #[arg(long)]
        dry_run: bool,
    },
    /// Disable a shortcut style (removes symlinks)
    Disable {
        /// The style to disable (git, shell, or legacy)
        style: String,
        /// Override installation directory
        #[arg(long)]
        install_dir: Option<PathBuf>,
        /// Preview without making changes
        #[arg(long)]
        dry_run: bool,
    },
    /// Enable only the specified style (disables others)
    Only {
        /// The style to enable exclusively (git, shell, or legacy)
        style: String,
        /// Override installation directory
        #[arg(long)]
        install_dir: Option<PathBuf>,
        /// Preview without making changes
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn run() -> Result<()> {
    // Skip "daft", "setup", and "shortcuts" from args
    let args: Vec<String> = std::env::args().skip(3).collect();
    let args = Args::parse_from(std::iter::once("shortcuts".to_string()).chain(args));
    let mut output = CliOutput::new(OutputConfig::new(false, false));

    match args.command {
        None | Some(ShortcutsCommand::Status) => cmd_status(&mut output),
        Some(ShortcutsCommand::List) => cmd_list(&mut output),
        Some(ShortcutsCommand::Enable {
            style,
            install_dir,
            dry_run,
        }) => cmd_enable(&style, install_dir, dry_run, &mut output),
        Some(ShortcutsCommand::Disable {
            style,
            install_dir,
            dry_run,
        }) => cmd_disable(&style, install_dir, dry_run, &mut output),
        Some(ShortcutsCommand::Only {
            style,
            install_dir,
            dry_run,
        }) => cmd_only(&style, install_dir, dry_run, &mut output),
    }
}

/// Detect the installation directory by finding where the daft binary is located.
pub fn detect_install_dir() -> Result<PathBuf> {
    let exe_path =
        std::env::current_exe().context("Failed to determine current executable path")?;
    let install_dir = exe_path
        .parent()
        .ok_or_else(|| anyhow!("Failed to determine installation directory"))?;
    Ok(install_dir.to_path_buf())
}

/// Get the path to the daft binary in the given directory.
fn get_daft_binary(install_dir: &Path) -> PathBuf {
    install_dir.join("daft")
}

/// Check if a path is a symlink pointing to the daft binary.
fn is_daft_symlink(path: &Path, install_dir: &Path) -> bool {
    if !path.is_symlink() {
        return false;
    }

    if let Ok(target) = fs::read_link(path) {
        let daft_binary = get_daft_binary(install_dir);
        // Check if target matches daft (absolute or relative)
        target == daft_binary || target == Path::new("daft")
    } else {
        false
    }
}

/// Get installed shortcuts in the given directory.
fn get_installed_shortcuts(install_dir: &Path) -> Vec<(&'static str, ShortcutStyle)> {
    let mut installed = Vec::new();

    for shortcut in SHORTCUTS {
        let path = install_dir.join(shortcut.alias);
        if is_daft_symlink(&path, install_dir) {
            installed.push((shortcut.alias, shortcut.style));
        }
    }

    installed
}

/// Create a symlink for a shortcut.
fn create_symlink(
    alias: &str,
    install_dir: &Path,
    dry_run: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let link_path = install_dir.join(alias);

    if dry_run {
        output.info(&format!("  Would create: {alias} -> daft"));
        return Ok(());
    }

    // Remove existing file/symlink if present
    if link_path.exists() || link_path.is_symlink() {
        fs::remove_file(&link_path)
            .with_context(|| format!("Failed to remove existing {alias}"))?;
    }

    // Create symlink (using relative path for portability)
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("daft", &link_path)
            .with_context(|| format!("Failed to create symlink for {alias}"))?;
    }

    #[cfg(not(unix))]
    {
        // On non-Unix, copy the binary instead
        let daft_binary = get_daft_binary(install_dir);
        fs::copy(&daft_binary, &link_path)
            .with_context(|| format!("Failed to copy daft binary to {alias}"))?;
    }

    output.step(&format!("  Created: {alias} -> daft"));
    Ok(())
}

/// Remove a shortcut symlink.
fn remove_symlink(
    alias: &str,
    install_dir: &Path,
    dry_run: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let link_path = install_dir.join(alias);

    if !link_path.exists() && !link_path.is_symlink() {
        return Ok(()); // Already doesn't exist
    }

    if dry_run {
        output.info(&format!("  Would remove: {alias}"));
        return Ok(());
    }

    fs::remove_file(&link_path).with_context(|| format!("Failed to remove {alias}"))?;

    output.step(&format!("  Removed: {alias}"));
    Ok(())
}

/// Check write permission to a directory.
fn check_write_permission(dir: &Path) -> Result<()> {
    let test_file = dir.join(".daft-write-test");
    fs::write(&test_file, "test")
        .with_context(|| format!("Cannot write to {}. Check permissions.", dir.display()))?;
    fs::remove_file(&test_file).ok();
    Ok(())
}

/// Show status of installed shortcuts.
fn cmd_status(output: &mut dyn Output) -> Result<()> {
    let install_dir = detect_install_dir()?;
    let installed = get_installed_shortcuts(&install_dir);

    output.info(&format!(
        "Installation directory: {}",
        install_dir.display()
    ));
    output.info("");

    if installed.is_empty() {
        output.info("No shortcuts currently installed.");
        output.info("");
        output.info("Enable shortcuts with:");
        output.info("  daft setup shortcuts enable git     # Enable git-style shortcuts");
        output.info("  daft setup shortcuts enable shell   # Enable shell-style shortcuts");
        output.info("  daft setup shortcuts enable legacy  # Enable legacy shortcuts");
    } else {
        output.info("Installed shortcuts:");
        output.info("");

        // Group by style
        for style in ShortcutStyle::all() {
            let style_shortcuts: Vec<_> = installed.iter().filter(|(_, s)| s == style).collect();
            if !style_shortcuts.is_empty() {
                output.info(&format!("  {} style:", style.name()));
                for (alias, _) in style_shortcuts {
                    output.info(&format!("    {}", alias));
                }
            }
        }
    }

    Ok(())
}

/// List all available shortcut styles and their aliases.
fn cmd_list(output: &mut dyn Output) -> Result<()> {
    output.info("Available shortcut styles:");
    output.info("");

    for style in ShortcutStyle::all() {
        output.info(&format!("{} - {}", style.name(), style.description()));
        output.info("");

        let shortcuts = shortcuts_for_style(*style);
        output.info(&format!("  {:20} {:40}", "SHORTCUT", "COMMAND"));
        output.info(&format!("  {:20} {:40}", "--------", "-------"));
        for shortcut in shortcuts {
            output.info(&format!("  {:20} {}", shortcut.alias, shortcut.command));
        }
        output.info("");
    }

    Ok(())
}

/// Enable shortcuts for a style.
fn cmd_enable(
    style_name: &str,
    install_dir: Option<PathBuf>,
    dry_run: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let style: ShortcutStyle = style_name
        .parse()
        .map_err(|_| anyhow!("Unknown style: {}. Use: git, shell, or legacy", style_name))?;

    let install_dir = install_dir.map(Ok).unwrap_or_else(detect_install_dir)?;

    enable_style(style, &install_dir, dry_run, output)
}

/// Enable shortcuts for a given style. Public API for use by other commands.
pub fn enable_style(
    style: ShortcutStyle,
    install_dir: &Path,
    dry_run: bool,
    output: &mut dyn Output,
) -> Result<()> {
    if !dry_run {
        check_write_permission(install_dir)?;
    }

    let shortcuts = shortcuts_for_style(style);

    if dry_run {
        output.info(&format!(
            "[dry-run] Would enable {} style shortcuts:",
            style.name()
        ));
    } else {
        output.info(&format!("Enabling {} style shortcuts:", style.name()));
    }
    output.info("");

    for shortcut in shortcuts {
        create_symlink(shortcut.alias, install_dir, dry_run, output)?;
    }

    if !dry_run {
        output.info("");
        output.result(&format!("Done! {} shortcuts enabled.", style.name()));
    }

    Ok(())
}

/// Disable shortcuts for a style.
fn cmd_disable(
    style_name: &str,
    install_dir: Option<PathBuf>,
    dry_run: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let style: ShortcutStyle = style_name
        .parse()
        .map_err(|_| anyhow!("Unknown style: {}. Use: git, shell, or legacy", style_name))?;

    let install_dir = install_dir.map(Ok).unwrap_or_else(detect_install_dir)?;

    if !dry_run {
        check_write_permission(&install_dir)?;
    }

    let shortcuts = shortcuts_for_style(style);

    if dry_run {
        output.info(&format!(
            "[dry-run] Would disable {} style shortcuts:",
            style.name()
        ));
    } else {
        output.info(&format!("Disabling {} style shortcuts:", style.name()));
    }
    output.info("");

    for shortcut in shortcuts {
        remove_symlink(shortcut.alias, &install_dir, dry_run, output)?;
    }

    if !dry_run {
        output.info("");
        output.result(&format!("Done! {} shortcuts disabled.", style.name()));
    }

    Ok(())
}

/// Enable only the specified style (disable all others).
fn cmd_only(
    style_name: &str,
    install_dir: Option<PathBuf>,
    dry_run: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let style: ShortcutStyle = style_name
        .parse()
        .map_err(|_| anyhow!("Unknown style: {}. Use: git, shell, or legacy", style_name))?;

    let install_dir = install_dir.map(Ok).unwrap_or_else(detect_install_dir)?;

    if !dry_run {
        check_write_permission(&install_dir)?;
    }

    if dry_run {
        output.info(&format!(
            "[dry-run] Would enable only {} style shortcuts:",
            style.name()
        ));
    } else {
        output.info(&format!("Enabling only {} style shortcuts:", style.name()));
    }
    output.info("");

    // Remove all other styles
    for other_style in ShortcutStyle::all() {
        if *other_style != style {
            let shortcuts = shortcuts_for_style(*other_style);
            for shortcut in shortcuts {
                let path = install_dir.join(shortcut.alias);
                if is_daft_symlink(&path, &install_dir) {
                    remove_symlink(shortcut.alias, &install_dir, dry_run, output)?;
                }
            }
        }
    }

    // Enable the requested style
    let shortcuts = shortcuts_for_style(style);
    for shortcut in shortcuts {
        create_symlink(shortcut.alias, &install_dir, dry_run, output)?;
    }

    if !dry_run {
        output.info("");
        output.result(&format!(
            "Done! Only {} shortcuts are now enabled.",
            style.name()
        ));
    }

    Ok(())
}
