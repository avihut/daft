use super::find_worktree_root;
use crate::hooks::{yaml_config_loader, yaml_config_validate};
use crate::styles::{dim, green, red, yellow};
use anyhow::{Context, Result};

/// Validate the YAML hooks configuration.
pub(super) fn cmd_validate() -> Result<()> {
    let worktree_root = find_worktree_root()?;

    let config = yaml_config_loader::load_merged_config(&worktree_root)
        .context("Failed to load YAML config")?;

    let config = match config {
        Some(c) => c,
        None => {
            println!("{}", dim("No daft.yml found."));
            return Ok(());
        }
    };

    let result = yaml_config_validate::validate_config(&config)?;

    for warning in &result.warnings {
        println!("  {} {warning}", yellow("warning:"));
    }

    for error in &result.errors {
        println!("  {} {error}", red("error:"));
    }

    if result.is_ok() {
        if result.warnings.is_empty() {
            println!("{}", green("Configuration is valid."));
        } else {
            println!(
                "\n{} ({} warning(s))",
                green("Configuration is valid"),
                result.warnings.len()
            );
        }
        Ok(())
    } else {
        println!(
            "\n{} ({} error(s), {} warning(s))",
            red("Configuration has errors"),
            result.errors.len(),
            result.warnings.len()
        );
        std::process::exit(1);
    }
}
