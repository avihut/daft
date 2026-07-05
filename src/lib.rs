// Forbid unsafe in production code. Tests are exempt because they need
// `unsafe { env::set_var/remove_var }` for process-wide env mutations
// (became `unsafe fn` in edition 2024). The forbid lint can't be
// `#[allow]`-overridden by inner attributes, so this `cfg_attr` is the
// canonical pattern for "no unsafe in production, but tests may have it."
#![cfg_attr(not(test), forbid(unsafe_code))]

use std::env;
use std::path::Path;

/// Clean version string from Cargo.toml, used by clap attributes and man pages.
pub const VERSION: &str = env!("DAFT_VERSION");

/// Display version for `daft --version`. Includes branch and commit hash in dev builds.
pub const VERSION_DISPLAY: &str = env!("DAFT_VERSION_DISPLAY");

/// Environment variable containing the path to a temp file where the shell
/// wrapper expects the cd target to be written.
pub const CD_FILE_ENV: &str = "DAFT_CD_FILE";

/// Environment variable to override the config directory path.
///
/// When set, all daft state files (trust database, hints, update cache, etc.)
/// are stored in this directory instead of `~/.config/daft/`.
///
/// Only honored in dev builds (local builds from a git checkout). Release
/// builds (tagged commits, `DAFT_BUILD_RELEASE=1`, crates.io installs) ignore
/// this variable to prevent trust database hijacking via env injection.
pub const CONFIG_DIR_ENV: &str = "DAFT_CONFIG_DIR";

/// Environment variable to override the data directory path.
///
/// When set, centralized layout worktrees and other application data are stored
/// in this directory instead of the XDG data directory (`~/.local/share/daft/`).
///
/// Only honored in dev builds (same policy as `DAFT_CONFIG_DIR`).
pub const DATA_DIR_ENV: &str = "DAFT_DATA_DIR";

/// Environment variable to override the state directory path.
///
/// When set, coordinator sockets, background job logs, and other runtime
/// state are stored in this directory instead of the XDG state directory
/// (`~/.local/state/daft/`).
///
/// Only honored in dev builds (same policy as `DAFT_CONFIG_DIR`).
pub const STATE_DIR_ENV: &str = "DAFT_STATE_DIR";

/// Returns the daft config directory path.
///
/// In dev builds (git-checkout builds without `DAFT_BUILD_RELEASE`; see
/// build.rs) and in unit tests, when `DAFT_CONFIG_DIR` is set to a non-empty
/// absolute path, uses that path directly (no `daft/` suffix appended).
/// Release builds ignore the env var. Always falls back to
/// `dirs::config_dir()/daft`.
pub fn daft_config_dir() -> anyhow::Result<std::path::PathBuf> {
    use std::path::PathBuf;
    if cfg!(any(daft_dev_build, test))
        && let Ok(dir) = env::var(CONFIG_DIR_ENV)
        && !dir.is_empty()
    {
        let path = PathBuf::from(&dir);
        if path.is_relative() {
            anyhow::bail!("DAFT_CONFIG_DIR must be an absolute path, got: {dir}");
        }
        return Ok(path);
    }
    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;
    Ok(config_dir.join("daft"))
}

/// Returns the daft data directory path.
///
/// In dev builds (git-checkout builds without `DAFT_BUILD_RELEASE`; see
/// build.rs) and in unit tests, when `DAFT_DATA_DIR` is set to a non-empty
/// absolute path, uses that path directly (no `daft/` suffix appended).
/// Release builds ignore the env var. Always falls back to
/// `dirs::data_dir()/daft`.
pub fn daft_data_dir() -> anyhow::Result<std::path::PathBuf> {
    use std::path::PathBuf;
    if cfg!(any(daft_dev_build, test))
        && let Ok(dir) = env::var(DATA_DIR_ENV)
        && !dir.is_empty()
    {
        let path = PathBuf::from(&dir);
        if path.is_relative() {
            anyhow::bail!("DAFT_DATA_DIR must be an absolute path, got: {dir}");
        }
        return Ok(path);
    }
    let data_dir =
        dirs::data_dir().ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?;
    Ok(data_dir.join("daft"))
}

