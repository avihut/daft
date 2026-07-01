use super::{find_project_hooks, styled_trust_level};
use crate::hooks::{TrustDatabase, TrustLevel, get_remote_url_for_git_dir};
use crate::output::Output;
use crate::output::emit::{self, Cell, EmitArgs, EmitPayload, Table};
use crate::styles::{bold, cyan, dim, green, red, yellow};
use crate::{get_git_common_dir, is_git_repository};
use anyhow::{Context, Result};
use std::io::{self, IsTerminal, Write};
use std::path::Path;

/// Set trust level for the repository at the given path.
pub(super) fn cmd_set_trust(
    path: &Path,
    new_level: TrustLevel,
    force: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let abs_path = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(&abs_path)
        .with_context(|| format!("Cannot change to directory: {}", abs_path.display()))?;

    let result = (|| -> Result<()> {
        if !is_git_repository()? {
            anyhow::bail!("Not in a git repository: {}", abs_path.display());
        }

        let git_dir = get_git_common_dir()?;

        let hooks = find_project_hooks(&git_dir)?;
        let db = TrustDatabase::load().context("Failed to load trust database")?;
        let current_level = db.get_trust_level(&git_dir);
        let project_root = git_dir.parent().context("Invalid git directory")?;

        // Build hooks list string
        let hook_names: Vec<_> = hooks
            .iter()
            .filter_map(|h| h.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .collect();
        let hooks_str = if hook_names.is_empty() {
            "none".to_string()
        } else {
            hook_names.join(", ")
        };

        output.info(&format!("{}", project_root.display()));
        output.info(&format!("  Hooks: {hooks_str}"));

        if current_level == new_level {
            output.info(&format!(
                "  Trust: already at {}, nothing to do.",
                styled_trust_level(current_level)
            ));
            return Ok(());
        }

        if !force {
            print!(
                "  Trust: {} -> {}? [y/N] ",
                styled_trust_level(current_level),
                styled_trust_level(new_level)
            );
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            if input != "y" && input != "yes" {
                output.info(&dim("Aborted."));
                return Ok(());
            }
        }

        // Update and save the trust database
        let mut db = db;
        if let Some(fp) = get_remote_url_for_git_dir(&git_dir) {
            db.set_trust_level_with_fingerprint(&git_dir, new_level, fp);
        } else {
            db.set_trust_level(&git_dir, new_level);
        }
        db.save().context("Failed to save trust database")?;

        if force {
            output.info(&format!(
                "  Trust: {} -> {}",
                styled_trust_level(current_level),
                styled_trust_level(new_level)
            ));
        } else {
            output.result("Done.");
        }

        if new_level == TrustLevel::Allow {
            suggest_skipped_replays(&git_dir, project_root, output);
        }

        Ok(())
    })();

    std::env::set_current_dir(&original_dir)?;
    result
}

/// One "replay this hook" line: the hook to run and the live branches whose
/// recorded skip it would repair.
#[derive(Debug, PartialEq, Eq)]
struct ReplaySuggestion {
    hook_type: String,
    branches: Vec<String>,
}

/// Hooks worth suggesting a retroactive replay for. Only the idempotent
/// setup hooks qualify: pre-* hooks prepare an operation that already
/// happened, remove hooks tear down state (replaying one against a live
/// worktree is harmful), and merge hooks template against `DAFT_MERGE_*`
/// env that only the merge command injects.
const REPLAYABLE_HOOKS: [&str; 2] = ["post-clone", "worktree-post-create"];

/// Filter recorded skips down to actionable replay suggestions: replayable
/// hook types only, branches that still have a live worktree, grouped per
/// hook type in [`REPLAYABLE_HOOKS`] order.
fn replay_suggestions(
    rows: &[crate::store::models::InvocationRow],
    live_branches: &std::collections::HashSet<String>,
) -> Vec<ReplaySuggestion> {
    use std::collections::BTreeSet;

    REPLAYABLE_HOOKS
        .iter()
        .filter_map(|hook| {
            let branches: BTreeSet<&str> = rows
                .iter()
                .filter(|r| r.hook_type == *hook && live_branches.contains(&r.worktree))
                .map(|r| r.worktree.as_str())
                .collect();
            (!branches.is_empty()).then(|| ReplaySuggestion {
                hook_type: hook.to_string(),
                branches: branches.into_iter().map(String::from).collect(),
            })
        })
        .collect()
}

/// After a trust grant, surface the hooks that were skipped while the
/// repository was untrusted and offer the replay commands. Best-effort
/// throughout — trusting must never fail because of the store.
fn suggest_skipped_replays(git_dir: &Path, project_root: &Path, output: &mut dyn Output) {
    use crate::coordinator::ports::JobsStorePort;
    use crate::core::worktree::remove_repo::{RepoTarget, enumerate_worktrees};
    use crate::store::paths::{COORDINATOR_DB, JOBS_SUBDIR};

    let Ok(repo_hash) = crate::core::repo_identity::compute_repo_id_from_common_dir(git_dir) else {
        return;
    };
    let Ok(state_base) = crate::daft_state_dir() else {
        return;
    };
    // Existence check before opening: `for_repo_base` would create the
    // per-repo state dir, and a repo with no hook history has none.
    let base = state_base.join(JOBS_SUBDIR).join(&repo_hash);
    if !base.join(COORDINATOR_DB).exists() {
        return;
    }
    let Ok(store) = crate::coordinator::adapters::SqliteJobsStore::for_repo_base(&base) else {
        return;
    };
    let Ok(rows) = store.list_skipped_invocations(&repo_hash) else {
        return;
    };
    if rows.is_empty() {
        return;
    }

    let target = RepoTarget {
        bare_git_dir: git_dir.to_path_buf(),
        project_root: project_root.to_path_buf(),
    };
    let Ok(entries) = enumerate_worktrees(&target, false) else {
        return;
    };
    let live: std::collections::HashSet<String> =
        entries.into_iter().filter_map(|e| e.branch).collect();

    let suggestions = replay_suggestions(&rows, &live);
    if suggestions.is_empty() {
        return;
    }

    output.info("");
    output.info(&bold(
        "Hooks were skipped here while the repository was untrusted. Replay them:",
    ));
    let exe = crate::cli_label();
    for s in &suggestions {
        output.info(&format!(
            "  {}   {}",
            cyan(&format!("{exe} hooks run {}", s.hook_type)),
            dim(&format!("# in {}", s.branches.join(", "))),
        ));
    }
}

/// Revoke trust for the repository at the given path.
pub(super) fn cmd_deny(path: &Path, force: bool, output: &mut dyn Output) -> Result<()> {
    let abs_path = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(&abs_path)
        .with_context(|| format!("Cannot change to directory: {}", abs_path.display()))?;

    let result = (|| -> Result<()> {
        if !is_git_repository()? {
            anyhow::bail!("Not in a git repository: {}", abs_path.display());
        }

        let git_dir = get_git_common_dir()?;
        let db = TrustDatabase::load().context("Failed to load trust database")?;
        let current_level = db.get_trust_level(&git_dir);
        let project_root = git_dir.parent().context("Invalid git directory")?;

        if !db.has_explicit_trust(&git_dir) {
            output.info(&format!("{}", project_root.display()));
            output.info(&dim("  Not explicitly trusted"));
            return Ok(());
        }

        let hooks = find_project_hooks(&git_dir)?;
        let hook_names: Vec<_> = hooks
            .iter()
            .filter_map(|h| h.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .collect();
        let hooks_str = if hook_names.is_empty() {
            "none".to_string()
        } else {
            hook_names.join(", ")
        };

        output.info(&format!("{}", project_root.display()));
        output.info(&format!("  Hooks: {hooks_str}"));

        if !force {
            print!(
                "  Trust: {} -> {}? [y/N] ",
                styled_trust_level(current_level),
                styled_trust_level(TrustLevel::Deny)
            );
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            if input != "y" && input != "yes" {
                output.info(&dim("Aborted."));
                return Ok(());
            }
        }

        let mut db = db;
        db.remove_trust(&git_dir);
        db.save().context("Failed to save trust database")?;

        if force {
            output.info(&format!(
                "  Trust: {} -> {}",
                styled_trust_level(current_level),
                styled_trust_level(TrustLevel::Deny)
            ));
        } else {
            output.result("Done.");
        }

        Ok(())
    })();

    std::env::set_current_dir(&original_dir)?;
    result
}

/// Prune stale entries from the trust database.
pub(super) fn cmd_prune(output: &mut dyn Output) -> Result<()> {
    let mut db = TrustDatabase::load().context("Failed to load trust database")?;

    let removed = db.prune();
    let backfilled = db.backfill_fingerprints();

    if removed.is_empty() && backfilled == 0 {
        output.info(&dim("No stale entries found."));
        return Ok(());
    }

    db.save().context("Failed to save trust database")?;

    if !removed.is_empty() {
        output.info(&bold("Pruned stale entries:"));
        for path in &removed {
            let repo_path = path.strip_suffix("/.git").unwrap_or(path);
            output.info(&format!("  {}", dim(repo_path)));
        }
        output.result(&format!(
            "Removed {} stale {}.",
            green(&removed.len().to_string()),
            if removed.len() == 1 {
                "entry"
            } else {
                "entries"
            }
        ));
    }

    if backfilled > 0 {
        output.result(&format!(
            "Backfilled fingerprints for {} {}.",
            green(&backfilled.to_string()),
            if backfilled == 1 {
                "repository"
            } else {
                "repositories"
            }
        ));
    }

    Ok(())
}

/// List all trusted repositories.
pub(super) fn cmd_list(
    show_all: bool,
    emit_args: &EmitArgs,
    output: &mut dyn Output,
) -> Result<()> {
    let db = TrustDatabase::load().context("Failed to load trust database")?;

    let repos: Vec<(&str, &crate::hooks::TrustEntry)> = if show_all {
        db.repositories
            .iter()
            .map(|(p, e)| (p.as_str(), e))
            .collect()
    } else {
        db.list_trusted()
    };

    if emit_args.is_structured() {
        let table = build_trust_table(&repos);
        return emit::emit_and_handle(
            "hooks trust list",
            EmitPayload::Tabular(table),
            emit_args,
            &mut std::io::stdout(),
        )
        .map_err(|e| anyhow::anyhow!("{e}"));
    }

    if repos.is_empty() {
        if show_all {
            output.info(&dim("No repositories in trust database."));
        } else {
            output.info(&dim("No trusted repositories."));
            output.info("");
            output.info(&bold("To trust a repository, cd into it and run:"));
            output.info(&format!("  {}", cyan(&crate::daft_cmd("hooks trust"))));
        }
        return Ok(());
    }

    // Build output text
    let mut text = String::new();

    let title = if show_all {
        bold("All repositories in trust database:")
    } else {
        bold("Trusted repositories:")
    };
    text.push_str(&title);
    text.push_str("\n\n");

    for (path, entry) in &repos {
        // Strip .git suffix if present to show repo path
        let repo_path = path.strip_suffix("/.git").unwrap_or(path);
        // Truncate long paths
        let display_path = if repo_path.len() > 60 {
            format!("...{}", &repo_path[repo_path.len() - 57..])
        } else {
            repo_path.to_string()
        };
        let display_time = entry.formatted_time();
        text.push_str(&format!("  {display_path}\n"));
        text.push_str(&format!(
            "    Level: {}  {}\n",
            styled_trust_level(entry.level),
            dim(&format!("(trusted: {display_time})"))
        ));
        if let Some(ref fp) = entry.fingerprint {
            text.push_str(&format!("    Remote: {}\n", dim(fp)));
        }
    }

    // Show patterns if any
    if !db.patterns.is_empty() {
        text.push_str(&format!("\n{}:\n", bold("Trust patterns")));
        for pattern in &db.patterns {
            let comment = pattern
                .comment
                .as_ref()
                .map(|c| format!(" {}", dim(&format!("# {c}"))))
                .unwrap_or_default();
            text.push_str(&format!(
                "  {} -> {}{comment}\n",
                cyan(&pattern.pattern),
                styled_trust_level(pattern.level)
            ));
        }
    }

    // Use pager if output is long and we're in a terminal
    let line_count = text.lines().count();
    if line_count > 20 && std::io::stdout().is_terminal() {
        crate::output::pager::display_with_pager(&text);
    } else {
        output.raw(&text);
    }

    Ok(())
}

/// Build a structured `Table` payload from trust entries.
fn build_trust_table(repos: &[(&str, &crate::hooks::TrustEntry)]) -> Table {
    use chrono::{TimeZone, Utc};

    let mut table = Table::new([
        "repo_path",
        "trust_level",
        "remote_fingerprint",
        "timestamp",
    ]);
    for (path, entry) in repos {
        let repo_path = path.strip_suffix("/.git").unwrap_or(path);
        let timestamp = Utc
            .timestamp_opt(entry.granted_at, 0)
            .single()
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_string());
        table = table.row([
            Cell::str(repo_path),
            Cell::str(entry.level.to_string()),
            Cell::str(entry.fingerprint.as_deref().unwrap_or("")),
            Cell::str(timestamp),
        ]);
    }
    table
}

/// Clear all trust settings.
pub(super) fn cmd_reset_trust(force: bool, output: &mut dyn Output) -> Result<()> {
    let db = TrustDatabase::load().context("Failed to load trust database")?;

    let repo_count = db.repositories.len();
    let pattern_count = db.patterns.len();

    if repo_count == 0 && pattern_count == 0 {
        output.info(&dim("Trust database is already empty."));
        return Ok(());
    }

    output.info(&format!(
        "Trust database: {} repositories, {} patterns",
        yellow(&repo_count.to_string()),
        yellow(&pattern_count.to_string())
    ));

    if !force {
        print!("{} all trust settings? [y/N] ", red("Clear"));
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            output.info(&dim("Aborted."));
            return Ok(());
        }
    }

    let mut db = db;
    db.clear();
    db.save().context("Failed to save trust database")?;

    if force {
        output.result("Trust database cleared.");
    } else {
        output.result("Done.");
    }

    Ok(())
}

