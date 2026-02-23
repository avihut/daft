use std::env;
use std::path::Path;

/// Version string from Cargo.toml.
/// Using CARGO_PKG_VERSION ensures consistency across all build methods
/// (git clone, tarball, Homebrew, etc.)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Environment variable containing the path to a temp file where the shell
/// wrapper expects the cd target to be written.
pub const CD_FILE_ENV: &str = "DAFT_CD_FILE";

/// Daft verb aliases that route through to worktree commands.
const DAFT_VERBS: &[&str] = &[
    "adopt", "carry", "clone", "eject", "go", "init", "prune", "remove", "start", "update",
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
