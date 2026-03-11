//! YAML scenario data structures for the manual test framework.
//!
//! Each scenario file describes a test environment (repos, branches, files)
//! and a sequence of steps to execute and verify.

use serde::Deserialize;
use std::collections::HashMap;

/// A complete test scenario parsed from a YAML file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Human-readable name for the scenario.
    pub name: String,

    /// Optional longer description.
    #[serde(default)]
    pub description: Option<String>,

    /// Repositories to create for this scenario.
    #[serde(default)]
    pub repos: Vec<RepoSpec>,

    /// Extra environment variables to set during the test.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Ordered list of steps to execute.
    pub steps: Vec<Step>,
}

/// Specification for a git repository to create.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoSpec {
    /// Directory name for the repo (relative to the test sandbox).
    pub name: String,

    /// Name of the default branch.
    #[serde(default = "default_branch_name")]
    pub default_branch: String,

    /// Branches to create.
    #[serde(default)]
    pub branches: Vec<BranchSpec>,

    /// Optional `daft.yml` content to write into the repo.
    #[serde(default)]
    pub daft_yml: Option<String>,

    /// Hook scripts to install in `.daft/hooks/`.
    #[serde(default)]
    pub hook_scripts: Vec<HookScriptSpec>,
}

fn default_branch_name() -> String {
    "main".to_string()
}

/// Specification for a branch within a repo.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchSpec {
    /// Branch name.
    pub name: String,

    /// Optional base branch to create from (default: current HEAD).
    #[serde(default)]
    pub from: Option<String>,

    /// Files to create/overwrite on this branch.
    #[serde(default)]
    pub files: Vec<FileSpec>,

    /// Commits to make on this branch.
    #[serde(default)]
    pub commits: Vec<CommitSpec>,
}

/// A file to create in a repo.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSpec {
    /// Path relative to the repo root.
    pub path: String,

    /// File content.
    pub content: String,
}

/// A commit to make on a branch.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitSpec {
    /// Commit message.
    pub message: String,
}

/// A hook script to install in `.daft/hooks/`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookScriptSpec {
    /// Script filename (e.g., `worktree-post-create`).
    pub name: String,

    /// Script content.
    pub content: String,
}

/// A single step in a test scenario.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Human-readable step name.
    pub name: String,

    /// Shell command to execute.
    pub run: String,

    /// Working directory (relative to the sandbox root).
    #[serde(default)]
    pub cwd: Option<String>,

    /// Optional expectations to verify after the command completes.
    #[serde(default)]
    pub expect: Option<Expectations>,
}

/// Expectations to verify after a step completes.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Expectations {
    /// Expected exit code (default: not checked).
    pub exit_code: Option<i32>,

    /// Directories that must exist after the step.
    pub dirs_exist: Vec<String>,

    /// Files that must exist after the step.
    pub files_exist: Vec<String>,

    /// Files that must NOT exist after the step.
    pub files_not_exist: Vec<String>,

    /// Files that must contain specific content.
    pub file_contains: Vec<FileContains>,

    /// Directories that must be valid git worktrees.
    pub is_git_worktree: Vec<WorktreeCheck>,

    /// Branches that must exist in a repo.
    pub branch_exists: Vec<BranchCheck>,
}

/// Assert that a file contains specific content.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileContains {
    /// Path to the file (relative to the sandbox root).
    pub path: String,

    /// String that must appear in the file.
    pub content: String,
}

/// Assert that a directory is a valid git worktree on a given branch.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeCheck {
    /// Directory to check (relative to the sandbox root).
    pub dir: String,

    /// Expected branch name.
    pub branch: String,
}

