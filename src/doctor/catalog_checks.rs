//! Doctor checks for the global repo catalog.
//!
//! The catalog is ambient state (updated as a side effect of daft
//! commands), so doctor is its hygiene tool: it surfaces entries that
//! drifted from reality and — with `--fix` — reconciles them. Issues are
//! aggregated per problem type (one check row listing every affected
//! repo) so a large catalog doesn't drown the report.

use crate::catalog::Catalog;
use crate::doctor::{CheckCategory, CheckResult, FixAction};
use crate::store::CatalogRepoRow;
use std::path::Path;

pub fn run_catalog_checks() -> CheckCategory {
    CheckCategory {
        title: "Catalog".to_string(),
        results: collect_results(),
    }
}

fn collect_results() -> Vec<CheckResult> {
    let rows = match Catalog::open_ro() {
        Ok(Some(catalog)) => match catalog.list(true) {
            Ok(rows) => rows,
            Err(e) => {
                return vec![CheckResult::fail(
                    "Catalog store",
                    &format!("unreadable: {e}"),
                )];
            }
        },
        Ok(None) => {
            return vec![CheckResult::pass(
                "Catalog store",
                "no catalog yet — repos register on first use",
            )];
        }
        Err(e) => {
            // SchemaTooNew and friends surface their own guidance.
            return vec![CheckResult::fail(
                "Catalog store",
                &format!("unreadable: {e}"),
            )];
        }
    };

    let live: Vec<&CatalogRepoRow> = rows.iter().filter(|r| r.removed_at.is_none()).collect();
    let removed = rows.len() - live.len();
    let mut results = vec![CheckResult::pass(
        "Catalog store",
        &format!("{} live entr(y/ies), {removed} removed", live.len()),
    )];

    results.push(stale_paths_check(&live));
    results.push(identity_check(&live));
    results.push(duplicate_names_check(&live));

    results
}

/// Live entries whose path no longer exists. Fix: mark them removed (the
/// standard tombstone — logs stay addressable, `daft clone <name>` restores).
fn stale_paths_check(live: &[&CatalogRepoRow]) -> CheckResult {
    let stale: Vec<(String, String)> = live
        .iter()
        .filter(|row| !Path::new(&row.path).is_dir())
        .map(|row| (row.uuid.clone(), format!("{} → {}", row.name, row.path)))
        .collect();

    if stale.is_empty() {
        return CheckResult::pass("Entry paths", "every live entry's path exists");
    }

    let uuids: Vec<String> = stale.iter().map(|(u, _)| u.clone()).collect();
    let labels: Vec<String> = stale.iter().map(|(_, l)| l.clone()).collect();
    let fix_uuids = uuids.clone();
    let dry_labels = labels.clone();
    CheckResult::warning(
        "Entry paths",
        &format!("{} live entr(y/ies) point at missing paths", stale.len()),
    )
    .with_details(labels)
    .with_suggestion("run with --fix to mark them removed (re-clone restores them by name)")
    .with_fix(Box::new(move || {
        let catalog = Catalog::open_rw().map_err(|e| e.to_string())?;
        for uuid in &fix_uuids {
            catalog.mark_removed(uuid).map_err(|e| e.to_string())?;
        }
        Ok(())
    }))
    .with_dry_run_fix(Box::new(move || {
        dry_labels
            .iter()
            .map(|label| FixAction {
                description: format!("Mark removed: {label}"),
                would_succeed: true,
                failure_reason: None,
            })
            .collect()
    }))
}

/// Live entries whose on-disk `daft-id` disagrees with the catalog (or is
/// gone). Fix: re-register from disk — the fresh identity takes over the
/// path and the stale row is retired, exactly like a re-clone.
fn identity_check(live: &[&CatalogRepoRow]) -> CheckResult {
    let mismatched: Vec<(String, String)> = live
        .iter()
        .filter(|row| Path::new(&row.path).is_dir())
        .filter(|row| {
            let on_disk = std::fs::read_to_string(Path::new(&row.git_common_dir).join("daft-id"))
                .map(|s| s.trim().to_string())
                .ok();
            on_disk.as_deref() != Some(row.uuid.as_str())
        })
        .map(|row| (row.name.clone(), row.git_common_dir.clone()))
        .collect();

    if mismatched.is_empty() {
        return CheckResult::pass("Identities", "every live entry matches its daft-id");
    }

    let labels: Vec<String> = mismatched
        .iter()
        .map(|(name, gcd)| format!("{name} ({gcd})"))
        .collect();
    let fix_targets = mismatched.clone();
    let dry_labels = labels.clone();
    CheckResult::warning(
        "Identities",
        &format!(
            "{} live entr(y/ies) disagree with the repo's daft-id",
            mismatched.len()
        ),
    )
    .with_details(labels)
    .with_suggestion("run with --fix to re-register from disk")
    .with_fix(Box::new(move || {
        let catalog = Catalog::open_rw().map_err(|e| e.to_string())?;
        for (_, gcd) in &fix_targets {
            let gcd = Path::new(gcd);
            let project_root = gcd.parent().unwrap_or(gcd);
            let facts = crate::catalog::gather_facts(gcd, project_root, None, None)
                .map_err(|e| e.to_string())?;
            catalog.register(&facts).map_err(|e| e.to_string())?;
        }
        Ok(())
    }))
    .with_dry_run_fix(Box::new(move || {
        dry_labels
            .iter()
            .map(|label| FixAction {
                description: format!("Re-register from disk: {label}"),
                would_succeed: true,
                failure_reason: None,
            })
            .collect()
    }))
}

/// Two live rows sharing a name should be impossible (partial unique
/// index); if it ever shows up, resolution is ambiguous — report loudly.
fn duplicate_names_check(live: &[&CatalogRepoRow]) -> CheckResult {
    let mut seen = std::collections::BTreeMap::new();
    for row in live {
        *seen.entry(row.name.as_str()).or_insert(0usize) += 1;
    }
    let dupes: Vec<String> = seen
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(name, n)| format!("{name} ×{n}"))
        .collect();
    if dupes.is_empty() {
        return CheckResult::pass("Names", "live names are unique");
    }
    CheckResult::fail("Names", "duplicate live names — resolution is ambiguous")
        .with_details(dupes)
        .with_suggestion(&format!(
            "rename with `{}` from inside each repo",
            crate::daft_cmd("repo add --name <name>")
        ))
}
