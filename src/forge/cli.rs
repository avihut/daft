//! Small shared runner for shelling out to `gh` / `glab`.
//!
//! Pure CLI passthrough (#127): daft never speaks HTTP or stores tokens — every
//! forge call shells out to a CLI that inherits the user's existing auth. This
//! module is the ~one place that spawns them, so the prompt-disable env, the
//! not-installed-vs-failed distinction, and error-detail extraction live in one
//! spot rather than scattered across providers.

use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};

/// Typed marker carried in the error chain when a forge call failed in a way
/// that will *keep* failing until the user intervenes — as opposed to a
/// transient network/API hiccup. The background refresh downcasts for it
/// ([`classify_unavailable`]) to record repo forge health, which decides
/// whether `daft list` shows its default `pr` column. Attached as the error
/// *source* (via `.context(...)`), so user-facing messages are unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeUnavailable {
    /// The gh/glab binary isn't installed.
    MissingTool,
    /// The CLI ran but isn't authenticated (dead/expired login).
    Unauthenticated,
    /// The CLI is authenticated but cannot see the repository (revoked
    /// access, repo deleted or renamed).
    RepoAccess,
}

impl ForgeUnavailable {
    /// The TEXT persisted in the store's `forge_health.error_kind` column.
    pub fn kind_str(self) -> &'static str {
        match self {
            ForgeUnavailable::MissingTool => "missing-tool",
            ForgeUnavailable::Unauthenticated => "unauthenticated",
            ForgeUnavailable::RepoAccess => "repo-access",
        }
    }
}

impl std::fmt::Display for ForgeUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ForgeUnavailable::MissingTool => "forge CLI is not installed",
            ForgeUnavailable::Unauthenticated => "forge CLI is not authenticated",
            ForgeUnavailable::RepoAccess => "forge repository is not accessible",
        })
    }
}

impl std::error::Error for ForgeUnavailable {}

/// Recover the [`ForgeUnavailable`] marker from anywhere in an error chain.
/// `None` means the failure is transient (network, rate limit, API change)
/// and must not flip the repo's forge health.
pub fn classify_unavailable(err: &anyhow::Error) -> Option<ForgeUnavailable> {
    err.downcast_ref::<ForgeUnavailable>().copied()
}

/// One `gh`/`glab` invocation.
pub struct CliApiRequest<'a> {
    /// Binary to run (`gh` / `glab`, or a config override).
    pub tool: &'a str,
    /// Arguments (e.g. `["api", "repos/o/r/pulls/1"]`).
    pub args: &'a [&'a str],
    /// Working directory — the CLI reads repo/auth context from here.
    pub repo_root: &'a Path,
    /// `(NAME, VALUE)` to disable interactive prompts
    /// (`GH_PROMPT_DISABLED=1` / `GLAB_NO_PROMPT=1`) so a missing/expired auth
    /// surfaces as an error instead of hanging on a prompt.
    pub prompt_env: (&'a str, &'a str),
    /// Additional env vars for this invocation — the escape hatch for CLI
    /// options that have no flag spelling (e.g. `GH_HOST` for `gh pr list`,
    /// whose `--hostname` exists only on `gh api`). Usually empty.
    pub extra_env: &'a [(&'a str, &'a str)],
    /// Shown (as the whole error) when the tool isn't installed.
    pub install_hint: &'a str,
    /// Context wrapped around a spawn failure that isn't "not found".
    pub run_context: &'a str,
}

/// Run a CLI API request. Distinguishes "tool not installed" (→ the install
/// hint) from "tool ran" (→ `Ok(Output)`, even on a non-zero exit — the caller
/// inspects `output.status` and the body).
pub fn run_cli_api(request: CliApiRequest<'_>) -> Result<Output> {
    match Command::new(request.tool)
        .args(request.args.iter().copied())
        .current_dir(request.repo_root)
        .env(request.prompt_env.0, request.prompt_env.1)
        .envs(request.extra_env.iter().copied())
        .output()
    {
        Ok(output) => Ok(output),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            Err(anyhow::Error::new(ForgeUnavailable::MissingTool)
                .context(request.install_hint.to_string()))
        }
        Err(error) => Err(anyhow::Error::from(error).context(request.run_context.to_string())),
    }
}

/// Best error detail from a failed invocation: stderr, falling back to stdout
/// (some CLIs print the API error body to stdout).
pub fn error_details(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr.trim().to_string()
    }
}

/// Read a `<tool> config get <key>` value (e.g. `git_protocol`). `None` if the
/// tool is missing, the key is unset, or the value is empty.
pub fn config_value(tool: &str, key: &str) -> Option<String> {
    Command::new(tool)
        .args(["config", "get", key])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Wrap a "tool ran but the request failed" output as an error, with the run
/// context and the extracted detail. Used as the fallback after providers have
/// tried to recognise specific status codes.
pub fn generic_api_error(run_context: &str, output: &Output) -> anyhow::Error {
    anyhow::anyhow!("{run_context}: {}", error_details(output))
}

/// Extract the host from a PR/MR `html_url` (`https://host/...`).
pub fn host_from_url(url: &str) -> Result<String> {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .and_then(|s| s.split('/').next())
        .filter(|h| !h.is_empty())
        .map(String::from)
        .with_context(|| format!("could not parse host from URL: {url}"))
}

/// Construct an [`ExitStatus`](std::process::ExitStatus) carrying a specific
/// exit `code`, portably across Unix and Windows, for tests that exercise
/// forge-CLI failure classification. Unix packs the code into the high byte of
/// the wait status; Windows stores it directly — both yield
/// `.code() == Some(code)` and `.success() == (code == 0)`.
#[cfg(test)]
pub(crate) fn exit_status_with_code(code: i32) -> std::process::ExitStatus {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;

    #[cfg(unix)]
    let raw = code << 8;
    #[cfg(windows)]
    let raw = code as u32;

    std::process::ExitStatus::from_raw(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Output;

    fn output(stdout: &str, stderr: &str) -> Output {
        Output {
            status: exit_status_with_code(1),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn unavailable_marker_survives_context_wrapping() {
        let err = anyhow::Error::new(ForgeUnavailable::MissingTool)
            .context("GitHub CLI (gh) is not installed.");
        assert_eq!(
            classify_unavailable(&err),
            Some(ForgeUnavailable::MissingTool)
        );
        // The context, not the marker, is what the user sees.
        assert!(err.to_string().contains("not installed"));

        let transient = anyhow::anyhow!("connect: network is unreachable");
        assert_eq!(classify_unavailable(&transient), None);
    }

    #[test]
    fn error_details_prefers_stderr() {
        assert_eq!(error_details(&output("out", "the error")), "the error");
    }

    #[test]
    fn error_details_falls_back_to_stdout() {
        assert_eq!(error_details(&output("body error", "   ")), "body error");
    }

    #[test]
    fn host_from_url_parses_scheme_host() {
        assert_eq!(
            host_from_url("https://github.com/o/r/pull/1").unwrap(),
            "github.com"
        );
        assert_eq!(
            host_from_url("http://gitlab.example.com/g/r/-/merge_requests/2").unwrap(),
            "gitlab.example.com"
        );
        assert!(host_from_url("not a url").is_err());
    }
}
