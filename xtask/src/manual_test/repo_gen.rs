//! Git repository generator for the manual test framework.
//!
//! Given a [`RepoSpec`] from a YAML scenario, creates a bare git repository
//! with the requested branches, files, commits, daft config, and hook scripts.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::schema::{BranchSpec, RepoSpec};

/// Build a `git` command pre-configured with test identity and config isolation.
fn git_cmd(work_dir: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(work_dir)
        .env("GIT_AUTHOR_NAME", "Manual Test")
        .env("GIT_AUTHOR_EMAIL", "test@daft.test")
        .env("GIT_COMMITTER_NAME", "Manual Test")
        .env("GIT_COMMITTER_EMAIL", "test@daft.test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd
}

/// Run a git command with the test identity in the given working directory.
fn run_git(work_dir: &Path, args: &[&str]) -> Result<()> {
    let status = git_cmd(work_dir)
        .args(args)
        .status()
        .with_context(|| format!("git {} failed to execute", args.join(" ")))?;
    anyhow::ensure!(
        status.success(),
        "git {} failed with {}",
        args.join(" "),
        status
    );
    Ok(())
}

/// Write files from a branch spec into the working tree, creating parent
/// directories as needed.
fn write_branch_files(clone_dir: &Path, branch: &BranchSpec) -> Result<()> {
    for file in &branch.files {
        let file_path = clone_dir.join(&file.path);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dirs for {}", file.path))?;
        }
        std::fs::write(&file_path, &file.content)
            .with_context(|| format!("writing file {}", file.path))?;
    }
    Ok(())
}

/// Stage all changes and create commits for a branch.
///
/// If there are files but no explicit commits, a single "Initial commit" is
/// created. If there are no files and no commits, nothing happens.
fn commit_branch(clone_dir: &Path, branch: &BranchSpec) -> Result<()> {
    if branch.files.is_empty() && branch.commits.is_empty() {
        return Ok(());
    }

    run_git(clone_dir, &["add", "."])?;

    if branch.commits.is_empty() {
        // Files present but no explicit commits — create one.
        run_git(clone_dir, &["commit", "-m", "Initial commit"])?;
    } else {
        for (i, commit) in branch.commits.iter().enumerate() {
            // Only `git add .` before the first commit; subsequent commits
            // operate on whatever is already staged.
            if i > 0 {
                run_git(clone_dir, &["add", "."])?;
            }
            run_git(clone_dir, &["commit", "-m", &commit.message])?;
        }
    }
    Ok(())
}

