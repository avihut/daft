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

/// Which group a completion entry belongs to, used for visual separation
/// in shells that support per-item tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum CompletionGroup {
    /// Branch has a worktree — immediate navigation target.
    Worktree,
    /// Local branch without a worktree.
    Local,
    /// Remote-tracking branch not mirrored locally.
    Remote,
}

impl CompletionGroup {
    #[allow(dead_code)]
    fn as_str(self) -> &'static str {
        match self {
            CompletionGroup::Worktree => "worktree",
            CompletionGroup::Local => "local",
            CompletionGroup::Remote => "remote",
        }
    }
}

/// A single completion candidate emitted by `daft __complete daft-go`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct CompletionEntry {
    pub name: String,
    pub group: CompletionGroup,
    pub description: String,
}

/// Pure grouping/dedupe/filter/sort function for `daft go` completions.
///
/// Takes already-collected git data and produces a flat, ordered list of
/// completion entries: worktrees first, then local branches, then remote
/// branches. Within each group, entries are sorted alphabetically.
///
/// Dedupe rules: worktree shadows local and remote; local shadows remote.
/// Shadowing is by stripped-name comparison — a remote-only branch whose
/// stripped name collides with a local or worktree branch is dropped.
///
/// The current worktree (if any) is excluded from the worktree group —
/// `daft go` to the branch you're already on is a no-op.
///
/// In single-remote mode, the leading `<default_remote>/` prefix is
/// stripped from remote-branch names. In multi-remote mode the full
/// `<remote>/<branch>` form is preserved. HEAD symrefs (`origin/HEAD`,
/// etc.) are always dropped.
///
/// Entries whose name doesn't start with `prefix` are filtered out.
#[allow(dead_code)]
pub(crate) fn build_go_completions(
    worktrees: &[(String, std::path::PathBuf)],
    local_branches: &[(String, String)],
    remote_branches: &[(String, String)],
    current_worktree_branch: Option<&str>,
    default_remote: &str,
    multi_remote: bool,
    prefix: &str,
) -> Vec<CompletionEntry> {
    use std::collections::BTreeSet;

    // Worktree group: exclude the current worktree's branch.
    let mut wt_entries: Vec<CompletionEntry> = worktrees
        .iter()
        .filter(|(name, _)| Some(name.as_str()) != current_worktree_branch)
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, path)| CompletionEntry {
            name: name.clone(),
            group: CompletionGroup::Worktree,
            description: path.display().to_string(),
        })
        .collect();
    wt_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let wt_names: BTreeSet<&str> = wt_entries.iter().map(|e| e.name.as_str()).collect();

    // Local group: drop anything already in the worktree group.
    let mut local_entries: Vec<CompletionEntry> = local_branches
        .iter()
        .filter(|(name, _)| !wt_names.contains(name.as_str()))
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, age)| CompletionEntry {
            name: name.clone(),
            group: CompletionGroup::Local,
            description: age.clone(),
        })
        .collect();
    local_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let local_names: BTreeSet<String> = local_entries
        .iter()
        .map(|e| e.name.clone())
        .collect::<BTreeSet<_>>();

    // Remote group: drop HEAD symrefs, prefix-strip in single-remote mode,
    // dedupe against worktree + local by stripped name.
    let prefix_to_strip = format!("{default_remote}/");
    let mut remote_entries: Vec<CompletionEntry> = remote_branches
        .iter()
        .filter(|(name, _)| !name.ends_with("/HEAD") && name != "HEAD")
        .filter_map(|(name, age)| {
            let display = if multi_remote {
                name.clone()
            } else if let Some(stripped) = name.strip_prefix(&prefix_to_strip) {
                stripped.to_string()
            } else {
                // In single-remote mode, a remote from a non-default remote
                // is unusual — keep its full name rather than inventing a
                // shadowing rule.
                name.clone()
            };
            if wt_names.contains(display.as_str()) || local_names.contains(&display) {
                return None;
            }
            if !display.starts_with(prefix) {
                return None;
            }
            Some(CompletionEntry {
                name: display,
                group: CompletionGroup::Remote,
                description: age.clone(),
            })
        })
        .collect();
    remote_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = Vec::with_capacity(wt_entries.len() + local_entries.len() + remote_entries.len());
    out.extend(wt_entries);
    out.extend(local_entries);
    out.extend(remote_entries);
    out
}

