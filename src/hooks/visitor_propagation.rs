//! Propagation of in-scope untracked daft files between worktrees.
//!
//! "In-scope" files for v1:
//!   - `daft.yml` if currently visitor (untracked) in the source worktree.
//!   - `daft.local.yml` (always treated as untracked overlay).
//!
//! Propagation writes the *resolved* content (source overlaid onto target's
//! existing content) into the target. Source wins on conflicts.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::hooks::yaml_config::YamlConfig;
use crate::hooks::yaml_config_loader::{
    ConfigStatus, classify_main_config, merge_configs, parse_yaml_config_str,
};

/// In-scope filenames for v1 propagation.
pub(crate) const VISITOR_DAFT_YML: &str = "daft.yml";
pub(crate) const VISITOR_DAFT_LOCAL_YML: &str = "daft.local.yml";

/// Result of a single propagation run.
#[derive(Debug, Default)]
pub struct PropagationResult {
    pub files_propagated: Vec<String>,
    pub files_skipped: Vec<String>,
}

/// Propagate in-scope untracked daft files from `source` worktree to `target`
/// worktree. The resolved content (source overlaid on target's existing
/// content) is written to the target.
pub fn propagate(source: &Path, target: &Path) -> Result<PropagationResult> {
    let mut result = PropagationResult::default();

    if matches!(classify_main_config(source), ConfigStatus::Visitor) {
        propagate_one(source, target, VISITOR_DAFT_YML, &mut result)?;
    } else if source.join(VISITOR_DAFT_YML).is_file() {
        result.files_skipped.push(VISITOR_DAFT_YML.to_string());
    }

    if source.join(VISITOR_DAFT_LOCAL_YML).is_file() {
        propagate_one(source, target, VISITOR_DAFT_LOCAL_YML, &mut result)?;
    }

    Ok(result)
}

fn propagate_one(
    source: &Path,
    target: &Path,
    filename: &str,
    result: &mut PropagationResult,
) -> Result<()> {
    let src_path = source.join(filename);
    let tgt_path = target.join(filename);

    if !src_path.is_file() {
        return Ok(());
    }

    let src_str = fs::read_to_string(&src_path)
        .with_context(|| format!("Failed to read source {}", src_path.display()))?;
    let src_cfg = parse_yaml_config_str(&src_str)
        .with_context(|| format!("Failed to parse source {}", src_path.display()))?;

    let base_cfg: YamlConfig = if tgt_path.is_file() {
        let tgt_str = fs::read_to_string(&tgt_path)
            .with_context(|| format!("Failed to read target {}", tgt_path.display()))?;
        parse_yaml_config_str(&tgt_str)
            .with_context(|| format!("Failed to parse target {}", tgt_path.display()))?
    } else {
        Default::default()
    };

    let merged = merge_configs(base_cfg, src_cfg);
    let merged_str = serde_yaml::to_string(&merged)
        .with_context(|| format!("Failed to serialize merged {}", filename))?;

    fs::write(&tgt_path, merged_str)
        .with_context(|| format!("Failed to write target {}", tgt_path.display()))?;

    result.files_propagated.push(filename.to_string());
    Ok(())
}

