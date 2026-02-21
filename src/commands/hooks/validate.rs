use super::find_worktree_root;
use crate::hooks::{yaml_config_loader, yaml_config_validate};
use crate::output::Output;
use crate::styles::{dim, green, red};
use anyhow::{Context, Result};

/// Validate the YAML hooks configuration.
pub(super) fn cmd_validate(output: &mut dyn Output) -> Result<()> {
    let worktree_root = find_worktree_root()?;

    let config = yaml_config_loader::load_merged_config(&worktree_root)
        .context("Failed to load YAML config")?;

    let config = match config {
        Some(c) => c,
        None => {
            output.info(&dim("No daft.yml found."));
            return Ok(());
        }
    };

    let result = yaml_config_validate::validate_config(&config)?;

    for warning in &result.warnings {
        output.warning(&warning.to_string());
    }

    for error in &result.errors {
        output.error(&error.to_string());
    }

    if result.is_ok() {
        if result.warnings.is_empty() {
            output.success(&green("Configuration is valid."));
        } else {
            output.success(&format!(
                "\n{} ({} warning(s))",
                green("Configuration is valid"),
                result.warnings.len()
            ));
        }
        Ok(())
    } else {
        output.error(&format!(
            "{} ({} error(s), {} warning(s))",
            red("Configuration has errors"),
            result.errors.len(),
            result.warnings.len()
        ));
        std::process::exit(1);
    }
}
