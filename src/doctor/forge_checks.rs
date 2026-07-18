//! Doctor checks for the forge CLIs (`gh` / `glab`).
//!
//! Forge integration is optional and pure passthrough (#127): daft never owns
//! auth, so a missing/unauthenticated CLI is a `Skipped`/`Warning`, never a
//! `Fail`. This is the one place daft proactively probes auth — per-operation
//! preflighting is deliberately avoided (the real call's error is enough).

use std::process::Command;

use crate::doctor::{CheckCategory, CheckResult};

/// Run the forge tooling checks: `gh` for GitHub PRs, `glab` for GitLab MRs,
/// plus this repo's persisted forge health when it is hiding the `pr` column.
pub fn run_forge_checks() -> CheckCategory {
    let mut results = vec![
        check_cli("gh", "GitHub CLI", "https://cli.github.com/"),
        check_cli(
            "glab",
            "GitLab CLI",
            "https://gitlab.com/gitlab-org/cli#installation",
        ),
    ];
    results.extend(check_repo_health());
    CheckCategory {
        title: "Forge integration".to_string(),
        results,
    }
}

/// Surface a persisted deep refresh failure — the record that silently hides
/// the default `pr` column in `daft list`. Doctor is where the silence gets
/// explained; nothing is shown when outside a repo, no failure is on record,
/// or the store is unreadable.
fn check_repo_health() -> Option<CheckResult> {
    let repo_hash = crate::core::repo_identity::compute_repo_id().ok()?;
    let health = crate::commands::forge_cache::read_health(&repo_hash)?;
    if health.healthy {
        return None;
    }
    let reason = match health.error_kind.as_deref() {
        Some("missing-tool") => "the forge CLI is not installed",
        Some("unauthenticated") => "the forge CLI is not authenticated",
        Some("repo-access") => "the forge repository is not accessible",
        _ => "the last background refresh hit a persistent failure",
    };
    Some(
        CheckResult::warning("PR column", &format!("hidden in `daft list`: {reason}"))
            .with_suggestion(
                "Fix the underlying issue (e.g. `gh auth login`); the next background \
             refresh detects it and restores the column automatically",
            ),
    )
}

/// Probe one forge CLI: installed? version? authenticated?
fn check_cli(bin: &str, label: &str, install_url: &str) -> CheckResult {
    if which::which(bin).is_err() {
        return CheckResult::skipped(label, "not installed (optional)").with_suggestion(&format!(
            "Install {bin} from {install_url} to check out pull/merge requests with daft"
        ));
    }

    let version = Command::new(bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty());

    // `auth status` reads local config + keyring — no network. A non-zero exit
    // means not logged in.
    let authed = Command::new(bin)
        .args(["auth", "status"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let version_note = version
        .as_deref()
        .map(|v| format!(" ({v})"))
        .unwrap_or_default();

    if authed {
        CheckResult::pass(label, &format!("installed and authenticated{version_note}"))
    } else {
        CheckResult::warning(
            label,
            &format!("installed but not authenticated{version_note}"),
        )
        .with_suggestion(&format!("Run `{bin} auth login` to enable PR/MR checkout"))
    }
}
