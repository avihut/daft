/// Dynamic completion helper for shell completions
///
/// This module provides the hidden `__complete` command that shells invoke
/// to get dynamic completion suggestions (e.g., branch names).
///
/// Performance target: < 50ms response time
use anyhow::{Context, Result};
use clap::Parser;

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

    #[arg(
        long,
        help = "When no local matches are found for the prefix, run `git fetch` \
                with a spinner and re-resolve"
    )]
    fetch_on_miss: bool,
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
        args.fetch_on_miss,
    )?;

    for suggestion in suggestions {
        println!("{}", suggestion);
    }

    Ok(())
}

/// Provide context-aware completions based on command and position
fn complete(
    command: &str,
    position: usize,
    word: &str,
    verbose: bool,
    fetch_on_miss: bool,
) -> Result<Vec<String>> {
    match (command, position) {
        // git-worktree-checkout: rich grouped completions (same as daft-go)
        ("git-worktree-checkout", 1) => Ok(format_entries_as_strings(&complete_rich_branches(
            word,
            &CONFIG_CHECKOUT,
        )?)),

        // git-worktree-clone: repository URL (no dynamic completion for now)
        ("git-worktree-clone", 1) => Ok(vec![]),

        // git-worktree-init: repository name (no dynamic completion)
        ("git-worktree-init", 1) => Ok(vec![]),

        // git-worktree-carry: worktree-only completions
        ("git-worktree-carry", _) => Ok(format_entries_as_strings(&complete_rich_branches(
            word,
            &CONFIG_CARRY,
        )?)),

        // git-worktree-fetch: worktree-only completions
        ("git-worktree-fetch", _) => Ok(format_entries_as_strings(&complete_rich_branches(
            word,
            &CONFIG_FETCH,
        )?)),

        // git-worktree-branch: worktree + local completions for deletion
        ("git-worktree-branch", _) => Ok(format_entries_as_strings(&complete_rich_branches(
            word,
            &CONFIG_BRANCH,
        )?)),

        // daft-go: grouped worktree/local/remote completions with fetch-on-miss
        ("daft-go", 1) => {
            let entries = complete_daft_go(word, fetch_on_miss)?;
            Ok(format_entries_as_strings(&entries))
        }

        // daft-start: no dynamic completion for new branch names
        ("daft-start", _) => Ok(vec![]),

        // daft-remove: worktree + local completions for deletion
        ("daft-remove", _) => Ok(format_entries_as_strings(&complete_rich_branches(
            word,
            &CONFIG_REMOVE,
        )?)),

        // daft-rename: worktree-only completions
        ("daft-rename", _) => Ok(format_entries_as_strings(&complete_rich_branches(
            word,
            &CONFIG_RENAME,
        )?)),

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

// ---------------------------------------------------------------------------
// Gitoxide helpers — repo discovery and time formatting
// ---------------------------------------------------------------------------

/// Discover the git repository from the current working directory via gitoxide.
/// This avoids spawning a subprocess and reuses the in-process gix object cache.
fn discover_repo() -> Result<gix::Repository> {
    let cwd = std::env::current_dir().context("Failed to get current working directory")?;
    let repo = gix::discover(&cwd).context("Failed to discover git repository")?;
    Ok(repo)
}

/// Format a Unix epoch timestamp as a human-readable relative time string
/// matching git's `%(committerdate:relative)` output (e.g. "3 days ago").
fn format_relative_time(epoch_secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let delta = now.saturating_sub(epoch_secs);
    if delta < 0 {
        return "in the future".to_string();
    }
    let delta = delta as u64;

    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;

    if delta < 90 {
        let n = delta;
        return if n == 1 {
            "1 second ago".to_string()
        } else {
            format!("{n} seconds ago")
        };
    }
    if delta < 90 * MINUTE {
        let n = delta / MINUTE;
        return if n == 1 {
            "1 minute ago".to_string()
        } else {
            format!("{n} minutes ago")
        };
    }
    if delta < 36 * HOUR {
        let n = delta / HOUR;
        return if n == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{n} hours ago")
        };
    }
    if delta < 14 * DAY {
        let n = delta / DAY;
        return if n == 1 {
            "1 day ago".to_string()
        } else {
            format!("{n} days ago")
        };
    }
    if delta < 10 * WEEK {
        let n = delta / WEEK;
        return if n == 1 {
            "1 week ago".to_string()
        } else {
            format!("{n} weeks ago")
        };
    }
    if delta < YEAR {
        let n = delta / MONTH;
        return if n == 1 {
            "1 month ago".to_string()
        } else {
            format!("{n} months ago")
        };
    }
    let years = delta / YEAR;
    let months = (delta % YEAR) / MONTH;
    if months == 0 {
        if years == 1 {
            "1 year ago".to_string()
        } else {
            format!("{years} years ago")
        }
    } else if years == 1 {
        format!("1 year, {months} months ago")
    } else {
        format!("{years} years, {months} months ago")
    }
}

/// Read the branch checked out in a worktree from its private git dir's HEAD file.
/// Returns `None` for detached HEAD or unreadable files.
fn worktree_branch_from_gitdir(git_dir: &std::path::Path) -> Option<String> {
    let head_contents = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let trimmed = head_contents.trim();
    trimmed
        .strip_prefix("ref: refs/heads/")
        .map(|s| s.to_string())
}

/// Peel a reference to its commit and return the committer timestamp as a
/// relative-time string. Returns an empty string on failure (tag that doesn't
/// point to a commit, corrupt object, etc.).
fn ref_commit_age(reference: &mut gix::Reference<'_>) -> String {
    let id = match reference.peel_to_id() {
        Ok(id) => id.detach(),
        Err(_) => return String::new(),
    };
    let repo = reference.repo;
    let object = match repo.find_object(id) {
        Ok(o) => o,
        Err(_) => return String::new(),
    };
    let commit = match object.try_into_commit() {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let sig = match commit.committer() {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    match sig.time() {
        Ok(time) => format_relative_time(time.seconds),
        Err(_) => String::new(),
    }
}

/// Collect `(branch, path)` pairs for every worktree (main + linked) that
/// has a branch checked out. Detached HEADs and bare repos are skipped.
fn collect_worktrees(repo: &gix::Repository) -> Vec<(String, std::path::PathBuf)> {
    let mut result = Vec::new();

    // Main worktree (if not bare / has a working directory).
    if let Some(workdir) = repo.workdir() {
        if let Ok(Some(head_ref)) = repo.head_ref() {
            let branch = head_ref.name().shorten().to_string();
            result.push((branch, workdir.to_path_buf()));
        }
        // Detached HEAD in main worktree → skip (no branch).
    }

    // Linked worktrees — skip any that duplicate the current workdir
    // (when gix::discover runs from a linked worktree, repo.workdir()
    // already returns it, but repo.worktrees() also lists it).
    let current_workdir = repo.workdir().map(|p| p.to_path_buf());
    let proxies = match repo.worktrees() {
        Ok(p) => p,
        Err(_) => return result,
    };
    for proxy in proxies {
        let branch = match worktree_branch_from_gitdir(proxy.git_dir()) {
            Some(b) => b,
            None => continue, // detached or unreadable
        };
        let path = match proxy.base() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if current_workdir.as_deref() == Some(path.as_ref()) {
            continue; // already collected via repo.workdir() above
        }
        result.push((branch, path));
    }

    result
}

/// Collect `(branch, relative_age)` pairs for a given ref prefix via gitoxide.
fn collect_refs_with_age(repo: &gix::Repository, prefix: &str) -> Vec<(String, String)> {
    let platform = match repo.references() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let references = match platform.prefixed(prefix) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut result = Vec::new();
    for ref_result in references {
        let mut reference = match ref_result {
            Ok(r) => r,
            Err(_) => continue,
        };
        let short_name = reference.name().shorten().to_string();
        let age = ref_commit_age(&mut reference);
        result.push((short_name, age));
    }
    result
}

/// Collect `(branch, relative_age)` pairs for every local branch.
fn collect_local_branches(repo: &gix::Repository) -> Vec<(String, String)> {
    collect_refs_with_age(repo, "refs/heads/")
}

/// Collect `(branch, relative_age)` pairs for every remote-tracking
/// branch across all remotes.
fn collect_remote_branches(repo: &gix::Repository) -> Vec<(String, String)> {
    collect_refs_with_age(repo, "refs/remotes/")
}

/// Collect the current worktree's branch, if any — used to exclude it
/// from the completion list. Returns `None` if HEAD is detached, if
/// the current directory is outside a git repository, or if we're in
/// a bare repository (e.g., the root of a contained layout where HEAD
/// points to the default branch but no worktree corresponds to CWD).
fn current_worktree_branch(repo: &gix::Repository) -> Option<String> {
    // In a bare repository there is no "current worktree" to exclude.
    repo.workdir()?;
    repo.head_ref()
        .ok()
        .flatten()
        .map(|r| r.name().shorten().to_string())
}

/// Top-level completion helper for `daft go`. Collects real git data,
/// applies grouping rules, and returns the ordered candidate list.
/// When `fetch_on_miss` is true and the prefix has no local matches,
/// runs `git fetch` with a spinner and re-resolves.
pub(crate) fn complete_daft_go(prefix: &str, fetch_on_miss: bool) -> Result<Vec<CompletionEntry>> {
    use std::time::Instant;

    use crate::core::settings::{defaults, keys};

    let timings = std::env::var("DAFT_COMPLETE_TIMINGS").is_ok();
    let t_total = Instant::now();

    let t = Instant::now();
    let repo = discover_repo()?;
    let d_discover = t.elapsed();

    // Read remote config + go-specific fetch-on-miss setting.
    let t = Instant::now();
    let (default_remote, multi_remote_enabled) = read_remote_config(&repo);
    let go_fetch_on_miss = repo
        .config_snapshot()
        .boolean(keys::GO_FETCH_ON_MISS)
        .unwrap_or(defaults::GO_FETCH_ON_MISS);
    let d_settings = t.elapsed();

    let collect = |repo: &gix::Repository, timings: bool| -> Vec<CompletionEntry> {
        let t = Instant::now();
        let worktrees = collect_worktrees(repo);
        let d_wt = t.elapsed();

        let t = Instant::now();
        let local = collect_local_branches(repo);
        let d_local = t.elapsed();

        let t = Instant::now();
        let remote = collect_remote_branches(repo);
        let d_remote = t.elapsed();

        let t = Instant::now();
        let current_branch = current_worktree_branch(repo);
        let d_current = t.elapsed();

        let t = Instant::now();
        let entries = build_rich_completions(
            &worktrees,
            &local,
            &remote,
            current_branch.as_deref(),
            &default_remote,
            multi_remote_enabled,
            prefix,
        );
        let d_build = t.elapsed();

        if timings {
            eprintln!(
                "[timings] worktrees        : {:>7.1}ms",
                d_wt.as_secs_f64() * 1000.0
            );
            eprintln!(
                "[timings] local_branches   : {:>7.1}ms",
                d_local.as_secs_f64() * 1000.0
            );
            eprintln!(
                "[timings] remote_branches  : {:>7.1}ms",
                d_remote.as_secs_f64() * 1000.0
            );
            eprintln!(
                "[timings] current_branch   : {:>7.1}ms",
                d_current.as_secs_f64() * 1000.0
            );
            eprintln!(
                "[timings] build            : {:>7.1}ms",
                d_build.as_secs_f64() * 1000.0
            );
        }

        entries
    };

    if timings {
        eprintln!(
            "[timings] repo_discover    : {:>7.1}ms",
            d_discover.as_secs_f64() * 1000.0
        );
        eprintln!(
            "[timings] settings_load    : {:>7.1}ms",
            d_settings.as_secs_f64() * 1000.0
        );
    }

    let entries = collect(&repo, timings);

    if !entries.is_empty() || !fetch_on_miss || !go_fetch_on_miss || prefix.is_empty() {
        if timings {
            eprintln!(
                "[timings] total            : {:>7.1}ms",
                t_total.elapsed().as_secs_f64() * 1000.0
            );
        }
        return Ok(entries);
    }

    let common_dir = repo.common_dir().to_path_buf();
    let git_common_dir = common_dir.canonicalize().unwrap_or(common_dir);
    let marker = git_common_dir.join("daft_complete_last_fetch");
    if !should_run_fetch(&marker, std::time::Duration::from_secs(30)) {
        if timings {
            eprintln!(
                "[timings] total            : {:>7.1}ms",
                t_total.elapsed().as_secs_f64() * 1000.0
            );
        }
        return Ok(entries);
    }

    let t = Instant::now();
    let spinner =
        crate::completion_spinner::Spinner::start(&format!("Fetching refs from {default_remote}…"));

    let fetch_result = std::process::Command::new("git")
        .args([
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-recurse-submodules",
            &default_remote,
        ])
        .output();
    let _ = fetch_result;

    if let Some(s) = spinner {
        s.stop();
    }
    let d_fetch = t.elapsed();

    let _ = touch_fetch_marker(&marker);

    if timings {
        eprintln!(
            "[timings] fetch            : {:>7.1}ms",
            d_fetch.as_secs_f64() * 1000.0
        );
    }

    let entries = collect(&repo, timings);

    if timings {
        eprintln!(
            "[timings] total            : {:>7.1}ms",
            t_total.elapsed().as_secs_f64() * 1000.0
        );
    }

    Ok(entries)
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
    let repo = discover_repo()?;
    let common_dir = repo.common_dir().to_path_buf();
    let canonical = common_dir.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize git directory: {}",
            common_dir.display()
        )
    })?;
    canonical
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Git common dir has no parent"))
}

