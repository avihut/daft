/// Dynamic completion helper for shell completions
///
/// This module provides the hidden `__complete` command that shells invoke
/// to get dynamic completion suggestions (e.g., branch names).
///
/// Performance target: < 50ms response time
use anyhow::Result;
use clap::Parser;
use std::process::Command;

use crate::hooks::yaml_config_loader;

#[derive(Parser)]
#[command(name = "daft-complete")]
#[command(about = "Internal helper for dynamic shell completions (not for direct use)")]
#[command(hide = true)]
struct Args {
    #[arg(help = "Command requesting completions")]
    command: String,

    #[arg(help = "Word prefix to complete")]
    word: String,

    #[arg(short, long, help = "Position of the word being completed (0-indexed)")]
    position: Option<usize>,

    #[arg(
        short,
        long,
        help = "Enable verbose error output for debugging completion issues"
    )]
    verbose: bool,
}

pub fn run() -> Result<()> {
    // When called as a subcommand, skip "daft" and "__complete" from args
    let mut args_vec: Vec<String> = std::env::args().collect();

    // If args start with [daft, __complete, ...], keep only [daft, ...]
    // to make clap parse correctly
    if args_vec.len() >= 2 && args_vec[1] == "__complete" {
        args_vec.remove(1); // Remove "__complete", keep "daft" for clap
    }

    let args = Args::parse_from(&args_vec);

    let suggestions = complete(
        &args.command,
        args.position.unwrap_or(1),
        &args.word,
        args.verbose,
    )?;

    for suggestion in suggestions {
        println!("{}", suggestion);
    }

    Ok(())
}

/// Provide context-aware completions based on command and position
fn complete(command: &str, position: usize, word: &str, verbose: bool) -> Result<Vec<String>> {
    match (command, position) {
        // git-worktree-checkout: complete existing branch names
        ("git-worktree-checkout", 1) => complete_existing_branches(word, verbose),

        // git-worktree-clone: repository URL (no dynamic completion for now)
        ("git-worktree-clone", 1) => Ok(vec![]),

        // git-worktree-init: repository name (no dynamic completion)
        ("git-worktree-init", 1) => Ok(vec![]),

        // git-worktree-carry: complete existing branch/worktree names
        ("git-worktree-carry", _) => complete_existing_branches(word, verbose),

        // git-worktree-fetch: complete existing branch/worktree names
        ("git-worktree-fetch", _) => complete_existing_branches(word, verbose),

        // git-worktree-branch: complete existing branch names for deletion
        ("git-worktree-branch", _) => complete_existing_branches(word, verbose),

        // daft-go: complete existing branch names
        ("daft-go", 1) => complete_existing_branches(word, verbose),

        // daft-start: no dynamic completion for new branch names
        ("daft-start", _) => Ok(vec![]),

        // daft-remove: complete existing branch names for deletion
        ("daft-remove", _) => complete_existing_branches(word, verbose),

        // daft-rename: complete existing branch names for rename
        ("daft-rename", _) => complete_existing_branches(word, verbose),

        // git-worktree-prune: no arguments
        ("git-worktree-prune", _) => Ok(vec![]),

        // git-worktree-flow-adopt: directory path (no dynamic completion)
        ("git-worktree-flow-adopt", _) => Ok(vec![]),

        // git-worktree-flow-eject: directory path (no dynamic completion)
        ("git-worktree-flow-eject", _) => Ok(vec![]),

        // hooks run: complete configured hook types
        ("hooks-run", 1) => complete_configured_hooks(word),

        // hooks run --job: complete job names for a hook type
        ("hooks-run-job", 1) => complete_hook_jobs(word, verbose),

        // layout transform / layout default / clone --layout: complete layout names
        ("layout-transform", 1) | ("layout-default", 1) | ("layout-value", 1) => {
            complete_layouts(word)
        }

        // shared-files: complete declared shared file paths from daft.yml
        ("shared-files", _) => complete_shared_files(word),

        // shared-worktrees: complete worktree directory names
        ("shared-worktrees", _) => complete_worktree_names(word),

        // hooks jobs: complete job addresses (names, invocation IDs, composite)
        ("hooks-jobs-job", 1) => complete_job_addresses(word),

        // Default: no completions
        _ => Ok(vec![]),
    }
}

