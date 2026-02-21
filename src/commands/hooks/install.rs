use super::find_worktree_root;
use crate::hooks::{yaml_config, yaml_config_loader};
use crate::output::Output;
use crate::styles::{bold, cyan, dim, green};
use anyhow::{Context, Result};

/// Scaffold a daft.yml configuration with hook definitions.
pub(super) fn cmd_install(hooks: &[String], output: &mut dyn Output) -> Result<()> {
    let worktree_root = find_worktree_root()?;

    // Determine which hooks to scaffold
    let hook_names: Vec<&str> = if hooks.is_empty() {
        yaml_config::KNOWN_HOOK_NAMES.to_vec()
    } else {
        // Validate all provided names
        for name in hooks {
            if !yaml_config::KNOWN_HOOK_NAMES.contains(&name.as_str()) {
                anyhow::bail!(
                    "Unknown hook name: '{name}'. Valid hooks: {}",
                    yaml_config::KNOWN_HOOK_NAMES.join(", ")
                );
            }
        }
        hooks.iter().map(|s| s.as_str()).collect()
    };

    // Check if config already exists
    let existing_config_file = yaml_config_loader::find_config_file(&worktree_root);

    if let Some((config_path, _)) = existing_config_file {
        // Config file exists — don't modify it. Show what's missing and provide a snippet.
        let config = yaml_config_loader::load_merged_config(&worktree_root)
            .context("Failed to load YAML config")?;

        output.info(&format!(
            "Config file already exists: {}",
            bold(&config_path.display().to_string())
        ));

        let (existing, missing): (Vec<&str>, Vec<&str>) = if let Some(ref cfg) = config {
            hook_names
                .iter()
                .partition(|name| cfg.hooks.contains_key(**name))
        } else {
            (vec![], hook_names.clone())
        };

        if missing.is_empty() {
            output.info(&format!(
                "\n{}",
                dim("All requested hooks are already defined.")
            ));
            return Ok(());
        }

        if !existing.is_empty() {
            output.info(&format!(
                "\nAlready defined: {}",
                existing
                    .iter()
                    .map(|n| green(n))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        output.info(&format!(
            "Not yet defined: {}",
            missing
                .iter()
                .map(|n| cyan(n))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        output.info(&format!(
            "\nAdd them to your {} under the {} key:\n",
            bold(&config_path.file_name().unwrap().to_string_lossy()),
            cyan("hooks")
        ));

        let mut snippet = String::new();
        for name in &missing {
            snippet.push_str(&format!("  {name}:\n"));
            snippet.push_str("    jobs:\n");
            snippet.push_str("      - name: setup\n");
            snippet.push_str(&format!(
                "        run: echo \"TODO: add your {name} command\"\n"
            ));
        }
        output.raw(&snippet);
    } else {
        // No config — create new file
        let config_path = worktree_root.join("daft.yml");
        let mut content = String::from(
            "# daft hooks configuration\n# See: https://github.com/avihut/daft\n\nhooks:\n",
        );

        for name in &hook_names {
            content.push_str(&format!(
                "  {name}:\n    jobs:\n      - name: setup\n        run: echo \"TODO: add your {name} command\"\n"
            ));
        }

        std::fs::write(&config_path, &content)
            .with_context(|| format!("Failed to write {}", config_path.display()))?;

        output.success(&format!("{} {}", green("Created"), config_path.display()));
        for name in &hook_names {
            output.info(&format!("  {} {name}", green("added")));
        }
    }

    Ok(())
}