/// Find the worktree root directory (for completions).
fn find_worktree_root() -> Result<std::path::PathBuf> {
    let repo = discover_repo()?;
    repo.workdir()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Not inside a worktree (bare repository)"))
}

/// Read multi-remote and default-remote settings from the repo's in-memory
/// config snapshot (zero subprocess overhead).
fn read_remote_config(repo: &gix::Repository) -> (String, bool) {
    use crate::core::settings::{defaults, keys};

    let config = repo.config_snapshot();
    let multi_remote_enabled = config
        .boolean(keys::multi_remote::ENABLED)
        .unwrap_or(defaults::MULTI_REMOTE_ENABLED);
    let default_remote = if multi_remote_enabled {
        config
            .string(keys::multi_remote::DEFAULT_REMOTE)
            .map(|v| v.to_string())
            .unwrap_or_else(|| defaults::MULTI_REMOTE_DEFAULT_REMOTE.to_string())
    } else {
        config
            .string(keys::REMOTE)
            .map(|v| v.to_string())
            .unwrap_or_else(|| defaults::REMOTE.to_string())
    };
    (default_remote, multi_remote_enabled)
}

/// Shared rich branch completion entry point. Discovers the repo, reads
/// config, collects branch/worktree data according to `config`, and
/// returns grouped completion entries.
fn complete_rich_branches(
    prefix: &str,
    config: &RichCompletionConfig,
) -> Result<Vec<CompletionEntry>> {
    let repo = discover_repo()?;
    let (default_remote, multi_remote_enabled) = read_remote_config(&repo);

    let worktrees = if config.include_worktrees {
        collect_worktrees(&repo)
    } else {
        Vec::new()
    };
    let local = if config.include_local || config.include_worktrees {
        // Always collect local branches when worktrees are included —
        // needed to look up commit ages for worktree entries.
        collect_local_branches(&repo)
    } else {
        Vec::new()
    };
    let remote = if config.include_remote {
        collect_remote_branches(&repo)
    } else {
        Vec::new()
    };
    let current_branch = if config.exclude_current {
        current_worktree_branch(&repo)
    } else {
        None
    };

    let mut entries = build_rich_completions(
        &worktrees,
        &local,
        &remote,
        current_branch.as_deref(),
        &default_remote,
        multi_remote_enabled,
        prefix,
    );

    // If the config excludes local branches but we collected them for
    // worktree age lookup, strip them from the output.
    if !config.include_local {
        entries.retain(|e| e.group != CompletionGroup::Local);
    }

    Ok(entries)
}