/// Assert that a branch exists in a repo.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchCheck {
    /// Repo directory (relative to the sandbox root).
    pub repo: String,

    /// Branch name that must exist.
    pub branch: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_scenario() {
        let yaml = r#"
name: minimal
steps:
  - name: do something
    run: echo hello
"#;
        let scenario: Scenario = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(scenario.name, "minimal");
        assert!(scenario.description.is_none());
        assert!(scenario.repos.is_empty());
        assert!(scenario.env.is_empty());
        assert_eq!(scenario.steps.len(), 1);
        assert_eq!(scenario.steps[0].name, "do something");
        assert_eq!(scenario.steps[0].run, "echo hello");
        assert!(scenario.steps[0].cwd.is_none());
        assert!(scenario.steps[0].expect.is_none());
    }

    #[test]
    fn test_full_scenario() {
        let yaml = r##"
name: full scenario
description: A comprehensive test
repos:
  - name: origin
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Hello"
        commits:
          - message: "initial commit"
      - name: feature/foo
        from: main
        files:
          - path: foo.txt
            content: "foo"
        commits:
          - message: "add foo"
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: setup
              run: echo setup
    hook_scripts:
      - name: worktree-post-create
        content: "#!/bin/sh\necho post-create"
env:
  DAFT_TESTING: "1"
  MY_VAR: hello
steps:
  - name: clone the repo
    run: git worktree-clone origin clone-target
    expect:
      exit_code: 0
      dirs_exist:
        - clone-target
      files_exist:
        - clone-target/README.md
      files_not_exist:
        - clone-target/nonexistent
      file_contains:
        - path: clone-target/README.md
          content: "# Hello"
      is_git_worktree:
        - dir: clone-target
          branch: main
      branch_exists:
        - repo: clone-target
          branch: main
  - name: check something
    run: ls -la
    cwd: clone-target
"##;
        let scenario: Scenario = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(scenario.name, "full scenario");
        assert_eq!(
            scenario.description.as_deref(),
            Some("A comprehensive test")
        );
        assert_eq!(scenario.repos.len(), 1);

        let repo = &scenario.repos[0];
        assert_eq!(repo.name, "origin");
        assert_eq!(repo.default_branch, "main");
        assert_eq!(repo.branches.len(), 2);
        assert!(repo.daft_yml.is_some());
        assert_eq!(repo.hook_scripts.len(), 1);
        assert_eq!(repo.hook_scripts[0].name, "worktree-post-create");

        let main_branch = &repo.branches[0];
        assert_eq!(main_branch.name, "main");
        assert!(main_branch.from.is_none());
        assert_eq!(main_branch.files.len(), 1);
        assert_eq!(main_branch.files[0].path, "README.md");
        assert_eq!(main_branch.commits.len(), 1);

        let feature_branch = &repo.branches[1];
        assert_eq!(feature_branch.name, "feature/foo");
        assert_eq!(feature_branch.from.as_deref(), Some("main"));

        assert_eq!(scenario.env.len(), 2);
        assert_eq!(scenario.env["DAFT_TESTING"], "1");

        assert_eq!(scenario.steps.len(), 2);

        let step1 = &scenario.steps[0];
        let expect = step1.expect.as_ref().unwrap();
        assert_eq!(expect.exit_code, Some(0));
        assert_eq!(expect.dirs_exist.len(), 1);
        assert_eq!(expect.files_exist.len(), 1);
        assert_eq!(expect.files_not_exist.len(), 1);
        assert_eq!(expect.file_contains.len(), 1);
        assert_eq!(expect.is_git_worktree.len(), 1);
        assert_eq!(expect.branch_exists.len(), 1);

        let step2 = &scenario.steps[1];
        assert_eq!(step2.cwd.as_deref(), Some("clone-target"));
        assert!(step2.expect.is_none());
    }

    #[test]
    fn test_expectations_default() {
        let expectations = Expectations::default();
        assert!(expectations.exit_code.is_none());
        assert!(expectations.dirs_exist.is_empty());
        assert!(expectations.files_exist.is_empty());
        assert!(expectations.files_not_exist.is_empty());
        assert!(expectations.file_contains.is_empty());
        assert!(expectations.is_git_worktree.is_empty());
        assert!(expectations.branch_exists.is_empty());
    }
}
