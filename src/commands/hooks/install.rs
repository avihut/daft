use super::find_worktree_root;
use crate::hooks::{yaml_config, yaml_config_loader};
use crate::styles::{bold, cyan, dim, green};
use anyhow::{Context, Result};

/// Scaffold a daft.yml configuration with hook definitions.
pub(super) fn cmd_install(hooks: &[String]) -> Result<()> {
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

        println!(
            "Config file already exists: {}",
            bold(&config_path.display().to_string())
        );

        let (existing, missing): (Vec<&str>, Vec<&str>) = if let Some(ref cfg) = config {
            hook_names
                .iter()
                .partition(|name| cfg.hooks.contains_key(**name))
        } else {
            (vec![], hook_names.clone())
        };

        if missing.is_empty() {
            println!("\n{}", dim("All requested hooks are already defined."));
            return Ok(());
        }

        if !existing.is_empty() {
            println!(
                "\nAlready defined: {}",
                existing
                    .iter()
                    .map(|n| green(n))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        println!(
            "Not yet defined: {}",
            missing
                .iter()
                .map(|n| cyan(n))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!(
            "\nAdd them to your {} under the {} key:\n",
            bold(&config_path.file_name().unwrap().to_string_lossy()),
            cyan("hooks")
        );

        for name in &missing {
            println!("  {name}:");
            println!("    jobs:");
            println!("      - name: setup");
            println!("        run: echo \"TODO: add your {name} command\"");
        }

        println!();
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

        println!("{} {}", green("Created"), config_path.display());
        for name in &hook_names {
            println!("  {} {name}", green("added"));
        }
    }

    Ok(())
}