/// Format rich completion entries as tab-separated strings for the shell
/// completion protocol: `<name>\t<group>\t<description>`.
fn format_entries_as_strings(entries: &[CompletionEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|e| format!("{}\t{}\t{}", e.name, e.group.as_str(), e.description))
        .collect()
}

/// Return `true` if the cooldown marker is missing or older than
/// `cooldown`. Used to decide whether the fetch-on-miss path should run.
fn should_run_fetch(marker: &std::path::Path, cooldown: std::time::Duration) -> bool {
    let metadata = match std::fs::metadata(marker) {
        Ok(m) => m,
        Err(_) => return true,
    };
    let mtime = match metadata.modified() {
        Ok(t) => t,
        Err(_) => return true,
    };
    match std::time::SystemTime::now().duration_since(mtime) {
        Ok(age) => age >= cooldown,
        Err(_) => true,
    }
}

/// Create or update the cooldown marker to reflect a just-completed fetch.
fn touch_fetch_marker(marker: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(marker)?;
    filetime::set_file_mtime(
        marker,
        filetime::FileTime::from_system_time(std::time::SystemTime::now()),
    )?;
    Ok(())
}

/// Which group a completion entry belongs to, used for visual separation
/// in shells that support per-item tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionGroup {
    /// Branch has a worktree — immediate navigation target.
    Worktree,
    /// Local branch without a worktree.
    Local,
    /// Remote-tracking branch not mirrored locally.
    Remote,
}