/// Returns the daft state directory path.
///
/// In dev builds (git-checkout builds without `DAFT_BUILD_RELEASE`; see
/// build.rs) and in unit tests, when `DAFT_STATE_DIR` is set to a non-empty
/// absolute path, uses that path directly (no `daft/` suffix appended).
/// Release builds ignore the env var. Always falls back to
/// `dirs::state_dir()/daft` (macOS: `~/.local/state/daft`, Linux:
/// `$XDG_STATE_HOME/daft`).
pub fn daft_state_dir() -> anyhow::Result<std::path::PathBuf> {
    use std::path::PathBuf;
    if cfg!(any(daft_dev_build, test))
        && let Ok(dir) = env::var(STATE_DIR_ENV)
        && !dir.is_empty()
    {
        let path = PathBuf::from(&dir);
        if path.is_relative() {
            anyhow::bail!("DAFT_STATE_DIR must be an absolute path, got: {dir}");
        }
        return Ok(path);
    }
    // dirs::state_dir() returns None on macOS (no native equivalent).
    // Fall back to ~/.local/state which is the XDG convention.
    let state_dir = dirs::state_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .expect("Could not determine home directory")
            .join(".local")
            .join("state")
    });
    Ok(state_dir.join("daft"))
}

/// Daft verb aliases that route through to worktree commands.
const DAFT_VERBS: &[&str] = &[
    "adopt", "carry", "clone", "eject", "exec", "go", "init", "list", "merge", "prune", "remove",
    "rename", "start", "sync", "update",
];

/// Returns args suitable for clap parsing, handling symlink, subcommand, and verb invocations.
///
/// When invoked via symlink (e.g., `git-worktree-clone <url>`), returns args as-is.
/// When invoked via `daft worktree-<cmd> <args>` or `daft <verb> <args>`, returns
/// `["git-worktree-<cmd>", <args>...]` so clap sees the expected command name.
///
/// # Arguments
/// * `expected_cmd` - The expected command name (e.g., "git-worktree-clone")
pub fn get_clap_args(expected_cmd: &str) -> Vec<String> {
    let args = cli::argv();

    // Check if invoked as `daft worktree-*` or `daft <verb>`
    if args.len() >= 2 {
        let program_name = Path::new(&args[0])
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if (program_name == "daft" || program_name == "git-daft")
            && (args[1].starts_with("worktree-") || DAFT_VERBS.contains(&args[1].as_str()))
        {
            // Reconstruct args with the expected command name
            let mut new_args = vec![expected_cmd.to_string()];
            new_args.extend(args.iter().skip(2).cloned());
            return new_args;
        }
    }

    // Default: return args as-is (symlink invocation)
    args.to_vec()
}

/// The daft executable as the user invoked it, for rendering suggested
/// commands in user-facing output: `"daft"` when invoked directly (`daft`,
/// `daft-go`, …), `"git daft"` when invoked through git (`git daft <verb>`,
/// `git worktree-checkout`, …).
///
/// Returns the canonical `"git daft"` when argv isn't installed, and always
/// under `cfg!(test)`: unit tests share one process, so the `daft-<hash>`
/// test-binary name (or a test that installed argv) would otherwise flip
/// other tests' expected hint strings order-dependently.
pub fn cli_label() -> &'static str {
    if cfg!(test) {
        return "git daft";
    }
    cli::try_argv()
        .and_then(|args| args.first())
        .map(|argv0| label_for_argv0(argv0))
        .unwrap_or("git daft")
}

/// Classify an argv\[0\] into the display label. `daft` and `daft-*`
/// (`daft-go`, `daft-start`, …) are direct-style; everything else — `git-daft`,
/// the `git-worktree-*` symlinks, shortcut symlinks, unknown names — renders
/// git-style, which is also the canonical documented form.
fn label_for_argv0(argv0: &str) -> &'static str {
    let name = Path::new(argv0)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if name == "daft" || name.starts_with("daft-") {
        "daft"
    } else {
        "git daft"
    }
}

/// Format a suggested daft invocation in the current invocation style:
/// `daft_cmd("hooks trust")` → `daft hooks trust` or `git daft hooks trust`.
pub fn daft_cmd(args: &str) -> String {
    format!("{} {args}", cli_label())
}

