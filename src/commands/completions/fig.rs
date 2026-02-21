use super::{get_command_for_name, get_flag_descriptions, COMMANDS};
use anyhow::{Context, Result};
use clap::Command;
use serde::Serialize;

// ── Fig/Amazon Q serialization types ──────────────────────────────

#[derive(Serialize)]
struct FigSpec {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<FigArgs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<FigOption>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subcommands: Option<Vec<FigSubcommand>>,
    #[serde(rename = "loadSpec", skip_serializing_if = "Option::is_none")]
    load_spec: Option<String>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum FigArgs {
    Single(FigArg),
    Multiple(Vec<FigArg>),
}

#[derive(Serialize)]
struct FigArg {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generators: Option<FigGenerator>,
}

#[derive(Serialize)]
struct FigGenerator {
    script: Vec<String>,
    #[serde(rename = "splitOn")]
    split_on: String,
}

#[derive(Serialize)]
struct FigOption {
    name: FigName,
    description: String,
}

#[derive(Serialize)]
#[serde(untagged)]
enum FigName {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Serialize)]
struct FigSubcommand {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(rename = "loadSpec", skip_serializing_if = "Option::is_none")]
    load_spec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subcommands: Option<Vec<FigSubcommand>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<FigArgs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<FigOption>>,
}

// ── Fig shared helpers ────────────────────────────────────────────

/// Wrap a serialized spec into ESM module format
fn wrap_esm(filename: &str, spec: &impl Serialize) -> Result<String> {
    let json =
        serde_json::to_string_pretty(spec).context("Failed to serialize Fig completion spec")?;
    Ok(format!(
        "// {filename}.js\nconst completionSpec = {json};\nexport default completionSpec;\n"
    ))
}

/// Build a FigGenerator for dynamic branch completion
fn build_fig_generator(command_name: &str, position: usize) -> FigGenerator {
    let mut script = vec![
        "daft".into(),
        "__complete".into(),
        command_name.to_string(),
        String::new(),
    ];
    if position > 1 {
        script.push("--position".into());
        script.push(position.to_string());
    }
    FigGenerator {
        script,
        split_on: "\n".to_string(),
    }
}

/// Get positional arguments from a clap Command
/// Returns (name, help_text, position_index) for each positional arg
fn get_positional_args(cmd: &Command) -> Vec<(String, String, usize)> {
    let mut args = Vec::new();
    let mut index = 1;

    for arg in cmd.get_arguments() {
        // Skip flags (have short or long)
        if arg.get_short().is_some() || arg.get_long().is_some() {
            continue;
        }

        let name = arg
            .get_value_names()
            .and_then(|v| v.first())
            .map(|n| n.to_string().to_lowercase().replace('_', "-"))
            .unwrap_or_else(|| arg.get_id().to_string().to_lowercase().replace('_', "-"));

        let help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();

        args.push((name, help, index));
        index += 1;
    }

    args
}

/// Generate a Fig/Amazon Q completion spec for a command
pub(super) fn generate_fig_completion_string(command_name: &str) -> Result<String> {
    let cmd =
        get_command_for_name(command_name).context(format!("Unknown command: {command_name}"))?;

    let has_branches = matches!(
        command_name,
        "git-worktree-checkout"
            | "git-worktree-checkout-branch"
            | "git-worktree-carry"
            | "git-worktree-fetch"
    );

    let about = cmd.get_about().map(|a| a.to_string());

    // Build positional args
    let positional_args = get_positional_args(&cmd);
    let args = if positional_args.is_empty() {
        None
    } else {
        let fig_args: Vec<FigArg> = positional_args
            .iter()
            .map(|(name, help, index)| FigArg {
                name: name.clone(),
                description: if help.is_empty() {
                    None
                } else {
                    Some(help.clone())
                },
                generators: if has_branches {
                    Some(build_fig_generator(command_name, *index))
                } else {
                    None
                },
            })
            .collect();
        Some(if fig_args.len() == 1 {
            FigArgs::Single(fig_args.into_iter().next().unwrap())
        } else {
            FigArgs::Multiple(fig_args)
        })
    };

    // Build options from flags
    let flag_descriptions = get_flag_descriptions(&cmd);
    let options: Vec<FigOption> = flag_descriptions
        .into_iter()
        .map(|(short, long, desc)| {
            let name = match (short.is_empty(), long.is_empty()) {
                (false, false) => FigName::Multiple(vec![short, long]),
                (true, false) => FigName::Single(long),
                (false, true) => FigName::Single(short),
                _ => FigName::Single(String::new()),
            };
            FigOption {
                name,
                description: desc.unwrap_or_default(),
            }
        })
        .collect();

    let spec = FigSpec {
        name: command_name.to_string(),
        description: about,
        args,
        options: if options.is_empty() {
            None
        } else {
            Some(options)
        },
        subcommands: None,
        load_spec: None,
    };

    wrap_esm(command_name, &spec)
}