/// Remove the trust entry for a specific repository path.
pub(super) fn cmd_reset_trust_path(
    path: &Path,
    force: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let abs_path = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(&abs_path)
        .with_context(|| format!("Cannot change to directory: {}", abs_path.display()))?;

    let result = (|| -> Result<()> {
        if !is_git_repository()? {
            anyhow::bail!("Not in a git repository: {}", abs_path.display());
        }

        let git_dir = get_git_common_dir()?;
        let db = TrustDatabase::load().context("Failed to load trust database")?;
        let project_root = git_dir.parent().context("Invalid git directory")?;

        if !db.has_explicit_trust(&git_dir) {
            output.info(&format!("{}", project_root.display()));
            output.info(&dim("  No explicit trust entry to remove."));
            return Ok(());
        }

        let current_level = db.get_trust_level(&git_dir);
        output.info(&format!("{}", project_root.display()));
        output.info(&format!("  Trust: {}", styled_trust_level(current_level)));

        if !force {
            print!("  {} trust entry? [y/N] ", red("Remove"));
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            if input != "y" && input != "yes" {
                output.info(&dim("Aborted."));
                return Ok(());
            }
        }

        let mut db = db;
        db.remove_trust(&git_dir);
        db.save().context("Failed to save trust database")?;

        if force {
            output.result(&dim("  Trust entry removed."));
        } else {
            output.result("Done.");
        }

        Ok(())
    })();

    std::env::set_current_dir(&original_dir)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::models::InvocationRow;
    use crate::store::models::invocation::{INVOCATION_STATUS_SKIPPED, SKIP_REASON_UNTRUSTED};
    use std::collections::HashSet;

    fn skip_row(hook_type: &str, worktree: &str) -> InvocationRow {
        InvocationRow {
            repo_hash: "r".into(),
            invocation_id: format!("{hook_type}-{worktree}"),
            trigger_command: "checkout".into(),
            hook_type: hook_type.into(),
            worktree: worktree.into(),
            created_at: chrono::Utc::now(),
            coordinator_pid: None,
            status: INVOCATION_STATUS_SKIPPED.into(),
            skip_reason: Some(SKIP_REASON_UNTRUSTED.into()),
        }
    }

    fn live(branches: &[&str]) -> HashSet<String> {
        branches.iter().map(|b| b.to_string()).collect()
    }

    #[test]
    fn suggests_only_replayable_hook_types() {
        let rows = vec![
            skip_row("worktree-pre-create", "feat/a"),
            skip_row("worktree-post-create", "feat/a"),
            skip_row("worktree-pre-remove", "feat/a"),
            skip_row("worktree-post-remove", "feat/a"),
            skip_row("pre-merge", "feat/a"),
            skip_row("post-merge", "feat/a"),
            skip_row("post-clone", "main"),
        ];
        let suggestions = replay_suggestions(&rows, &live(&["main", "feat/a"]));
        assert_eq!(
            suggestions,
            vec![
                ReplaySuggestion {
                    hook_type: "post-clone".into(),
                    branches: vec!["main".into()],
                },
                ReplaySuggestion {
                    hook_type: "worktree-post-create".into(),
                    branches: vec!["feat/a".into()],
                },
            ]
        );
    }

    #[test]
    fn excludes_branches_without_a_live_worktree() {
        let rows = vec![
            skip_row("worktree-post-create", "feat/alive"),
            skip_row("worktree-post-create", "feat/removed"),
        ];
        let suggestions = replay_suggestions(&rows, &live(&["feat/alive"]));
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].branches, vec!["feat/alive".to_string()]);
    }

    #[test]
    fn groups_branches_per_hook_type_sorted_and_deduped() {
        let rows = vec![
            skip_row("worktree-post-create", "feat/b"),
            skip_row("worktree-post-create", "feat/a"),
            skip_row("worktree-post-create", "feat/a"),
        ];
        let suggestions = replay_suggestions(&rows, &live(&["feat/a", "feat/b"]));
        assert_eq!(suggestions.len(), 1);
        assert_eq!(
            suggestions[0].branches,
            vec!["feat/a".to_string(), "feat/b".to_string()]
        );
    }

    #[test]
    fn empty_when_nothing_actionable() {
        assert!(replay_suggestions(&[], &live(&["main"])).is_empty());
        let rows = vec![skip_row("worktree-pre-remove", "main")];
        assert!(replay_suggestions(&rows, &live(&["main"])).is_empty());
    }
}
