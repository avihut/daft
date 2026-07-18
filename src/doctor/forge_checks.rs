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
///
/// Probes the *configured* CLI binaries (`daft.forge.githubCli` /
/// `gitlabCli`) that the real checkout path uses — otherwise an Enterprise
/// user whose working `gh` is a wrapper (`gh-ent`, no plain `gh` on PATH) sees
/// a misleading "not installed" that contradicts a functioning integration.
pub fn run_forge_checks() -> CheckCategory {
    let config = crate::forge::ForgeConfig::load(&crate::git::GitCommand::new(true));
    let (gh, glab) = forge_binaries(&config);
    let mut results = vec![
        check_cli(&gh, "GitHub CLI", "https://cli.github.com/"),
        check_cli(
            &glab,
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

/// The `gh`/`glab` binary names to probe, honoring the configured overrides so
/// doctor checks the same binary the checkout path invokes. Defaults to the
/// canonical `gh`/`glab` when unset.
fn forge_binaries(config: &crate::forge::ForgeConfig) -> (String, String) {
    (
        config
            .github_cli
            .clone()
            .unwrap_or_else(|| "gh".to_string()),
        config
            .gitlab_cli
            .clone()
            .unwrap_or_else(|| "glab".to_string()),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::ForgeConfig;

    #[test]
    fn forge_binaries_honor_configured_overrides() {
        let cfg = ForgeConfig {
            github_cli: Some("gh-ent".to_string()),
            ..Default::default()
        };
        assert_eq!(
            forge_binaries(&cfg),
            ("gh-ent".to_string(), "glab".to_string()),
            "an Enterprise gh wrapper must be the binary doctor probes"
        );
        assert_eq!(
            forge_binaries(&ForgeConfig::default()),
            ("gh".to_string(), "glab".to_string()),
            "unset overrides fall back to the canonical binaries"
        );
    }
}