/// Complete existing branch names (local and remote)
fn complete_existing_branches(prefix: &str, verbose: bool) -> Result<Vec<String>> {
    // Use git for-each-ref for fast, parseable output
    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/",
            "refs/remotes/origin/",
        ])
        .output()?;

    if !output.status.success() {
        // Not in a git repository or git command failed
        if verbose {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Git command failed: {}", stderr.trim());
            eprintln!("Exit code: {}", output.status.code().unwrap_or(-1));
            if !std::path::Path::new(".git").exists() {
                eprintln!("Note: Not in a git repository (no .git directory found)");
            }
        }
        return Ok(vec![]);
    }

    let branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|branch| !branch.contains("HEAD"))
        .map(|branch| {
            // Remove "origin/" prefix for cleaner suggestions
            branch.trim_start_matches("origin/").to_string()
        })
        .filter(|branch| branch.starts_with(prefix))
        .collect();

    // Deduplicate branches (local and remote might have same name)
    let mut unique_branches: Vec<String> = branches;
    unique_branches.sort();
    unique_branches.dedup();

    if verbose && unique_branches.is_empty() {
        eprintln!("No branches found matching prefix: '{}'", prefix);
    }

    Ok(unique_branches)
}

/// Suggest common branch name patterns for new branches
#[allow(dead_code)]
fn suggest_new_branch_names(prefix: &str) -> Vec<String> {
    let patterns = [
        "feature/",
        "bugfix/",
        "hotfix/",
        "release/",
        "fix/",
        "feat/",
        "chore/",
        "docs/",
        "test/",
        "refactor/",
    ];

    patterns
        .iter()
        .filter(|pattern| pattern.starts_with(prefix))
        .map(|pattern| pattern.to_string())
        .collect()
}

/// Complete local branches only (for base branch selection)
#[allow(dead_code)]
fn complete_local_branches(prefix: &str, verbose: bool) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .output()?;

    if !output.status.success() {
        if verbose {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Git command failed (local branches): {}", stderr.trim());
        }
        return Ok(vec![]);
    }

    let branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|branch| branch.starts_with(prefix))
        .map(String::from)
        .collect();

    Ok(branches)
}

/// Complete remote branches only
#[allow(dead_code)]
fn complete_remote_branches(prefix: &str, verbose: bool) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/remotes/origin/",
        ])
        .output()?;

    if !output.status.success() {
        if verbose {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Git command failed (remote branches): {}", stderr.trim());
        }
        return Ok(vec![]);
    }

    let branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|branch| !branch.contains("HEAD") && branch.starts_with(prefix))
        .map(|branch| branch.trim_start_matches("origin/").to_string())
        .collect();

    Ok(branches)
}

/// Complete available layout names (built-in + custom from global config).
///
/// Output format: `name\tdescription — template` (tab-separated for shells
/// that support descriptions).
fn complete_layouts(prefix: &str) -> Result<Vec<String>> {
    use crate::core::global_config::GlobalConfig;
    use crate::core::layout::BuiltinLayout;

    let mut entries: Vec<String> = Vec::new();

    // Built-in layouts
    let descriptions: &[(&str, &str)] = &[
        ("contained", "Bare repo, worktrees inside"),
        ("contained-classic", "Regular clone, worktrees beside it"),
        ("contained-flat", "Bare repo, flat branch dirs"),
        ("sibling", "Worktrees next to repo (default)"),
        ("nested", "Hidden .worktrees/ subdir"),
        ("centralized", "Worktrees in XDG data dir"),
    ];

    for builtin in BuiltinLayout::all() {
        let name = builtin.name();
        if !name.starts_with(prefix) {
            continue;
        }
        let desc = descriptions
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, d)| *d)
            .unwrap_or("");
        let layout = builtin.to_layout();
        entries.push(format!("{name}\t{desc:<36}— {}", layout.template));
    }

    // Custom layouts from global config
    if let Ok(config) = GlobalConfig::load() {
        for (name, custom) in &config.layouts {
            if !name.starts_with(prefix) {
                continue;
            }
            // Skip if it shadows a built-in (already listed)
            if BuiltinLayout::from_name(name).is_some() {
                continue;
            }
            let desc = "custom";
            entries.push(format!("{name}\t{desc:<36}— {}", custom.template));
        }
    }

    Ok(entries)
}

/// Complete configured hook types from the current worktree's daft.yml.
fn complete_configured_hooks(prefix: &str) -> Result<Vec<String>> {
    let worktree_root = find_worktree_root().ok();
    let root = match worktree_root {
        Some(ref r) => r.as_path(),
        None => return Ok(vec![]),
    };

    let config = yaml_config_loader::load_merged_config(root).ok().flatten();

    match config {
        Some(cfg) => {
            let mut names: Vec<String> = cfg
                .hooks
                .keys()
                .filter(|name| name.starts_with(prefix))
                .cloned()
                .collect();
            names.sort();
            Ok(names)
        }
        None => Ok(vec![]),
    }
}