/// Generate a bare git repository from a [`RepoSpec`].
///
/// The bare repo is created at `<remotes_dir>/<spec.name>`. A temporary clone
/// is used to populate branches and is removed before returning.
///
/// Returns the path to the bare repository.
pub fn generate_repo(spec: &RepoSpec, remotes_dir: &Path) -> Result<PathBuf> {
    let bare_path = remotes_dir.join(&spec.name);
    let tmp_clone_path = remotes_dir
        .parent()
        .unwrap_or(remotes_dir)
        .join(format!("tmp-clone-{}", spec.name));

    // 1. Create bare repo.
    std::fs::create_dir_all(&bare_path)
        .with_context(|| format!("creating bare repo dir: {}", bare_path.display()))?;
    run_git(&bare_path, &["init", "--bare", "."]).context("initialising bare repo")?;

    // 2. Clone the bare repo into a temp working directory.
    //    We need a parent dir that exists for the clone target.
    if let Some(parent) = tmp_clone_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    run_git(
        remotes_dir,
        &[
            "clone",
            bare_path.to_str().unwrap(),
            tmp_clone_path.to_str().unwrap(),
        ],
    )
    .context("cloning bare repo into temp dir")?;

    // 3. Process default branch.
    let default_branch = &spec.default_branch;
    let default_branch_spec = spec.branches.iter().find(|b| b.name == *default_branch);

    // Ensure we're on the default branch (newly cloned repos may have no
    // commits yet, so we use checkout -b to create the branch).
    run_git(&tmp_clone_path, &["checkout", "-b", default_branch])?;

    if let Some(branch) = default_branch_spec {
        write_branch_files(&tmp_clone_path, branch)?;
        commit_branch(&tmp_clone_path, branch)?;
    } else if spec.branches.is_empty() {
        // No branches defined at all — create a minimal initial commit.
        let readme_path = tmp_clone_path.join("README.md");
        std::fs::write(&readme_path, "").context("writing default README.md")?;
        run_git(&tmp_clone_path, &["add", "."])?;
        run_git(&tmp_clone_path, &["commit", "-m", "Initial commit"])?;
    }

    // 4. Process additional branches.
    for branch in &spec.branches {
        if branch.name == *default_branch {
            continue;
        }

        let base = branch.from.as_deref().unwrap_or(default_branch.as_str());
        run_git(&tmp_clone_path, &["checkout", base])?;
        run_git(&tmp_clone_path, &["checkout", "-b", &branch.name])?;

        write_branch_files(&tmp_clone_path, branch)?;
        commit_branch(&tmp_clone_path, branch)?;
    }

    // 5. daft.yml config.
    if let Some(daft_yml_content) = &spec.daft_yml {
        run_git(&tmp_clone_path, &["checkout", default_branch])?;
        let daft_yml_path = tmp_clone_path.join("daft.yml");
        std::fs::write(&daft_yml_path, daft_yml_content).context("writing daft.yml")?;
        run_git(&tmp_clone_path, &["add", "."])?;
        run_git(&tmp_clone_path, &["commit", "-m", "Add daft.yml config"])?;
    }

    // 6. Hook scripts.
    if !spec.hook_scripts.is_empty() {
        run_git(&tmp_clone_path, &["checkout", default_branch])?;
        let hooks_dir = tmp_clone_path.join(".daft");
        std::fs::create_dir_all(&hooks_dir).context("creating .daft dir")?;

        for script in &spec.hook_scripts {
            let script_path = hooks_dir.join(&script.name);
            std::fs::write(&script_path, &script.content)
                .with_context(|| format!("writing hook script {}", script.name))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o755);
                std::fs::set_permissions(&script_path, perms)
                    .with_context(|| format!("chmod hook script {}", script.name))?;
            }
        }

        run_git(&tmp_clone_path, &["add", "."])?;
        run_git(&tmp_clone_path, &["commit", "-m", "Add hook scripts"])?;
    }

    // 7. Push all branches to the bare repo.
    run_git(&tmp_clone_path, &["push", "origin", "--all"])?;

    // 8. Set HEAD on bare repo to point at the default branch.
    run_git(
        &bare_path,
        &[
            "symbolic-ref",
            "HEAD",
            &format!("refs/heads/{default_branch}"),
        ],
    )?;

    // 9. Cleanup temp clone.
    std::fs::remove_dir_all(&tmp_clone_path)
        .with_context(|| format!("removing temp clone dir: {}", tmp_clone_path.display()))?;

    Ok(bare_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual_test::schema::*;

    #[test]
    fn test_generate_basic_repo() {
        let spec = RepoSpec {
            name: "test-repo".into(),
            default_branch: "main".into(),
            branches: vec![BranchSpec {
                name: "main".into(),
                from: None,
                files: vec![FileSpec {
                    path: "README.md".into(),
                    content: "# Test".into(),
                }],
                commits: vec![CommitSpec {
                    message: "Initial".into(),
                }],
            }],
            daft_yml: None,
            hook_scripts: vec![],
        };
        let dir = tempfile::tempdir().unwrap();
        let repo_path = generate_repo(&spec, dir.path()).unwrap();
        assert!(repo_path.join("HEAD").exists(), "Should be a bare repo");

        let output = std::process::Command::new("git")
            .args(["--git-dir", repo_path.to_str().unwrap(), "branch"])
            .output()
            .unwrap();
        let branches = String::from_utf8_lossy(&output.stdout);
        assert!(branches.contains("main"));
    }

    #[test]
    fn test_generate_repo_with_multiple_branches() {
        let spec = RepoSpec {
            name: "multi-branch".into(),
            default_branch: "main".into(),
            branches: vec![
                BranchSpec {
                    name: "main".into(),
                    from: None,
                    files: vec![FileSpec {
                        path: "README.md".into(),
                        content: "# Main".into(),
                    }],
                    commits: vec![CommitSpec {
                        message: "Initial".into(),
                    }],
                },
                BranchSpec {
                    name: "develop".into(),
                    from: Some("main".into()),
                    files: vec![FileSpec {
                        path: "dev.txt".into(),
                        content: "dev".into(),
                    }],
                    commits: vec![CommitSpec {
                        message: "Add dev file".into(),
                    }],
                },
            ],
            daft_yml: None,
            hook_scripts: vec![],
        };
        let dir = tempfile::tempdir().unwrap();
        let repo_path = generate_repo(&spec, dir.path()).unwrap();

        let output = std::process::Command::new("git")
            .args(["--git-dir", repo_path.to_str().unwrap(), "branch"])
            .output()
            .unwrap();
        let branches = String::from_utf8_lossy(&output.stdout);
        assert!(branches.contains("main"));
        assert!(branches.contains("develop"));
    }

    #[test]
    fn test_generate_repo_with_daft_yml() {
        let spec = RepoSpec {
            name: "hooks-repo".into(),
            default_branch: "main".into(),
            branches: vec![BranchSpec {
                name: "main".into(),
                from: None,
                files: vec![FileSpec {
                    path: "README.md".into(),
                    content: "# Hooks".into(),
                }],
                commits: vec![CommitSpec {
                    message: "Initial".into(),
                }],
            }],
            daft_yml: Some(
                "hooks:\n  post-clone:\n    jobs:\n      - name: test\n        run: echo hi\n"
                    .into(),
            ),
            hook_scripts: vec![],
        };
        let dir = tempfile::tempdir().unwrap();
        let repo_path = generate_repo(&spec, dir.path()).unwrap();

        // Verify daft.yml exists in the repo by checking git show
        let output = std::process::Command::new("git")
            .args([
                "--git-dir",
                repo_path.to_str().unwrap(),
                "show",
                "main:daft.yml",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "daft.yml should exist on main branch"
        );
        let content = String::from_utf8_lossy(&output.stdout);
        assert!(content.contains("post-clone"));
    }
}
