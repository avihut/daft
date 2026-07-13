//! Doctor checks for the forge CLIs (`gh` / `glab`).
//!
//! Forge integration is optional and pure passthrough (#127): daft never owns
//! auth, so a missing/unauthenticated CLI is a `Skipped`/`Warning`, never a
//! `Fail`. This is the one place daft proactively probes auth — per-operation
//! preflighting is deliberately avoided (the real call's error is enough).

use std::process::Command;

use crate::doctor::{CheckCategory, CheckResult};

/// Run the forge tooling checks: `gh` for GitHub PRs, `glab` for GitLab MRs.
pub fn run_forge_checks() -> CheckCategory {
    CheckCategory {
        title: "Forge integration".to_string(),
        results: vec![
            check_cli("gh", "GitHub CLI", "https://cli.github.com/"),
            check_cli(
                "glab",
                "GitLab CLI",
                "https://gitlab.com/gitlab-org/cli#installation",
            ),
        ],
    }
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
