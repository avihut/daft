/// Configuration constants for git-worktree-workflow
///
/// This module centralizes all configurable values and magic numbers
/// to improve maintainability and reduce hardcoded values throughout the codebase.
/// Git-related constants
pub mod git {
    /// Default threshold for considering a branch as having "ahead" commits
    /// When checking if a local branch has commits ahead of its remote counterpart,
    /// any positive number of commits is considered "ahead"
    pub const COMMITS_AHEAD_THRESHOLD: u32 = 0;

    /// Fallback commit count when git rev-list operations fail
    pub const DEFAULT_COMMIT_COUNT: u32 = 0;

    /// Default exit code when process status cannot be determined
    pub const DEFAULT_EXIT_CODE: i32 = -1;
}

/// Array indexing constants for Git command parsing
pub mod parsing {
    /// Minimum number of parts expected when splitting Git command output
    pub const MIN_PARTS_COUNT: usize = 2;

    /// Index of the reference path in split Git command output  
    pub const REF_PATH_INDEX: usize = 1;
}

/// Counter initialization values
pub mod counters {
    /// Initial value for branch deletion counter
    pub const INITIAL_BRANCHES_DELETED: u32 = 0;

    /// Initial value for worktree removal counter
    pub const INITIAL_WORKTREES_REMOVED: u32 = 0;

    /// Increment value for successful operations
    pub const OPERATION_INCREMENT: u32 = 1;
}

/// Test-related constants
pub mod test {
    /// Sample environment variable value for testing
    pub const TEST_ENV_VALUE: &str = "export TEST=1";
}
