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
pub const CONFIG_DIR_ENV: &str = "DAFT_CONFIG_DIR";

/// Returns the daft config directory path.
///
/// When `DAFT_CONFIG_DIR` is set to a non-empty value, uses that path directly
/// (no `daft/` suffix appended). Otherwise falls back to `dirs::config_dir()/daft`.
pub fn daft_config_dir() -> anyhow::Result<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Ok(dir) = env::var(CONFIG_DIR_ENV) {
        if !dir.is_empty() {
            let path = PathBuf::from(&dir);
            if path.is_relative() {
                anyhow::bail!("DAFT_CONFIG_DIR must be an absolute path, got: {dir}");
            }
            return Ok(path);
        }
    }
    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;
    Ok(config_dir.join("daft"))
}

/// Daft verb aliases that route through to worktree commands.
const DAFT_VERBS: &[&str] = &[
    "adopt", "carry", "clone", "eject", "go", "init", "list", "prune", "remove", "rename", "start",
    "sync", "update",
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
    let args: Vec<String> = env::args().collect();

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
            new_args.extend(args.into_iter().skip(2));
            return new_args;
        }
    }

    // Default: return args as-is (symlink invocation)
    args
}

pub mod commands;
pub mod core;
pub mod doctor;
pub mod exec;
pub mod git;
pub mod hints;
pub mod hooks;
pub mod logging;
pub mod output;
pub mod shortcuts;
pub mod styles;
pub mod suggest;
pub mod trust_prune;
pub mod update_check;
pub mod utils;

// Re-exported from core
pub use self::core::config;
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
    #[serial]
    fn test_daft_config_dir_default() {
        env::remove_var(CONFIG_DIR_ENV);
        let dir = daft_config_dir().unwrap();
        assert!(dir.ends_with("daft"));
    }

    #[test]
    #[serial]
    fn test_daft_config_dir_override() {
        env::set_var(CONFIG_DIR_ENV, "/tmp/test-daft-config");
        let dir = daft_config_dir().unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/test-daft-config"));
        env::remove_var(CONFIG_DIR_ENV);
    }

    #[test]
    #[serial]
    fn test_daft_config_dir_override_no_suffix() {
        env::set_var(CONFIG_DIR_ENV, "/tmp/my-custom-dir");
        let dir = daft_config_dir().unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/my-custom-dir"));
        assert!(!dir.ends_with("daft"));
        env::remove_var(CONFIG_DIR_ENV);
    }

    #[test]
    #[serial]
    fn test_daft_config_dir_empty_falls_back() {
        env::set_var(CONFIG_DIR_ENV, "");
        let dir = daft_config_dir().unwrap();
        assert!(dir.ends_with("daft"));
        env::remove_var(CONFIG_DIR_ENV);
    }

    #[test]
    #[serial]
    fn test_daft_config_dir_rejects_relative_path() {
        env::set_var(CONFIG_DIR_ENV, "relative/path");
        let result = daft_config_dir();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must be an absolute path"));
        env::remove_var(CONFIG_DIR_ENV);
    }
}