/// Complete job names within a hook type.
///
/// The hook type is passed via the `DAFT_COMPLETE_HOOK` environment variable.
fn complete_hook_jobs(prefix: &str, _verbose: bool) -> Result<Vec<String>> {
    let hook_name = match std::env::var("DAFT_COMPLETE_HOOK") {
        Ok(name) => name,
        Err(_) => return Ok(vec![]),
    };

    let worktree_root = find_worktree_root().ok();
    let root = match worktree_root {
        Some(ref r) => r.as_path(),
        None => return Ok(vec![]),
    };

    let config = yaml_config_loader::load_merged_config(root).ok().flatten();

    let config = match config {
        Some(c) => c,
        None => return Ok(vec![]),
    };

    let hook_def = match config.hooks.get(&hook_name) {
        Some(def) => def,
        None => return Ok(vec![]),
    };

    let jobs = yaml_config_loader::get_effective_jobs(hook_def);
    let mut entries: Vec<String> = jobs
        .iter()
        .filter_map(|j| {
            let name = j.name.as_ref()?;
            if !name.starts_with(prefix) {
                return None;
            }
            Some(if let Some(ref desc) = j.description {
                format!("{name}\t{desc}")
            } else {
                name.clone()
            })
        })
        .collect();
    entries.sort();
    entries.dedup();
    Ok(entries)
}

/// Complete declared shared file paths from daft.yml.
fn complete_shared_files(prefix: &str) -> Result<Vec<String>> {
    let root = find_project_root().ok();
    let root = match root {
        Some(ref r) => r.as_path(),
        None => return Ok(vec![]),
    };

    let paths = crate::core::shared::read_shared_paths(root).unwrap_or_default();
    let mut entries: Vec<String> = paths
        .into_iter()
        .filter(|p| p.starts_with(prefix))
        .collect();
    entries.sort();
    Ok(entries)
}