/// Returns `true` if the given argv corresponds to an invocation that should
/// skip startup-time background work (update checks, trust pruning, etc.).
///
/// `shell-init` and `completions` both emit shell code to stdout that users
/// `eval` from their shell rc files, so they run on every interactive shell
/// startup. Their codepaths must stay free of subprocess calls, file IO,
/// network requests, and background-process spawns. They must also stay quiet
/// on stderr — `eval` only captures stdout, so banners on stderr (e.g., the
/// update-available notice) leak straight into the user's terminal.
///
/// `__*` subcommands are background tasks (e.g., `__check-update`,
/// `__prune-trust`, and the `__complete` tab-completion helper). Skipping
/// further background work for them prevents recursive fork bombs and keeps
/// tab completion responsive.
///
/// New commands with similar constraints should be added here rather than
/// introducing a parallel gate at the call sites. Callers that need the
/// composed test-mode/coordinator gates as well should use
/// [`should_skip_background_tasks`] instead.
pub fn skip_startup_tasks_for(args: &[String]) -> bool {
    let Some(sub) = args.get(1).map(String::as_str) else {
        return false;
    };
    sub.starts_with("__") || matches!(sub, "shell-init" | "completions")
}

/// Whether daft's startup-time background work should be skipped for this
/// invocation. Composes three independent gates, each of which is sufficient
/// on its own:
///
/// 1. [`skip_startup_tasks_for`] — argv-driven gate for invocations whose
///    codepaths must stay lean (shell-init, completions, `__*` background
///    tasks). See its doc comment for the security/perf rationale.
/// 2. `DAFT_IS_COORDINATOR` — set by the hook coordinator process when it
///    spawns daft children, so daemons don't spawn further daemons.
/// 3. `DAFT_TESTING` — set by the YAML manual-test runner on every step.
///    Suppresses update-check/trust-prune/log-clean spawns that would
///    otherwise reparent under PID 1 and pile up as orphans across the
///    suite's ~2000 invocations.
///
/// Each gate has its own surface: argv shape, coordinator env var, runner env
/// var. Composing them here means main.rs makes the dispatch decision once,
/// instead of every `maybe_*` background-spawn helper re-deriving the same
/// "should I run?" condition from per-feature env vars (`DAFT_NO_UPDATE_CHECK`
/// etc.). Those per-feature flags remain as user-facing opt-outs.
pub fn should_skip_background_tasks(args: &[String]) -> bool {
    should_skip_background_tasks_impl(
        skip_startup_tasks_for(args),
        std::env::var("DAFT_IS_COORDINATOR").is_ok(),
        std::env::var("DAFT_TESTING").is_ok(),
    )
}

fn should_skip_background_tasks_impl(
    skip_startup_tasks: bool,
    is_coordinator: bool,
    is_test_mode: bool,
) -> bool {
    skip_startup_tasks || is_coordinator || is_test_mode
}

pub mod cli;
pub mod commands;
pub mod completion_spinner;
pub mod coordinator;
pub mod core;
pub mod cow_copy;
pub mod doctor;
pub mod exec;
pub mod executor;
pub mod git;
pub mod hints;
pub mod homebrew;
pub mod hooks;
pub mod log_clean;
pub mod logging;
pub mod output;
pub mod prompt;
pub mod shortcuts;
pub mod store;
pub mod styles;
pub mod suggest;
pub mod trust_prune;
pub mod update_check;
pub mod utils;