impl CompletionGroup {
    fn as_str(self) -> &'static str {
        match self {
            CompletionGroup::Worktree => "worktree",
            CompletionGroup::Local => "local",
            CompletionGroup::Remote => "remote",
        }
    }
}

/// A single completion candidate emitted by `daft __complete` for rich
/// branch completions.
#[derive(Debug, Clone)]
pub(crate) struct CompletionEntry {
    pub name: String,
    pub group: CompletionGroup,
    pub description: String,
}

/// Configuration for which groups to include in rich branch completions.
struct RichCompletionConfig {
    /// Include worktree branches (branches checked out in a worktree).
    include_worktrees: bool,
    /// Include local branches not checked out in any worktree.
    include_local: bool,
    /// Include remote-tracking branches.
    include_remote: bool,
    /// Exclude the branch checked out in the current worktree.
    exclude_current: bool,
}

// Per-command completion configurations.

const CONFIG_CHECKOUT: RichCompletionConfig = RichCompletionConfig {
    include_worktrees: true,
    include_local: true,
    include_remote: true,
    exclude_current: true,
};

const CONFIG_REMOVE: RichCompletionConfig = RichCompletionConfig {
    include_worktrees: true,
    include_local: true,
    include_remote: false,
    exclude_current: false,
};

const CONFIG_RENAME: RichCompletionConfig = RichCompletionConfig {
    include_worktrees: true,
    include_local: false,
    include_remote: false,
    exclude_current: false,
};