/// Complete worktree directory names.
fn complete_worktree_names(prefix: &str) -> Result<Vec<String>> {
    let paths = crate::core::shared::list_worktree_paths().unwrap_or_default();
    let mut entries: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().to_string();
            if name.starts_with(prefix) {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    entries.sort();
    entries.dedup();
    Ok(entries)
}

/// Complete job addresses for `hooks jobs logs/cancel/retry`.
///
/// Supports three levels of colon-separated addressing:
/// - `name` or `abcd` (0 colons): job names from latest invocation + short IDs
/// - `abcd:name` (1 colon): jobs within a specific invocation OR invocations for a worktree
/// - `worktree:abcd:name` (2 colons): jobs within a specific worktree+invocation
fn complete_job_addresses(prefix: &str) -> Result<Vec<String>> {
    use crate::coordinator::log_store::LogStore;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let repo_hash = find_project_root().ok().map(|root| {
        let mut hasher = DefaultHasher::new();
        root.display().to_string().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    });
    let repo_hash = match repo_hash {
        Some(h) => h,
        None => return Ok(vec![]),
    };

    let store = match LogStore::for_repo(&repo_hash) {
        Ok(s) => s,
        Err(_) => return Ok(vec![]),
    };

    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();
    let now = chrono::Utc::now();

    let colon_count = prefix.matches(':').count();

    match colon_count {
        0 => {
            // First level: job names from latest invocation + invocation short IDs
            let invocations = store
                .list_invocations_for_worktree(&current_worktree)
                .unwrap_or_default();
            let mut entries = Vec::new();

            // Job names from the latest invocation
            if let Some(latest) = invocations.last() {
                let job_dirs = store
                    .list_jobs_in_invocation(&latest.invocation_id)
                    .unwrap_or_default();
                for dir in &job_dirs {
                    if let Ok(meta) = store.read_meta(dir) {
                        if meta.name.starts_with(prefix) {
                            let status_icon = match meta.status {
                                crate::coordinator::log_store::JobStatus::Completed => {
                                    "\u{2713} completed"
                                }
                                crate::coordinator::log_store::JobStatus::Failed => {
                                    "\u{2717} failed"
                                }
                                crate::coordinator::log_store::JobStatus::Running => {
                                    "\u{27f3} running"
                                }
                                crate::coordinator::log_store::JobStatus::Cancelled => {
                                    "\u{2014} cancelled"
                                }
                            };
                            let short_id =
                                &latest.invocation_id[..4.min(latest.invocation_id.len())];
                            let ago = crate::output::format::shorthand_from_seconds(
                                now.signed_duration_since(latest.created_at).num_seconds(),
                            );
                            entries.push(format!(
                                "{}\t{status_icon} -- {ago} ago [{short_id}]",
                                meta.name
                            ));
                        }
                    }
                }
            }

            // Invocation short IDs
            for inv in &invocations {
                let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];
                if short_id.starts_with(prefix) {
                    let ago = crate::output::format::shorthand_from_seconds(
                        now.signed_duration_since(inv.created_at).num_seconds(),
                    );
                    let job_count = store
                        .list_jobs_in_invocation(&inv.invocation_id)
                        .map(|d| d.len())
                        .unwrap_or(0);
                    entries.push(format!(
                        "{short_id}\t{} -- {ago} ago ({job_count} job{})",
                        inv.trigger_command,
                        if job_count == 1 { "" } else { "s" },
                    ));
                }
            }

            Ok(entries)
        }
        1 => {
            // After one colon: try as inv:job, then as worktree:inv
            let (before, after) = prefix.split_once(':').unwrap_or(("", ""));

            let invocations = store
                .list_invocations_for_worktree(&current_worktree)
                .unwrap_or_default();
            let matching: Vec<_> = invocations
                .iter()
                .filter(|inv| inv.invocation_id.starts_with(before))
                .collect();

            if matching.len() == 1 {
                // inv:job completions
                let inv = matching[0];
                let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];
                let job_dirs = store
                    .list_jobs_in_invocation(&inv.invocation_id)
                    .unwrap_or_default();
                let mut entries = Vec::new();
                for dir in &job_dirs {
                    if let Ok(meta) = store.read_meta(dir) {
                        if meta.name.starts_with(after) {
                            let status_icon = match meta.status {
                                crate::coordinator::log_store::JobStatus::Completed => {
                                    "\u{2713} completed"
                                }
                                crate::coordinator::log_store::JobStatus::Failed => {
                                    "\u{2717} failed"
                                }
                                crate::coordinator::log_store::JobStatus::Running => {
                                    "\u{27f3} running"
                                }
                                crate::coordinator::log_store::JobStatus::Cancelled => {
                                    "\u{2014} cancelled"
                                }
                            };
                            entries.push(format!("{short_id}:{}\t{status_icon}", meta.name));
                        }
                    }
                }
                return Ok(entries);
            }

            // Try as worktree: prefix
            let wt_invocations = store
                .list_invocations_for_worktree(before)
                .unwrap_or_default();
            let mut entries = Vec::new();
            for inv in &wt_invocations {
                let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];
                if short_id.starts_with(after) {
                    let ago = crate::output::format::shorthand_from_seconds(
                        now.signed_duration_since(inv.created_at).num_seconds(),
                    );
                    entries.push(format!(
                        "{before}:{short_id}\t{} -- {ago} ago",
                        inv.trigger_command
                    ));
                }
            }
            Ok(entries)
        }
        2 => {
            // worktree:inv:job
            let parts: Vec<&str> = prefix.rsplitn(3, ':').collect();
            let (job_prefix, inv_prefix, wt) = (parts[0], parts[1], parts[2]);
            let invocations = store.list_invocations_for_worktree(wt).unwrap_or_default();
            let matching: Vec<_> = invocations
                .iter()
                .filter(|inv| inv.invocation_id.starts_with(inv_prefix))
                .collect();

            if matching.len() != 1 {
                return Ok(vec![]);
            }
            let inv = matching[0];
            let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];
            let job_dirs = store
                .list_jobs_in_invocation(&inv.invocation_id)
                .unwrap_or_default();
            let mut entries = Vec::new();
            for dir in &job_dirs {
                if let Ok(meta) = store.read_meta(dir) {
                    if meta.name.starts_with(job_prefix) {
                        let status_icon = match meta.status {
                            crate::coordinator::log_store::JobStatus::Completed => {
                                "\u{2713} completed"
                            }
                            crate::coordinator::log_store::JobStatus::Failed => "\u{2717} failed",
                            crate::coordinator::log_store::JobStatus::Running => "\u{27f3} running",
                            crate::coordinator::log_store::JobStatus::Cancelled => {
                                "\u{2014} cancelled"
                            }
                        };
                        entries.push(format!("{wt}:{short_id}:{}\t{status_icon}", meta.name));
                    }
                }
            }
            Ok(entries)
        }
        _ => Ok(vec![]),
    }
}

/// Find the project root (parent of git common dir).
fn find_project_root() -> Result<std::path::PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Not in a git repository");
    }

    let common_dir = std::path::PathBuf::from(String::from_utf8(output.stdout)?.trim());
    let canonical = common_dir.canonicalize()?;
    canonical
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Git common dir has no parent"))
}

/// Find the worktree root directory (for completions).
fn find_worktree_root() -> Result<std::path::PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Not in a git worktree");
    }

    Ok(std::path::PathBuf::from(
        String::from_utf8(output.stdout)?.trim(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suggest_new_branch_names() {
        let suggestions = suggest_new_branch_names("fea");
        assert!(suggestions.contains(&"feature/".to_string()));

        let suggestions = suggest_new_branch_names("hot");
        assert!(suggestions.contains(&"hotfix/".to_string()));

        let suggestions = suggest_new_branch_names("");
        assert!(suggestions.len() >= 10); // All patterns
    }

    #[test]
    fn test_suggest_new_branch_names_no_match() {
        let suggestions = suggest_new_branch_names("xyz");
        assert!(suggestions.is_empty());
    }
}