// Re-exported from core
pub use self::core::config;
pub use self::core::global_config;
pub use self::core::layout;
pub use self::core::multi_remote;
pub use self::core::remote;
pub use self::core::repo::{
    check_dependencies, extract_repo_name, get_current_branch, get_current_worktree_path,
    get_git_common_dir, get_project_root, is_git_repository, resolve_initial_branch,
};
pub use self::core::settings;
pub use self::core::settings::{DaftSettings, PruneCdTarget};
pub use self::core::worktree::WorktreeConfig;

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::PathBuf;

    #[test]
    fn label_for_argv0_direct_invocations() {
        assert_eq!(label_for_argv0("daft"), "daft");
        assert_eq!(label_for_argv0("/usr/local/bin/daft"), "daft");
        assert_eq!(label_for_argv0("daft-go"), "daft");
        assert_eq!(label_for_argv0("daft-start"), "daft");
        assert_eq!(label_for_argv0("./target/debug/daft"), "daft");
    }

    #[test]
    fn label_for_argv0_git_style_and_fallbacks() {
        assert_eq!(label_for_argv0("git-daft"), "git daft");
        assert_eq!(label_for_argv0("/usr/lib/git-core/git-daft"), "git daft");
        assert_eq!(label_for_argv0("git-worktree-checkout"), "git daft");
        assert_eq!(label_for_argv0("git-worktree-clone"), "git daft");
        // Shortcut symlinks and unknown names render the canonical form.
        assert_eq!(label_for_argv0("gwtco"), "git daft");
        assert_eq!(label_for_argv0(""), "git daft");
    }

    #[test]
    fn cli_label_is_canonical_in_tests() {
        // Pins the cfg!(test) gate: in-process unit tests must always see the
        // canonical label regardless of the test binary's name or any argv a
        // sibling test installed.
        assert_eq!(cli_label(), "git daft");
        assert_eq!(daft_cmd("hooks trust"), "git daft hooks trust");
    }

    #[test]
    #[serial]
    fn test_daft_config_dir_default() {
        unsafe {
            env::remove_var(CONFIG_DIR_ENV);
        }
        let dir = daft_config_dir().unwrap();
        assert!(dir.ends_with("daft"));
    }

    #[test]
    #[serial]
    fn test_daft_config_dir_override() {
        unsafe {
            env::set_var(CONFIG_DIR_ENV, "/tmp/test-daft-config");
        }
        let dir = daft_config_dir().unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/test-daft-config"));
        unsafe {
            env::remove_var(CONFIG_DIR_ENV);
        }
    }

    #[test]
    #[serial]
    fn test_daft_config_dir_override_no_suffix() {
        unsafe {
            env::set_var(CONFIG_DIR_ENV, "/tmp/my-custom-dir");
        }
        let dir = daft_config_dir().unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/my-custom-dir"));
        assert!(!dir.ends_with("daft"));
        unsafe {
            env::remove_var(CONFIG_DIR_ENV);
        }
    }

    #[test]
    #[serial]
    fn test_daft_config_dir_empty_falls_back() {
        unsafe {
            env::set_var(CONFIG_DIR_ENV, "");
        }
        let dir = daft_config_dir().unwrap();
        assert!(dir.ends_with("daft"));
        unsafe {
            env::remove_var(CONFIG_DIR_ENV);
        }
    }

    #[test]
    #[serial]
    fn test_daft_config_dir_rejects_relative_path() {
        unsafe {
            env::set_var(CONFIG_DIR_ENV, "relative/path");
        }
        let result = daft_config_dir();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must be an absolute path")
        );
        unsafe {
            env::remove_var(CONFIG_DIR_ENV);
        }
    }

    #[test]
    #[serial]
    fn test_daft_data_dir_default() {
        unsafe {
            env::remove_var(DATA_DIR_ENV);
        }
        let dir = daft_data_dir().unwrap();
        assert!(dir.ends_with("daft"));
    }

    #[test]
    #[serial]
    fn test_daft_data_dir_override() {
        unsafe {
            env::set_var(DATA_DIR_ENV, "/tmp/test-daft-data");
        }
        let dir = daft_data_dir().unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/test-daft-data"));
        unsafe {
            env::remove_var(DATA_DIR_ENV);
        }
    }

    #[test]
    #[serial]
    fn test_daft_data_dir_override_no_suffix() {
        unsafe {
            env::set_var(DATA_DIR_ENV, "/tmp/my-custom-data");
        }
        let dir = daft_data_dir().unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/my-custom-data"));
        assert!(!dir.ends_with("daft"));
        unsafe {
            env::remove_var(DATA_DIR_ENV);
        }
    }

    #[test]
    #[serial]
    fn test_daft_data_dir_empty_falls_back() {
        unsafe {
            env::set_var(DATA_DIR_ENV, "");
        }
        let dir = daft_data_dir().unwrap();
        assert!(dir.ends_with("daft"));
        unsafe {
            env::remove_var(DATA_DIR_ENV);
        }
    }

    #[test]
    #[serial]
    fn test_daft_data_dir_rejects_relative_path() {
        unsafe {
            env::set_var(DATA_DIR_ENV, "relative/path");
        }
        let result = daft_data_dir();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must be an absolute path")
        );
        unsafe {
            env::remove_var(DATA_DIR_ENV);
        }
    }

    #[test]
    #[serial]
    fn test_daft_state_dir_default() {
        unsafe {
            env::remove_var("DAFT_STATE_DIR");
        }
        let dir = daft_state_dir().unwrap();
        assert!(dir.ends_with("daft"));
    }

    #[test]
    #[serial]
    fn test_daft_state_dir_override() {
        unsafe {
            env::set_var("DAFT_STATE_DIR", "/tmp/test-daft-state");
        }
        let dir = daft_state_dir().unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/test-daft-state"));
        unsafe {
            env::remove_var("DAFT_STATE_DIR");
        }
    }

    #[test]
    #[serial]
    fn test_daft_state_dir_rejects_relative_path() {
        unsafe {
            env::set_var("DAFT_STATE_DIR", "relative/path");
        }
        let result = daft_state_dir();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must be an absolute path")
        );
        unsafe {
            env::remove_var("DAFT_STATE_DIR");
        }
    }

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_skip_startup_tasks_empty_args() {
        assert!(!skip_startup_tasks_for(&[]));
    }

    #[test]
    fn test_skip_startup_tasks_no_subcommand() {
        assert!(!skip_startup_tasks_for(&args(&["daft"])));
    }

    #[test]
    fn test_skip_startup_tasks_shell_init_via_daft() {
        assert!(skip_startup_tasks_for(&args(&[
            "daft",
            "shell-init",
            "bash"
        ])));
    }

    #[test]
    fn test_skip_startup_tasks_shell_init_via_git_daft() {
        assert!(skip_startup_tasks_for(&args(&[
            "git-daft",
            "shell-init",
            "zsh"
        ])));
    }

    #[test]
    fn test_skip_startup_tasks_completions() {
        assert!(skip_startup_tasks_for(&args(&[
            "daft",
            "completions",
            "zsh"
        ])));
    }

    #[test]
    fn test_skip_startup_tasks_completions_via_git_daft() {
        assert!(skip_startup_tasks_for(&args(&[
            "git-daft",
            "completions",
            "bash"
        ])));
    }

    #[test]
    fn test_skip_startup_tasks_check_update() {
        assert!(skip_startup_tasks_for(&args(&["daft", "__check-update"])));
    }

    #[test]
    fn test_skip_startup_tasks_prune_trust() {
        assert!(skip_startup_tasks_for(&args(&["daft", "__prune-trust"])));
    }

    #[test]
    fn test_skip_startup_tasks_regular_command_clone() {
        assert!(!skip_startup_tasks_for(&args(&[
            "daft",
            "clone",
            "https://example.invalid/repo.git"
        ])));
    }

    #[test]
    fn test_skip_startup_tasks_regular_command_list() {
        assert!(!skip_startup_tasks_for(&args(&["daft", "list"])));
    }

    #[test]
    fn test_skip_startup_tasks_does_not_match_substring() {
        // A subcommand that merely contains "shell-init" should not match.
        assert!(!skip_startup_tasks_for(&args(&[
            "daft",
            "shell-init-something"
        ])));
    }

    #[test]
    fn test_should_skip_background_all_false_is_false() {
        assert!(!should_skip_background_tasks_impl(false, false, false));
    }

    #[test]
    fn test_should_skip_background_argv_gate_is_sufficient() {
        // skip_startup_tasks_for already returned true (e.g. `shell-init`).
        assert!(should_skip_background_tasks_impl(true, false, false));
    }

    #[test]
    fn test_should_skip_background_coordinator_is_sufficient() {
        // DAFT_IS_COORDINATOR alone is sufficient — coordinator children must
        // not recursively spawn more background work.
        assert!(should_skip_background_tasks_impl(false, true, false));
    }

    #[test]
    fn test_should_skip_background_test_mode_is_sufficient() {
        // DAFT_TESTING alone is sufficient — the YAML test runner sets this on
        // every step to suppress orphaned background spawns.
        assert!(should_skip_background_tasks_impl(false, false, true));
    }

    #[test]
    fn test_should_skip_background_all_true_is_true() {
        assert!(should_skip_background_tasks_impl(true, true, true));
    }
}