/// Generate a Fig alias spec that loads another command's spec
pub(super) fn generate_fig_alias_string(alias: &str, command_name: &str) -> String {
    let spec = FigSpec {
        name: alias.to_string(),
        description: None,
        args: None,
        options: None,
        subcommands: None,
        load_spec: Some(command_name.to_string()),
    };
    wrap_esm(alias, &spec).unwrap()
}

/// Create a simple FigSubcommand with just name and description
fn fig_subcommand(name: &str, description: &str) -> FigSubcommand {
    FigSubcommand {
        name: name.to_string(),
        description: Some(description.to_string()),
        load_spec: None,
        subcommands: None,
        args: None,
        options: None,
    }
}

/// Build the hooks subcommand with nested subcommands including `run` with a generator
fn build_fig_hooks_subcommand() -> FigSubcommand {
    let hooks_run = FigSubcommand {
        name: "run".to_string(),
        description: Some("Run a hook manually".to_string()),
        load_spec: None,
        subcommands: None,
        args: Some(FigArgs::Single(FigArg {
            name: "hook-type".to_string(),
            description: Some("Hook type to run".to_string()),
            generators: Some(FigGenerator {
                script: vec![
                    "daft".into(),
                    "__complete".into(),
                    "hooks-run".into(),
                    String::new(),
                ],
                split_on: "\n".to_string(),
            }),
        })),
        options: Some(vec![
            FigOption {
                name: FigName::Single("--job".into()),
                description: "Run only the named job".into(),
            },
            FigOption {
                name: FigName::Single("--tag".into()),
                description: "Run only jobs with this tag".into(),
            },
            FigOption {
                name: FigName::Single("--dry-run".into()),
                description: "Preview what would run".into(),
            },
        ]),
    };

    FigSubcommand {
        name: "hooks".to_string(),
        description: Some("Manage lifecycle hooks".to_string()),
        load_spec: None,
        subcommands: Some(vec![
            fig_subcommand("trust", "Trust repository"),
            fig_subcommand("prompt", "Prompt before hooks"),
            fig_subcommand("deny", "Deny hooks"),
            fig_subcommand("status", "Show hooks status"),
            fig_subcommand("migrate", "Migrate hook files"),
            fig_subcommand("install", "Scaffold hooks config"),
            fig_subcommand("validate", "Validate hooks config"),
            fig_subcommand("dump", "Show merged config"),
            hooks_run,
        ]),
        args: None,
        options: None,
    }
}