/// Format grouped completion entries as tab-separated lines for the
/// shell completion protocol: `<name>\t<group>\t<description>`.
#[allow(dead_code)]
pub(crate) fn format_go_completions(entries: &[CompletionEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&entry.name);
        out.push('\t');
        out.push_str(entry.group.as_str());
        out.push('\t');
        out.push_str(&entry.description);
        out.push('\n');
    }
    out
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

    // Tests for the new go-completion grouping function.

    fn wt(name: &str, path: &str) -> (String, std::path::PathBuf) {
        (name.to_string(), std::path::PathBuf::from(path))
    }

    fn br(name: &str, age: &str) -> (String, String) {
        (name.to_string(), age.to_string())
    }

    #[test]
    fn go_completions_group_order_is_worktrees_then_local_then_remote() {
        let entries = build_go_completions(
            &[wt("master", "/tmp/repo/master")],
            &[br("feat/local", "4 days ago")],
            &[br("origin/bug/xyz", "3 weeks ago")],
            None, // no current worktree
            "origin",
            false, // single-remote mode
            "",
        );
        let groups: Vec<CompletionGroup> = entries.iter().map(|e| e.group).collect();
        assert_eq!(
            groups,
            vec![
                CompletionGroup::Worktree,
                CompletionGroup::Local,
                CompletionGroup::Remote,
            ],
            "worktrees must come first, then local, then remote"
        );
    }

    #[test]
    fn go_completions_sort_within_group_alphabetically() {
        let entries = build_go_completions(
            &[wt("b", "/tmp/b"), wt("a", "/tmp/a")],
            &[br("z", "1 day ago"), br("m", "2 days ago")],
            &[],
            None,
            "origin",
            false,
            "",
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "m", "z"]);
    }

    #[test]
    fn go_completions_local_shadows_remote() {
        let entries = build_go_completions(
            &[],
            &[br("feat/shared", "1 day ago")],
            &[br("origin/feat/shared", "2 days ago")],
            None,
            "origin",
            false,
            "",
        );
        assert_eq!(entries.len(), 1, "remote should be shadowed by local");
        assert_eq!(entries[0].name, "feat/shared");
        assert_eq!(entries[0].group, CompletionGroup::Local);
    }

    #[test]
    fn go_completions_worktree_shadows_local_and_remote() {
        let entries = build_go_completions(
            &[wt("master", "/tmp/master")],
            &[br("master", "1 day ago")],
            &[br("origin/master", "2 days ago")],
            None,
            "origin",
            false,
            "",
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "master");
        assert_eq!(entries[0].group, CompletionGroup::Worktree);
    }

    #[test]
    fn go_completions_exclude_current_worktree() {
        let entries = build_go_completions(
            &[wt("master", "/tmp/master"), wt("feat/x", "/tmp/feat-x")],
            &[],
            &[],
            Some("feat/x"),
            "origin",
            false,
            "",
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["master"]);
    }

    #[test]
    fn go_completions_strip_remote_prefix_in_single_remote_mode() {
        let entries = build_go_completions(
            &[],
            &[],
            &[br("origin/bug/xyz", "3 weeks ago")],
            None,
            "origin",
            false,
            "",
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "bug/xyz");
        assert_eq!(entries[0].group, CompletionGroup::Remote);
    }

    #[test]
    fn go_completions_keep_remote_prefix_in_multi_remote_mode() {
        let entries = build_go_completions(
            &[],
            &[],
            &[
                br("origin/bug/xyz", "3 weeks ago"),
                br("fork/feat/y", "2 days ago"),
            ],
            None,
            "origin",
            true, // multi-remote mode
            "",
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["fork/feat/y", "origin/bug/xyz"]);
    }

    #[test]
    fn go_completions_filter_by_prefix() {
        let entries = build_go_completions(
            &[wt("master", "/tmp/master")],
            &[br("feat/x", "1d"), br("fix/y", "2d")],
            &[br("origin/bug/z", "3w")],
            None,
            "origin",
            false,
            "fe",
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["feat/x"]);
    }

    #[test]
    fn go_completions_drop_remote_head_symrefs() {
        let entries = build_go_completions(
            &[],
            &[],
            &[
                br("origin/HEAD", "just now"),
                br("origin/master", "1 day ago"),
            ],
            None,
            "origin",
            false,
            "",
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["master"]);
    }

    #[test]
    fn format_go_completions_emits_tab_separated_name_group_description() {
        let entries = vec![
            CompletionEntry {
                name: "master".into(),
                group: CompletionGroup::Worktree,
                description: "2 hours ago".into(),
            },
            CompletionEntry {
                name: "feat/bar".into(),
                group: CompletionGroup::Local,
                description: "4 days ago".into(),
            },
        ];
        let out = format_go_completions(&entries);
        assert_eq!(
            out,
            "master\tworktree\t2 hours ago\nfeat/bar\tlocal\t4 days ago\n"
        );
    }
}