/// Save target's in-scope daft files, propagate from source, run `action`,
/// and restore the saved content if `action` returns an error.
///
/// Used by `daft merge` (Task 5.2) so that a failed git merge leaves the
/// target worktree's untracked daft files in their pre-merge state.
pub fn propagate_atomic<F>(source: &Path, target: &Path, action: F) -> Result<PropagationResult>
where
    F: FnOnce() -> Result<()>,
{
    // Snapshot the pre-existing content of each in-scope file in the target.
    // `None` means the file didn't exist before.
    let saved: Vec<(std::path::PathBuf, Option<String>)> =
        [VISITOR_DAFT_YML, VISITOR_DAFT_LOCAL_YML]
            .iter()
            .map(|f| {
                let p = target.join(f);
                let content = if p.is_file() {
                    fs::read_to_string(&p).ok()
                } else {
                    None
                };
                (p, content)
            })
            .collect();

    let result = propagate(source, target)?;

    match action() {
        Ok(()) => Ok(result),
        Err(e) => {
            // Restore on failure.
            for (path, original) in &saved {
                match original {
                    Some(content) => {
                        let _ = fs::write(path, content);
                    }
                    None => {
                        // File didn't exist originally — remove it if propagation created it.
                        let _ = fs::remove_file(path);
                    }
                }
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn init_git(dir: &Path) {
        Command::new("git")
            .args(["init"])
            .arg(dir)
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.email", "t@t.com"])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.name", "T"])
            .output()
            .unwrap();
    }

    #[test]
    fn test_propagate_visitor_daft_yml() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(
            src.join("daft.yml"),
            "hooks:\n  post-clone:\n    jobs:\n      - name: a\n        run: echo a\n",
        )
        .unwrap();

        let result = propagate(&src, &tgt).unwrap();
        assert!(result.files_propagated.contains(&"daft.yml".to_string()));
        assert!(tgt.join("daft.yml").is_file());
    }

    #[test]
    fn test_propagate_skips_tracked_daft_yml() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(src.join("daft.yml"), "hooks: {}").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&src)
            .args(["add", "daft.yml"])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&src)
            .args(["commit", "-m", "add"])
            .output()
            .unwrap();

        let result = propagate(&src, &tgt).unwrap();
        assert!(!result.files_propagated.contains(&"daft.yml".to_string()));
        assert!(result.files_skipped.contains(&"daft.yml".to_string()));
    }

    #[test]
    fn test_propagate_daft_local_yml_always() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(
            src.join("daft.local.yml"),
            "hooks:\n  worktree-post-create:\n    jobs:\n      - name: x\n        run: echo x\n",
        )
        .unwrap();

        let result = propagate(&src, &tgt).unwrap();
        assert!(
            result
                .files_propagated
                .contains(&"daft.local.yml".to_string())
        );
        assert!(tgt.join("daft.local.yml").is_file());
    }

    #[test]
    fn test_propagate_merges_with_existing_target() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(
            src.join("daft.yml"),
            "hooks:\n  post-clone:\n    jobs:\n      - name: src\n        run: echo src\n",
        )
        .unwrap();
        fs::write(
            tgt.join("daft.yml"),
            "hooks:\n  worktree-post-create:\n    jobs:\n      - name: tgt\n        run: echo tgt\n",
        )
        .unwrap();

        propagate(&src, &tgt).unwrap();

        let merged = fs::read_to_string(tgt.join("daft.yml")).unwrap();
        assert!(merged.contains("post-clone"));
        assert!(merged.contains("worktree-post-create"));
    }

    #[test]
    fn test_propagate_atomic_restores_on_failure() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(
            src.join("daft.yml"),
            "hooks:\n  post-clone:\n    jobs:\n      - run: echo src\n",
        )
        .unwrap();
        fs::write(
            tgt.join("daft.yml"),
            "hooks:\n  post-clone:\n    jobs:\n      - run: echo tgt-original\n",
        )
        .unwrap();

        let tgt_original = fs::read_to_string(tgt.join("daft.yml")).unwrap();

        // Run an atomic propagation that fails inside the action callback.
        let result = propagate_atomic(&src, &tgt, || anyhow::bail!("simulated merge failure"));

        assert!(result.is_err());

        // Target file should be restored to its original content.
        let tgt_now = fs::read_to_string(tgt.join("daft.yml")).unwrap();
        assert_eq!(
            tgt_now, tgt_original,
            "target file should be restored on failure"
        );
    }

    #[test]
    fn test_propagate_atomic_persists_on_success() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(
            src.join("daft.yml"),
            "hooks:\n  worktree-post-create:\n    jobs:\n      - run: echo src\n",
        )
        .unwrap();
        fs::write(
            tgt.join("daft.yml"),
            "hooks:\n  post-clone:\n    jobs:\n      - run: echo tgt\n",
        )
        .unwrap();

        propagate_atomic(&src, &tgt, || Ok(())).unwrap();

        let merged = fs::read_to_string(tgt.join("daft.yml")).unwrap();
        assert!(merged.contains("worktree-post-create"));
        assert!(merged.contains("post-clone"));
    }

    #[test]
    fn test_propagate_atomic_removes_files_created_by_propagation_on_failure() {
        // When target didn't have a file before propagation, and propagation
        // creates one, but the action then fails — the created file should
        // be removed (returning to "didn't exist" state).
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(
            src.join("daft.local.yml"),
            "hooks:\n  post-clone:\n    jobs:\n      - run: echo src\n",
        )
        .unwrap();
        // Target has no daft.local.yml originally.

        let _ = propagate_atomic(&src, &tgt, || anyhow::bail!("fail"));

        assert!(
            !tgt.join("daft.local.yml").is_file(),
            "file created only by propagation should be removed on rollback"
        );
    }
}