/// Generate the daft.js umbrella spec with subcommands
pub(super) fn generate_fig_daft_spec() -> Result<String> {
    let simple_subcommands = [
        ("shell-init", "Generate shell initialization scripts"),
        ("completions", "Generate shell completion scripts"),
        ("setup", "Setup and configuration"),
        ("branch", "Branch management utilities"),
        ("multi-remote", "Multi-remote management"),
        ("release-notes", "Generate release notes"),
    ];

    let mut subcommands: Vec<FigSubcommand> = vec![build_fig_hooks_subcommand()];
    subcommands.extend(
        simple_subcommands
            .iter()
            .map(|(name, desc)| fig_subcommand(name, desc)),
    );

    // Worktree commands accessible via daft worktree-*
    for command in COMMANDS {
        let subcommand_name = command.trim_start_matches("git-");
        subcommands.push(FigSubcommand {
            name: subcommand_name.to_string(),
            description: None,
            load_spec: Some(command.to_string()),
            subcommands: None,
            args: None,
            options: None,
        });
    }

    let spec = FigSpec {
        name: "daft".to_string(),
        description: Some("Git Extensions Toolkit".to_string()),
        args: None,
        options: Some(vec![
            FigOption {
                name: FigName::Multiple(vec!["--version".to_string(), "-V".to_string()]),
                description: "Print version information".to_string(),
            },
            FigOption {
                name: FigName::Multiple(vec!["--help".to_string(), "-h".to_string()]),
                description: "Print help".to_string(),
            },
        ]),
        subcommands: Some(subcommands),
        load_spec: None,
    };

    wrap_esm("daft", &spec)
}

/// Generate a git-daft.js spec that loads the daft spec
pub(super) fn generate_fig_git_daft_spec() -> String {
    let spec = FigSpec {
        name: "git-daft".to_string(),
        description: None,
        args: None,
        options: None,
        subcommands: None,
        load_spec: Some("daft".to_string()),
    };
    wrap_esm("git-daft", &spec).unwrap()
}