const CONFIG_CARRY: RichCompletionConfig = RichCompletionConfig {
    include_worktrees: true,
    include_local: false,
    include_remote: false,
    exclude_current: true,
};

const CONFIG_FETCH: RichCompletionConfig = RichCompletionConfig {
    include_worktrees: true,
    include_local: false,
    include_remote: false,
    exclude_current: false,
};

const CONFIG_BRANCH: RichCompletionConfig = RichCompletionConfig {
    include_worktrees: true,
    include_local: true,
    include_remote: false,
    exclude_current: false,
};

/// Pure grouping/dedupe/filter/sort function for rich branch completions.
///
/// Takes already-collected git data and produces a flat, ordered list of
/// completion entries: worktrees first, then local branches, then remote
/// branches. Within each group, entries are sorted alphabetically.
///
/// Dedupe rules: worktree shadows local and remote; local shadows remote.
/// Shadowing is by stripped-name comparison — a remote-only branch whose
/// stripped name collides with a local or worktree branch is dropped.
///
/// The current worktree (if any) is excluded from the worktree group.
///
/// In single-remote mode, the leading `<default_remote>/` prefix is
/// stripped from remote-branch names. In multi-remote mode the full
/// `<remote>/<branch>` form is preserved. HEAD symrefs (`origin/HEAD`,
/// etc.) are always dropped.
///
/// Entries whose name doesn't start with `prefix` are filtered out.
pub(crate) fn build_rich_completions(
    worktrees: &[(String, std::path::PathBuf)],
    local_branches: &[(String, String)],
    remote_branches: &[(String, String)],
    current_worktree_branch: Option<&str>,
    default_remote: &str,
    multi_remote: bool,
    prefix: &str,
) -> Vec<CompletionEntry> {
    use std::collections::HashSet;

    // Worktree group: exclude the current worktree's branch.
    // Look up the commit age from local_branches for each worktree, and
    // pack both age and path into the description as "age\tpath".
    let mut wt_entries: Vec<CompletionEntry> = worktrees
        .iter()
        .filter(|(name, _)| Some(name.as_str()) != current_worktree_branch)
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, path)| {
            let age = local_branches
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, a)| a.as_str())
                .unwrap_or("");
            CompletionEntry {
                name: name.clone(),
                group: CompletionGroup::Worktree,
                description: format!("{}\t{}", age, path.display()),
            }
        })
        .collect();
    wt_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let wt_names: HashSet<&str> = wt_entries.iter().map(|e| e.name.as_str()).collect();

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

    let local_names: HashSet<&str> = local_entries.iter().map(|e| e.name.as_str()).collect();

    // Remote group: drop HEAD symrefs, prefix-strip in single-remote mode,
    // dedupe against worktree + local by stripped name.
    let prefix_to_strip = format!("{default_remote}/");
    let mut remote_entries: Vec<CompletionEntry> = remote_branches
        .iter()
        .filter(|(name, _)| {
            // Drop HEAD symrefs (`origin/HEAD`) and the collapsed HEAD
            // short-form (git emits the bare remote name `origin` for the
            // remote HEAD symref via `for-each-ref --format=%(refname:short)`).
            // Any legitimate remote-tracking branch has a `/` separating
            // the remote name from the branch path.
            !name.ends_with("/HEAD") && name != "HEAD" && name.contains('/')
        })
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
            if wt_names.contains(display.as_str()) || local_names.contains(display.as_str()) {
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

/// Format rich completion entries as tab-separated lines for the
/// shell completion protocol: `<name>\t<group>\t<description>`.
#[allow(dead_code)]
pub(crate) fn format_rich_completions(entries: &[CompletionEntry]) -> String {
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

    // Tests for the rich completion grouping function.

    fn wt(name: &str, path: &str) -> (String, std::path::PathBuf) {
        (name.to_string(), std::path::PathBuf::from(path))
    }

    fn br(name: &str, age: &str) -> (String, String) {
        (name.to_string(), age.to_string())
    }

    #[test]
    fn rich_completions_group_order_is_worktrees_then_local_then_remote() {
        let entries = build_rich_completions(
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
    fn rich_completions_sort_within_group_alphabetically() {
        let entries = build_rich_completions(
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
    fn rich_completions_local_shadows_remote() {
        let entries = build_rich_completions(
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
    fn rich_completions_worktree_shadows_local_and_remote() {
        let entries = build_rich_completions(
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
    fn rich_completions_exclude_current_worktree() {
        let entries = build_rich_completions(
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
    fn rich_completions_strip_remote_prefix_in_single_remote_mode() {
        let entries = build_rich_completions(
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
    fn rich_completions_keep_remote_prefix_in_multi_remote_mode() {
        let entries = build_rich_completions(
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
    fn rich_completions_filter_by_prefix() {
        let entries = build_rich_completions(
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
    fn rich_completions_drop_remote_head_symrefs() {
        let entries = build_rich_completions(
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
    fn format_rich_completions_emits_tab_separated_name_group_description() {
        let entries = vec![
            CompletionEntry {
                name: "master".into(),
                group: CompletionGroup::Worktree,
                description: "2 hours ago\t/tmp/wt/master".into(),
            },
            CompletionEntry {
                name: "feat/bar".into(),
                group: CompletionGroup::Local,
                description: "4 days ago".into(),
            },
        ];
        let out = format_rich_completions(&entries);
        assert_eq!(
            out,
            "master\tworktree\t2 hours ago\t/tmp/wt/master\nfeat/bar\tlocal\t4 days ago\n"
        );
    }

    #[test]
    fn rich_completions_empty_input_returns_empty() {
        let entries = build_rich_completions(&[], &[], &[], None, "origin", false, "");
        assert!(entries.is_empty());
    }

    #[test]
    fn rich_completions_non_matching_prefix_returns_empty() {
        let entries = build_rich_completions(
            &[wt("master", "/tmp/master")],
            &[br("feat/x", "1d")],
            &[br("origin/bug/y", "2d")],
            None,
            "origin",
            false,
            "zzz",
        );
        assert!(entries.is_empty());
    }

    #[test]
    fn rich_completions_prefix_filter_applies_after_remote_strip() {
        // In single-remote mode, the user sees `bug/xyz` (not `origin/bug/xyz`).
        // The prefix filter must match against the stripped display name.
        let entries = build_rich_completions(
            &[],
            &[],
            &[br("origin/bug/xyz", "3w")],
            None,
            "origin",
            false,
            "bu",
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "bug/xyz");
        assert_eq!(entries[0].group, CompletionGroup::Remote);
    }

    #[test]
    fn rich_completions_multi_remote_dedupe_keeps_distinct_display_names() {
        // In multi-remote mode, `origin/feat/x` and `fork/feat/x` are distinct
        // display names and both must survive — they don't shadow each other.
        let entries = build_rich_completions(
            &[],
            &[],
            &[br("origin/feat/x", "1d"), br("fork/feat/x", "2d")],
            None,
            "origin",
            true, // multi-remote
            "",
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["fork/feat/x", "origin/feat/x"]);
    }

    #[test]
    fn rich_completions_drop_bare_remote_name_symref_collapse() {
        // `git for-each-ref refs/remotes/ --format=%(refname:short)` emits
        // the bare remote name (`origin`) as the short name for
        // `refs/remotes/origin/HEAD`. This is the collapsed form that our
        // `/HEAD` suffix filter misses, so it needs its own filter rule.
        let entries = build_rich_completions(
            &[],
            &[],
            &[
                br("origin", "2 hours ago"),
                br("origin/master", "1 day ago"),
            ],
            None,
            "origin",
            false,
            "",
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["master"],
            "bare remote name `origin` must be filtered out"
        );
    }

    #[test]
    fn rich_completions_drop_bare_remote_name_in_multi_remote_mode() {
        // In multi-remote mode, bare remote names are equally bogus — git
        // still collapses `refs/remotes/<remote>/HEAD` to just `<remote>`.
        let entries = build_rich_completions(
            &[],
            &[],
            &[
                br("origin", "1 day ago"),
                br("fork", "2 days ago"),
                br("origin/feat/x", "1 hour ago"),
            ],
            None,
            "origin",
            true,
            "",
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["origin/feat/x"]);
    }

    #[test]
    fn fetch_cooldown_allows_fetch_when_marker_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("daft_complete_last_fetch");
        assert!(
            should_run_fetch(&marker, std::time::Duration::from_secs(30)),
            "missing marker must allow fetch"
        );
    }

    #[test]
    fn fetch_cooldown_blocks_fetch_within_window() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("daft_complete_last_fetch");
        touch_fetch_marker(&marker).unwrap();
        assert!(
            !should_run_fetch(&marker, std::time::Duration::from_secs(30)),
            "freshly touched marker must block fetch"
        );
    }

    #[test]
    fn fetch_cooldown_allows_fetch_after_window() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("daft_complete_last_fetch");
        touch_fetch_marker(&marker).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(31);
        filetime::set_file_mtime(&marker, filetime::FileTime::from_system_time(old)).unwrap();
        assert!(
            should_run_fetch(&marker, std::time::Duration::from_secs(30)),
            "marker older than cooldown must allow fetch"
        );
    }

    // --- format_relative_time tests ---

    fn now_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn relative_time_seconds() {
        assert_eq!(format_relative_time(now_secs() - 30), "30 seconds ago");
        assert_eq!(format_relative_time(now_secs() - 1), "1 second ago");
    }

    #[test]
    fn relative_time_minutes() {
        assert_eq!(format_relative_time(now_secs() - 120), "2 minutes ago");
        assert_eq!(format_relative_time(now_secs() - 60 * 45), "45 minutes ago");
    }

    #[test]
    fn relative_time_hours() {
        assert_eq!(format_relative_time(now_secs() - 3600 * 3), "3 hours ago");
    }

    #[test]
    fn relative_time_days() {
        assert_eq!(format_relative_time(now_secs() - 86400 * 5), "5 days ago");
    }

    #[test]
    fn relative_time_weeks() {
        assert_eq!(format_relative_time(now_secs() - 86400 * 21), "3 weeks ago");
    }

    #[test]
    fn relative_time_months() {
        assert_eq!(
            format_relative_time(now_secs() - 86400 * 90),
            "3 months ago"
        );
    }

    #[test]
    fn relative_time_years() {
        assert_eq!(
            format_relative_time(now_secs() - 86400 * 400),
            "1 year, 1 months ago"
        );
        assert_eq!(
            format_relative_time(now_secs() - 86400 * 800),
            "2 years, 2 months ago"
        );
    }

    // --- RichCompletionConfig behavior tests ---

    #[test]
    fn rich_completions_worktree_only_returns_no_local_or_remote() {
        // When only worktrees are requested (like daft-rename),
        // local and remote groups must be stripped even if data is provided.
        let worktrees = vec![wt("main", "/repo/main"), wt("feat", "/repo/feat")];
        let local = vec![
            br("main", "1 day ago"),
            br("feat", "2 days ago"),
            br("dev", "3 days ago"),
        ];
        let remote = vec![br("origin/ci", "4 days ago")];

        let entries =
            build_rich_completions(&worktrees, &local, &remote, None, "origin", false, "");

        // build_rich_completions returns all groups — the caller filters.
        // Simulate what complete_rich_branches does with include_local=false:
        let filtered: Vec<_> = entries
            .into_iter()
            .filter(|e| e.group == CompletionGroup::Worktree)
            .collect();
        assert_eq!(filtered.len(), 2);
        assert!(filtered
            .iter()
            .all(|e| e.group == CompletionGroup::Worktree));
    }

    #[test]
    fn rich_completions_no_remote_returns_worktrees_and_local() {
        // When remote is excluded (like daft-remove), only worktree + local.
        let worktrees = vec![wt("main", "/repo/main")];
        let local = vec![br("main", "1 day ago"), br("dev", "2 days ago")];
        let remote = vec![br("origin/ci", "3 days ago")];

        let entries =
            build_rich_completions(&worktrees, &local, &remote, None, "origin", false, "");

        // Simulate exclude_remote by filtering.
        let filtered: Vec<_> = entries
            .into_iter()
            .filter(|e| e.group != CompletionGroup::Remote)
            .collect();
        assert_eq!(filtered.len(), 2); // main (worktree), dev (local)
        let groups: Vec<_> = filtered.iter().map(|e| e.group).collect();
        assert!(groups.contains(&CompletionGroup::Worktree));
        assert!(groups.contains(&CompletionGroup::Local));
    }

    #[test]
    fn rich_completions_exclude_current_false_keeps_current_in_worktree_group() {
        // When exclude_current is false (like daft-remove), the current
        // worktree branch should appear in the worktree group.
        let worktrees = vec![wt("main", "/repo/main"), wt("feat", "/repo/feat")];
        let local = vec![br("main", "1 day ago"), br("feat", "2 days ago")];

        // Pass None as current branch (exclude_current: false behavior)
        let entries = build_rich_completions(&worktrees, &local, &[], None, "origin", false, "");
        let wt_names: Vec<&str> = entries
            .iter()
            .filter(|e| e.group == CompletionGroup::Worktree)
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            wt_names.contains(&"main"),
            "main should be in worktree group"
        );
        assert!(
            wt_names.contains(&"feat"),
            "feat should be in worktree group"
        );

        // Compare with Some("main") — main removed from worktree group
        let entries =
            build_rich_completions(&worktrees, &local, &[], Some("main"), "origin", false, "");
        let wt_names: Vec<&str> = entries
            .iter()
            .filter(|e| e.group == CompletionGroup::Worktree)
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            !wt_names.contains(&"main"),
            "main should be excluded from worktree group"
        );
        assert!(
            wt_names.contains(&"feat"),
            "feat should remain in worktree group"
        );
    }

    #[test]
    fn format_entries_as_strings_produces_tab_separated_output() {
        let entries = vec![
            CompletionEntry {
                name: "main".to_string(),
                group: CompletionGroup::Worktree,
                description: "1 day ago\t/repo/main".to_string(),
            },
            CompletionEntry {
                name: "dev".to_string(),
                group: CompletionGroup::Local,
                description: "2 days ago".to_string(),
            },
        ];
        let strings = format_entries_as_strings(&entries);
        assert_eq!(strings[0], "main\tworktree\t1 day ago\t/repo/main");
        assert_eq!(strings[1], "dev\tlocal\t2 days ago");
    }
}
