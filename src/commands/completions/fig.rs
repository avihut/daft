use super::{
    COMMANDS, command_has_repo_positional, emit_formats_for, get_command_for_name,
    get_flag_descriptions, uses_rich_completions,
};
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

#[derive(Serialize, Clone)]
#[serde(untagged)]
enum FigArgs {
    Single(FigArg),
    Multiple(Vec<FigArg>),
}

#[derive(Serialize, Clone)]
struct FigArg {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generators: Option<FigGenerator>,
}

#[derive(Serialize, Clone)]
struct FigGenerator {
    script: Vec<String>,
    #[serde(rename = "splitOn")]
    split_on: String,
}

#[derive(Serialize)]
struct FigOption {
    name: FigName,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<FigOptionArg>,
}

#[derive(Serialize)]
struct FigOptionArg {
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestions: Option<Vec<FigSuggestion>>,
    /// Fig built-in completion source (e.g. "folders", "filepaths"). Set this
    /// to let Fig's filesystem completion fill in the argument value.
    #[serde(skip_serializing_if = "Option::is_none")]
    template: Option<String>,
}

#[derive(Serialize)]
struct FigSuggestion {
    name: String,
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

/// Generator for a positional cataloged-repo name (`daft list [<repo>]`).
fn repo_name_generator() -> FigGenerator {
    FigGenerator {
        script: vec![
            "daft".into(),
            "__complete".into(),
            "repo-name".into(),
            String::new(),
        ],
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

    let has_branches = uses_rich_completions(command_name) || command_name == "daft-start";

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
                } else if command_has_repo_positional(command_name) && *index == 1 {
                    Some(repo_name_generator())
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
    let has_columns = matches!(
        command_name,
        "git-worktree-list" | "git-worktree-sync" | "git-worktree-prune"
    );
    let flag_descriptions = get_flag_descriptions(&cmd);
    let options: Vec<FigOption> = flag_descriptions
        .into_iter()
        .map(|(short, long, desc)| {
            // Add column name suggestions for --columns flag
            let args = if has_columns && long == "--columns" {
                let column_defs = [
                    ("annotation", "Annotation markers"),
                    ("branch", "Branch name"),
                    ("path", "Worktree path"),
                    ("size", "Disk size of worktree"),
                    ("base", "Ahead/behind base branch"),
                    ("changes", "Local changes"),
                    ("remote", "Ahead/behind remote"),
                    ("age", "Branch age"),
                    ("owner", "Branch owner"),
                    ("hash", "Commit hash"),
                    ("last-commit", "Last commit"),
                ];
                let mut suggestions: Vec<FigSuggestion> = Vec::new();
                for (name, description) in &column_defs {
                    suggestions.push(FigSuggestion {
                        name: name.to_string(),
                        description: description.to_string(),
                    });
                    suggestions.push(FigSuggestion {
                        name: format!("+{name}"),
                        description: format!("Add {description}"),
                    });
                    suggestions.push(FigSuggestion {
                        name: format!("-{name}"),
                        description: format!("Remove {description}"),
                    });
                }
                Some(FigOptionArg {
                    suggestions: Some(suggestions),
                    template: None,
                })
            } else if has_columns && long == "--sort" {
                let sort_defs = [
                    ("branch", "Sort by branch name"),
                    ("path", "Sort by worktree path"),
                    ("size", "Sort by disk size"),
                    ("base", "Sort by total base divergence"),
                    ("changes", "Sort by total local changes"),
                    ("remote", "Sort by total remote divergence"),
                    ("age", "Sort by branch age"),
                    ("owner", "Sort by branch owner"),
                    ("hash", "Sort by commit hash"),
                    (
                        "activity",
                        "Sort by overall activity (commits + uncommitted)",
                    ),
                    ("commit", "Sort by last commit time only"),
                ];
                let mut suggestions: Vec<FigSuggestion> = Vec::new();
                for (name, description) in &sort_defs {
                    suggestions.push(FigSuggestion {
                        name: name.to_string(),
                        description: description.to_string(),
                    });
                    suggestions.push(FigSuggestion {
                        name: format!("+{name}"),
                        description: format!("{description} ascending"),
                    });
                    suggestions.push(FigSuggestion {
                        name: format!("-{name}"),
                        description: format!("{description} descending"),
                    });
                }
                Some(FigOptionArg {
                    suggestions: Some(suggestions),
                    template: None,
                })
            } else if long == "--format" {
                emit_formats_for(command_name).map(|formats| {
                    let suggestions = formats
                        .into_iter()
                        .map(|name| FigSuggestion {
                            name: name.to_string(),
                            description: format!("{name} output format"),
                        })
                        .collect();
                    FigOptionArg {
                        suggestions: Some(suggestions),
                        template: None,
                    }
                })
            } else {
                None
            };
            let name = match (short.is_empty(), long.is_empty()) {
                (false, false) => FigName::Multiple(vec![short, long]),
                (true, false) => FigName::Single(long),
                (false, true) => FigName::Single(short),
                _ => FigName::Single(String::new()),
            };
            FigOption {
                name,
                description: desc.unwrap_or_default(),
                args,
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

/// Build the `--format`, `--template`, and `--no-headers` options for an
/// emit-enabled subcommand. Returns empty vec if the path is not emit-enabled.
fn build_emit_options(command_path: &str) -> Vec<FigOption> {
    let Some(formats) = emit_formats_for(command_path) else {
        return Vec::new();
    };
    let suggestions = formats
        .into_iter()
        .map(|name| FigSuggestion {
            name: name.to_string(),
            description: format!("{name} output format"),
        })
        .collect();
    vec![
        FigOption {
            name: FigName::Single("--format".into()),
            description: "Output format. Mutually exclusive with --template.".into(),
            args: Some(FigOptionArg {
                suggestions: Some(suggestions),
                template: None,
            }),
        },
        FigOption {
            name: FigName::Single("--template".into()),
            description: "Tera template string. Mutually exclusive with --format.".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--no-headers".into()),
            description: "Omit header row (tsv/csv only).".into(),
            args: None,
        },
    ]
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
        options: Some({
            let mut opts = vec![
                FigOption {
                    name: FigName::Single("--job".into()),
                    description: "Run only the named job".into(),
                    args: None,
                },
                FigOption {
                    name: FigName::Single("--tag".into()),
                    description: "Run only jobs with this tag".into(),
                    args: None,
                },
                FigOption {
                    name: FigName::Single("--dry-run".into()),
                    description: "Preview what would run".into(),
                    args: None,
                },
            ];
            opts.extend(build_emit_options("hooks run"));
            opts
        }),
    };

    let hooks_jobs = FigSubcommand {
        name: "jobs".to_string(),
        description: Some("View and manage background hook jobs".to_string()),
        load_spec: None,
        subcommands: Some(vec![
            fig_subcommand("logs", "Show logs for a job"),
            fig_subcommand("cancel", "Cancel a running job"),
            fig_subcommand("retry", "Re-run failed jobs from an invocation"),
            fig_subcommand("prune", "Remove old job records past retention"),
        ]),
        args: None,
        options: Some({
            let mut opts = vec![
                FigOption {
                    name: FigName::Single("--all".into()),
                    description: "Show jobs across all worktrees".into(),
                    args: None,
                },
                FigOption {
                    name: FigName::Single("--worktree".into()),
                    description: "Filter to a specific worktree".into(),
                    args: Some(FigOptionArg {
                        suggestions: None,
                        template: None,
                    }),
                },
                FigOption {
                    name: FigName::Single("--status".into()),
                    description: "Filter by job status".into(),
                    args: Some(FigOptionArg {
                        suggestions: None,
                        template: None,
                    }),
                },
                FigOption {
                    name: FigName::Single("--hook".into()),
                    description: "Filter to invocations of this hook type".into(),
                    args: Some(FigOptionArg {
                        suggestions: None,
                        template: None,
                    }),
                },
            ];
            opts.extend(build_emit_options("hooks jobs"));
            opts
        }),
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
            hooks_jobs,
        ]),
        args: None,
        options: None,
    }
}

/// Build the repo subcommand with nested subcommands
fn build_fig_repo_subcommand() -> FigSubcommand {
    let add = FigSubcommand {
        name: "add".to_string(),
        description: Some("Register a repository in the repo catalog".to_string()),
        load_spec: None,
        subcommands: None,
        args: Some(FigArgs::Single(FigArg {
            name: "path".to_string(),
            description: Some("Repository to register (default: current directory)".to_string()),
            generators: None,
        })),
        options: Some(vec![
            FigOption {
                name: FigName::Single("--name".into()),
                description: "Catalog name for the repo; renames it when already registered".into(),
                args: Some(FigOptionArg {
                    suggestions: None,
                    template: None,
                }),
            },
            FigOption {
                name: FigName::Multiple(vec!["--quiet".into(), "-q".into()]),
                description: "Suppress progress reporting".into(),
                args: None,
            },
            FigOption {
                name: FigName::Multiple(vec!["--verbose".into(), "-v".into()]),
                description: "Show detailed progress".into(),
                args: None,
            },
        ]),
    };

    let info = FigSubcommand {
        name: "info".to_string(),
        description: Some("Show a repository's catalog entry".to_string()),
        load_spec: None,
        subcommands: None,
        args: Some(FigArgs::Single(FigArg {
            name: "repo".to_string(),
            description: Some("Catalog name, path, or uuid (default: current repo)".to_string()),
            generators: None,
        })),
        options: None,
    };

    // Column name suggestions for repo list --columns, mirroring the
    // worktree commands' +/- triples.
    let repo_column_defs = [
        ("annotation", "Current repo marker"),
        ("name", "Catalog name"),
        ("worktrees", "Worktree count"),
        ("layout", "Worktree layout"),
        ("branch", "Default branch"),
        ("path", "Repository path"),
        ("size", "Disk size of repository"),
        ("remote", "Remote URL"),
    ];
    let mut repo_column_suggestions: Vec<FigSuggestion> = Vec::new();
    for (name, description) in &repo_column_defs {
        repo_column_suggestions.push(FigSuggestion {
            name: name.to_string(),
            description: description.to_string(),
        });
        repo_column_suggestions.push(FigSuggestion {
            name: format!("+{name}"),
            description: format!("Add {description}"),
        });
        repo_column_suggestions.push(FigSuggestion {
            name: format!("-{name}"),
            description: format!("Remove {description}"),
        });
    }

    let list = FigSubcommand {
        name: "list".to_string(),
        description: Some("List repositories in the repo catalog".to_string()),
        load_spec: None,
        subcommands: None,
        args: None,
        options: Some(vec![
            FigOption {
                name: FigName::Multiple(vec!["--all".into(), "-a".into()]),
                description: "Include removed repositories".into(),
                args: None,
            },
            FigOption {
                name: FigName::Multiple(vec!["--worktrees".into(), "-w".into()]),
                description: "Expand each repository with its worktrees".into(),
                args: None,
            },
            FigOption {
                name: FigName::Single("--columns".into()),
                description: "Columns to display (comma-separated)".into(),
                args: Some(FigOptionArg {
                    suggestions: Some(repo_column_suggestions),
                    template: None,
                }),
            },
        ]),
    };

    let install = FigSubcommand {
        name: "install".to_string(),
        description: Some("Install a starter daft.yml in the current worktree".to_string()),
        load_spec: None,
        subcommands: None,
        args: None,
        options: Some(vec![
            FigOption {
                name: FigName::Multiple(vec!["--quiet".into(), "-q".into()]),
                description: "Suppress progress reporting".into(),
                args: None,
            },
            FigOption {
                name: FigName::Multiple(vec!["--verbose".into(), "-v".into()]),
                description: "Show detailed progress".into(),
                args: None,
            },
            FigOption {
                name: FigName::Single("--git-exclude".into()),
                description: "Add /daft.yml to .git/info/exclude without prompting (keeps it private to this clone)".into(),
                args: None,
            },
        ]),
    };

    let remove = FigSubcommand {
        name: "remove".to_string(),
        description: Some("Remove a repository, including all worktrees".to_string()),
        load_spec: None,
        subcommands: None,
        args: Some(FigArgs::Single(FigArg {
            name: "path".to_string(),
            description: Some("Path to the repo or any directory inside it".to_string()),
            generators: None,
        })),
        options: Some(vec![
            FigOption {
                name: FigName::Single("--repo".into()),
                description: "Cataloged repository to remove (instead of a path)".into(),
                args: Some(FigOptionArg {
                    suggestions: None,
                    template: None,
                }),
            },
            FigOption {
                name: FigName::Single("--keep-files".into()),
                description: "Only remove the repo from the catalog; leave all files on disk"
                    .into(),
                args: None,
            },
            FigOption {
                name: FigName::Multiple(vec!["--force".into(), "-y".into()]),
                description: "Skip the confirmation prompt".into(),
                args: None,
            },
            FigOption {
                name: FigName::Single("--dry-run".into()),
                description: "Print what would be removed without touching anything".into(),
                args: None,
            },
            FigOption {
                name: FigName::Multiple(vec!["--verbose".into(), "-v".into()]),
                description: "Increase verbosity".into(),
                args: None,
            },
        ]),
    };

    FigSubcommand {
        name: "repo".to_string(),
        description: Some("Repository-level operations".to_string()),
        load_spec: None,
        subcommands: Some(vec![add, info, install, list, remove]),
        args: None,
        options: None,
    }
}

/// Build the skill subcommand with nested subcommands
fn build_fig_skill_subcommand() -> FigSubcommand {
    let install = FigSubcommand {
        name: "install".to_string(),
        description: Some("Install or update the agent skill for Claude Code".to_string()),
        load_spec: None,
        subcommands: None,
        args: None,
        options: Some(vec![
            FigOption {
                name: FigName::Single("--project".into()),
                description: "Install into the current worktree's .claude/skills/".into(),
                args: None,
            },
            FigOption {
                name: FigName::Single("--dir".into()),
                description: "Install under this skills root (for other agents)".into(),
                args: Some(FigOptionArg {
                    suggestions: None,
                    template: None,
                }),
            },
            FigOption {
                name: FigName::Multiple(vec!["--quiet".into(), "-q".into()]),
                description: "Suppress the result line".into(),
                args: None,
            },
            FigOption {
                name: FigName::Multiple(vec!["--verbose".into(), "-v".into()]),
                description: "Show detailed progress".into(),
                args: None,
            },
        ]),
    };

    let show = FigSubcommand {
        name: "show".to_string(),
        description: Some("Print the embedded SKILL.md to stdout".to_string()),
        load_spec: None,
        subcommands: None,
        args: None,
        options: None,
    };

    FigSubcommand {
        name: "skill".to_string(),
        description: Some("Manage the daft agent skill".to_string()),
        load_spec: None,
        subcommands: Some(vec![install, show]),
        args: None,
        options: None,
    }
}

/// Build the multi-remote subcommand with nested subcommands
fn build_fig_multi_remote_subcommand() -> FigSubcommand {
    FigSubcommand {
        name: "multi-remote".to_string(),
        description: Some("Multi-remote management".to_string()),
        load_spec: None,
        subcommands: Some(vec![
            fig_subcommand("enable", "Enable multi-remote mode"),
            fig_subcommand("disable", "Disable multi-remote mode"),
            fig_subcommand("status", "Show multi-remote status"),
            fig_subcommand("set-default", "Change default remote"),
            fig_subcommand("move", "Move worktree to different remote folder"),
        ]),
        args: None,
        options: None,
    }
}

/// Build the layout subcommand with nested subcommands and dynamic completions
fn build_fig_layout_subcommand() -> FigSubcommand {
    let layout_arg = FigArgs::Single(FigArg {
        name: "layout".to_string(),
        description: Some("Target layout name or template".to_string()),
        generators: Some(FigGenerator {
            script: vec![
                "daft".into(),
                "__complete".into(),
                "layout-transform".into(),
                String::new(),
            ],
            split_on: "\n".to_string(),
        }),
    });

    let transform = FigSubcommand {
        name: "transform".to_string(),
        description: Some("Convert repo between layouts".to_string()),
        load_spec: None,
        subcommands: None,
        args: Some(layout_arg.clone()),
        options: Some(vec![
            FigOption {
                name: FigName::Multiple(vec!["--force".into(), "-f".into()]),
                description: "Force transform even with uncommitted changes".into(),
                args: None,
            },
            FigOption {
                name: FigName::Single("--dry-run".into()),
                description: "Show plan without executing".into(),
                args: None,
            },
            FigOption {
                name: FigName::Single("--include".into()),
                description: "Also relocate non-conforming worktree".into(),
                args: Some(FigOptionArg {
                    suggestions: None,
                    template: None,
                }),
            },
            FigOption {
                name: FigName::Single("--include-all".into()),
                description: "Relocate all non-conforming worktrees".into(),
                args: None,
            },
        ]),
    };

    let default = FigSubcommand {
        name: "default".to_string(),
        description: Some("View or change global default layout".to_string()),
        load_spec: None,
        subcommands: None,
        args: Some(layout_arg),
        options: Some(vec![FigOption {
            name: FigName::Single("--reset".into()),
            description: "Reset to built-in default".into(),
            args: None,
        }]),
    };

    FigSubcommand {
        name: "layout".to_string(),
        description: Some("Manage worktree layouts".to_string()),
        load_spec: None,
        subcommands: Some(vec![
            fig_subcommand("show", "Show resolved layout for current repo"),
            fig_subcommand("list", "List all available layouts"),
            transform,
            default,
        ]),
        args: None,
        options: None,
    }
}

/// Build the `merge` subcommand with the full flag set and branch completion.
///
/// Wired inline in the daft umbrella spec (not via `COMMANDS`) so the shell
/// completions pick it up without registering merge in the auto-generated list.
fn build_fig_merge_subcommand(name: &str) -> FigSubcommand {
    let branch_generator = FigGenerator {
        script: vec![
            "bash".into(),
            "-c".into(),
            "git for-each-ref --format='%(refname:short)' refs/heads refs/remotes 2>/dev/null"
                .into(),
        ],
        split_on: "\n".to_string(),
    };

    let source_arg = FigArg {
        name: "source".to_string(),
        description: Some(
            "Source branches/commits to merge, OR optional target worktree/branch for finish mode"
                .to_string(),
        ),
        generators: Some(branch_generator.clone()),
    };

    let cleanup_suggestions = Some(FigOptionArg {
        suggestions: Some(vec![
            FigSuggestion {
                name: "default".into(),
                description: "Default cleanup".into(),
            },
            FigSuggestion {
                name: "scissors".into(),
                description: "Remove below scissors line".into(),
            },
            FigSuggestion {
                name: "strip".into(),
                description: "Strip comments and trailing whitespace".into(),
            },
            FigSuggestion {
                name: "verbatim".into(),
                description: "No cleanup".into(),
            },
            FigSuggestion {
                name: "whitespace".into(),
                description: "Strip trailing whitespace only".into(),
            },
        ]),
        template: None,
    });

    let strategy_suggestions = Some(FigOptionArg {
        suggestions: Some(
            ["ours", "recursive", "resolve", "octopus", "subtree"]
                .iter()
                .map(|s| FigSuggestion {
                    name: (*s).to_string(),
                    description: format!("{s} merge strategy"),
                })
                .collect(),
        ),
        template: None,
    });

    let into_suggestions = Some(FigOptionArg {
        suggestions: None,
        template: None,
    });
    let branch_arg_option = FigOption {
        name: FigName::Single("--into".into()),
        description: "Target worktree/branch for the merge".into(),
        args: into_suggestions,
    };

    let options = vec![
        branch_arg_option,
        FigOption {
            name: FigName::Single("--abort".into()),
            description: "Abort an in-progress merge".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--continue".into()),
            description: "Continue an in-progress merge".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--quit".into()),
            description: "Quit an in-progress merge without resetting the index".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--adopt-target".into()),
            description: "Adopt an ephemeral worktree for ref-only non-FF merges".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--no-adopt-target".into()),
            description: "Refuse non-FF merges against target without a worktree".into(),
            args: None,
        },
        FigOption {
            name: FigName::Multiple(vec!["-y".into(), "--yes".into()]),
            description: "Auto-accept interactive prompts".into(),
            args: None,
        },
        FigOption {
            name: FigName::Multiple(vec!["-r".into(), "--remove-branch".into()]),
            description: "Remove the source worktree and delete the source branch".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--keep-branch".into()),
            description: "Explicit keep — for canceling a config-set cleanup default".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--set-default".into()),
            description: "Persist the invocation's style + cleanup as repo defaults".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("-m".into()),
            description: "Commit message for the merge commit".into(),
            args: Some(FigOptionArg {
                suggestions: None,
                template: None,
            }),
        },
        FigOption {
            name: FigName::Multiple(vec!["-F".into(), "--file".into()]),
            description: "Read the commit message from FILE".into(),
            args: Some(FigOptionArg {
                suggestions: None,
                template: None,
            }),
        },
        FigOption {
            name: FigName::Single("--edit".into()),
            description: "Launch editor for merge commit message".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--no-edit".into()),
            description: "Accept auto-generated merge commit message".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--cleanup".into()),
            description: "Commit message cleanup mode".into(),
            args: cleanup_suggestions,
        },
        FigOption {
            name: FigName::Single("--merge".into()),
            description: "Explicit merge style — always create a merge commit (default)".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--squash".into()),
            description: "Squash style — collapse source commits into one squash commit".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--rebase".into()),
            description: "Rebase style — replay source onto target, fast-forward".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--rebase-merge".into()),
            description: "Rebase-merge style — rebase source onto target, then merge commit".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--commit".into()),
            description: "Automatically create the merge commit".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--no-commit".into()),
            description: "Leave the merge staged without committing".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--signoff".into()),
            description: "Add Signed-off-by trailer".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--no-signoff".into()),
            description: "Explicitly disable signoff".into(),
            args: None,
        },
        FigOption {
            name: FigName::Multiple(vec!["-s".into(), "--strategy".into()]),
            description: "Merge strategy to use".into(),
            args: strategy_suggestions,
        },
        FigOption {
            name: FigName::Multiple(vec!["-X".into(), "--strategy-option".into()]),
            description: "Strategy-specific option".into(),
            args: Some(FigOptionArg {
                suggestions: None,
                template: None,
            }),
        },
        FigOption {
            name: FigName::Multiple(vec!["-S".into(), "--gpg-sign".into()]),
            description: "GPG-sign the merge commit (optional KEYID)".into(),
            args: Some(FigOptionArg {
                suggestions: None,
                template: None,
            }),
        },
        FigOption {
            name: FigName::Single("--no-gpg-sign".into()),
            description: "Do not GPG-sign the merge commit".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--verify-signatures".into()),
            description: "Verify source signature".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--no-verify-signatures".into()),
            description: "Do not verify source signature".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--allow-unrelated-histories".into()),
            description: "Allow merging histories with no common ancestor".into(),
            args: None,
        },
        FigOption {
            name: FigName::Single("--stat".into()),
            description: "Show a diffstat at the end of the merge".into(),
            args: None,
        },
        FigOption {
            name: FigName::Multiple(vec!["-n".into(), "--no-stat".into()]),
            description: "Suppress the diffstat".into(),
            args: None,
        },
        FigOption {
            name: FigName::Multiple(vec!["-v".into(), "--verbose".into()]),
            description: "Show detailed progress".into(),
            args: None,
        },
    ];

    FigSubcommand {
        name: name.to_string(),
        description: Some("Merge branches across worktrees".to_string()),
        load_spec: None,
        subcommands: None,
        args: Some(FigArgs::Single(source_arg)),
        options: Some(options),
    }
}

/// Generate the daft.js umbrella spec with subcommands
pub(super) fn generate_fig_daft_spec() -> Result<String> {
    let simple_subcommands = [
        ("shell-init", "Generate shell initialization scripts"),
        ("activate", "Activate daft in this shell"),
        ("release-notes", "Generate release notes"),
    ];

    let mut subcommands: Vec<FigSubcommand> = vec![
        build_fig_hooks_subcommand(),
        build_fig_multi_remote_subcommand(),
        build_fig_layout_subcommand(),
        build_fig_repo_subcommand(),
        build_fig_skill_subcommand(),
        build_fig_merge_subcommand("merge"),
        build_fig_merge_subcommand("worktree-merge"),
    ];
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
                args: None,
            },
            FigOption {
                name: FigName::Multiple(vec!["--help".to_string(), "-h".to_string()]),
                description: "Print help".to_string(),
                args: None,
            },
            // Top-level `-C <path>` (issue #519). `template: "folders"` makes
            // Fig fill in directory completions for the value.
            FigOption {
                name: FigName::Single("-C".to_string()),
                description: "Run as if started in <path>".to_string(),
                args: Some(FigOptionArg {
                    suggestions: None,
                    template: Some("folders".to_string()),
                }),
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

    /// `daft list [<repo>]` — the positional completes catalog repo names.
    #[test]
    fn fig_list_spec_completes_the_repo_positional() {
        let spec = generate_fig_completion_string("git-worktree-list").unwrap();
        assert!(
            spec.contains("repo-name"),
            "list spec must generate catalog repo names for its positional"
        );
    }

    #[test]
    fn fig_repo_remove_spec_offers_repo_and_keep_files() {
        let spec = serde_json::to_string(&build_fig_repo_subcommand()).unwrap();
        assert!(
            spec.contains("--repo") && spec.contains("--keep-files"),
            "repo remove spec must offer --repo and --keep-files"
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