/// Install Fig/Amazon Q completion specs as individual .js files
pub(super) fn install_fig_completions() -> Result<()> {
    let home = dirs::home_dir().context("Could not determine home directory")?;

    // Kiro / Amazon Q loads specs from autocomplete/build/, not autocomplete/.
    // Detect which parent directory exists, default to ~/.amazon-q/ if neither.
    let amazon_q_parent = home.join(".amazon-q");
    let fig_parent = home.join(".fig");

    let install_dir = if amazon_q_parent.exists() {
        amazon_q_parent.join("autocomplete/build")
    } else if fig_parent.exists() {
        fig_parent.join("autocomplete/build")
    } else {
        amazon_q_parent.join("autocomplete/build")
    };

    std::fs::create_dir_all(&install_dir)
        .with_context(|| format!("Failed to create completion directory: {:?}", install_dir))?;

    eprintln!("Installing Fig/Amazon Q specs to: {:?}", install_dir);

    // Write each command spec
    for command in COMMANDS {
        let filename = format!("{command}.js");
        let file_path = install_dir.join(&filename);
        eprintln!("  Installing: {filename}");
        std::fs::write(&file_path, generate_fig_completion_string(command)?)
            .with_context(|| format!("Failed to write spec file: {:?}", file_path))?;
    }

    // Write shortcut alias specs
    let mut seen_aliases = std::collections::HashSet::new();
    for shortcut in crate::shortcuts::SHORTCUTS {
        if seen_aliases.insert(shortcut.alias) {
            let filename = format!("{}.js", shortcut.alias);
            let file_path = install_dir.join(&filename);
            eprintln!("  Installing: {filename}");
            std::fs::write(
                &file_path,
                generate_fig_alias_string(shortcut.alias, shortcut.command),
            )
            .with_context(|| format!("Failed to write spec file: {:?}", file_path))?;
        }
    }

    // Write daft.js umbrella spec
    let daft_path = install_dir.join("daft.js");
    eprintln!("  Installing: daft.js");
    std::fs::write(&daft_path, generate_fig_daft_spec()?)
        .with_context(|| format!("Failed to write spec file: {:?}", daft_path))?;

    // Write git-daft.js spec
    let git_daft_path = install_dir.join("git-daft.js");
    eprintln!("  Installing: git-daft.js");
    std::fs::write(&git_daft_path, generate_fig_git_daft_spec())
        .with_context(|| format!("Failed to write spec file: {:?}", git_daft_path))?;

    eprintln!("\n✓ Fig/Amazon Q specs installed successfully!");
    eprintln!("\nSpecs will be loaded automatically by Amazon Q / Kiro.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: assert a spec string uses ESM format (const + export default)
    /// and does NOT use CommonJS (var + module.exports).
    fn assert_esm_format(spec: &str, label: &str) {
        assert!(
            spec.contains("const completionSpec"),
            "{label}: should use 'const completionSpec'"
        );
        assert!(
            spec.contains("export default completionSpec"),
            "{label}: should use 'export default completionSpec'"
        );
        assert!(
            !spec.contains("var completionSpec"),
            "{label}: should NOT use 'var completionSpec'"
        );
        assert!(
            !spec.contains("module.exports"),
            "{label}: should NOT use 'module.exports'"
        );
    }

    #[test]
    fn fig_command_spec_uses_esm_format() {
        let spec = generate_fig_completion_string("git-worktree-checkout").unwrap();
        assert_esm_format(&spec, "generate_fig_completion_string");
    }

    #[test]
    fn fig_alias_spec_uses_esm_format() {
        let spec = generate_fig_alias_string("gwtco", "git-worktree-checkout");
        assert_esm_format(&spec, "generate_fig_alias_string");
    }

    #[test]
    fn fig_daft_spec_uses_esm_format() {
        let spec = generate_fig_daft_spec().unwrap();
        assert_esm_format(&spec, "generate_fig_daft_spec");
    }

    #[test]
    fn fig_git_daft_spec_uses_esm_format() {
        let spec = generate_fig_git_daft_spec();
        assert_esm_format(&spec, "generate_fig_git_daft_spec");
    }

    #[test]
    fn fig_checkout_spec_has_generators() {
        let spec = generate_fig_completion_string("git-worktree-checkout").unwrap();
        assert!(
            spec.contains("generators"),
            "checkout spec should have generators"
        );
        assert!(
            spec.contains("__complete"),
            "checkout spec should reference __complete"
        );
        assert!(
            spec.contains("splitOn"),
            "checkout spec should have splitOn"
        );
    }

    #[test]
    fn fig_prune_spec_has_no_generators() {
        let spec = generate_fig_completion_string("git-worktree-prune").unwrap();
        assert!(
            !spec.contains("generators"),
            "prune spec should not have generators"
        );
    }

    #[test]
    fn fig_checkout_branch_spec_has_position() {
        let spec = generate_fig_completion_string("git-worktree-checkout-branch").unwrap();
        assert!(
            spec.contains("--position"),
            "checkout-branch spec should have --position for second arg"
        );
    }

    #[test]
    fn fig_alias_spec_has_load_spec() {
        let spec = generate_fig_alias_string("gwtco", "git-worktree-checkout");
        assert!(spec.contains("loadSpec"), "alias spec should have loadSpec");
        assert!(
            spec.contains("git-worktree-checkout"),
            "alias spec should reference target command"
        );
    }

    #[test]
    fn fig_daft_spec_has_subcommands() {
        let spec = generate_fig_daft_spec().unwrap();
        assert!(
            spec.contains("subcommands"),
            "daft spec should have subcommands"
        );
        assert!(
            spec.contains("shell-init"),
            "daft spec should include shell-init subcommand"
        );
        assert!(
            spec.contains("worktree-checkout"),
            "daft spec should include worktree-checkout subcommand"
        );
    }

    #[test]
    fn fig_spec_output_is_valid_json_in_esm() {
        let spec = generate_fig_completion_string("git-worktree-checkout").unwrap();
        // Extract JSON between "const completionSpec = " and ";\nexport"
        let json_start =
            spec.find("const completionSpec = ").unwrap() + "const completionSpec = ".len();
        let json_end = spec.find(";\nexport").unwrap();
        let json_str = &spec[json_start..json_end];
        let parsed: serde_json::Value =
            serde_json::from_str(json_str).expect("Fig spec should contain valid JSON");
        assert!(parsed.is_object(), "Parsed spec should be a JSON object");
        assert_eq!(parsed["name"], "git-worktree-checkout");
    }
}
