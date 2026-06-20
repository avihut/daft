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

use crate::hooks::config_merge::merge_configs;
use crate::hooks::yaml_config_loader::{ConfigStatus, classify_main_config, parse_yaml_config_str};

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

    if tgt_path.is_file() {
        // The target already has this file: a genuine merge is needed (source
        // overlaid on the target's existing config). Only the consolidation
        // paths (`daft merge`) reach this — a freshly created worktree never has
        // the file yet. Re-serializing to canonical YAML is acceptable when two
        // real configs are being combined.
        let src_str = fs::read_to_string(&src_path)
            .with_context(|| format!("Failed to read source {}", src_path.display()))?;
        let src_cfg = parse_yaml_config_str(&src_str)
            .with_context(|| format!("Failed to parse source {}", src_path.display()))?;
        let tgt_str = fs::read_to_string(&tgt_path)
            .with_context(|| format!("Failed to read target {}", tgt_path.display()))?;
        let base_cfg = parse_yaml_config_str(&tgt_str)
            .with_context(|| format!("Failed to parse target {}", tgt_path.display()))?;

        let merged = merge_configs(base_cfg, src_cfg);
        let merged_str = serde_yaml::to_string(&merged)
            .with_context(|| format!("Failed to serialize merged {}", filename))?;

        fs::write(&tgt_path, merged_str)
            .with_context(|| format!("Failed to write target {}", tgt_path.display()))?;
    } else {
        // The target has no such file yet (the checkout case): copy the source
        // verbatim. There is nothing to merge into, so a byte-for-byte copy
        // preserves comments, formatting, and every field. Routing this case
        // through merge_configs + serde serialization is what previously
        // canonicalized the file — stripping comments, emitting `null` for every
        // unset field, and (before the merge_configs fix) silently dropping
        // `shared`/`extends`.
        fs::copy(&src_path, &tgt_path).with_context(|| {
            format!(
                "Failed to copy {} to {}",
                src_path.display(),
                tgt_path.display()
            )
        })?;
    }

    result.files_propagated.push(filename.to_string());
    Ok(())
}

/// Does the source worktree have in-scope untracked daft files whose content
/// differs from the target worktree's corresponding file?
///
/// Returns false if source has no in-scope files (nothing to lose).
/// Returns true if any in-scope file is present in source but absent in
/// target, or if both are present and the content differs byte-for-byte.
///
/// Used by the worktree-removal safety boundary (Task 7.2) to prevent
/// silently losing visitor-config refinements when a worktree is removed
/// before its untracked daft files were propagated to the merge target.
pub fn has_inscope_divergence(source: &Path, target: &Path) -> Result<bool> {
    for filename in [VISITOR_DAFT_YML, VISITOR_DAFT_LOCAL_YML] {
        // For daft.yml, only consider it in-scope if the source classifies as visitor.
        if filename == VISITOR_DAFT_YML
            && !matches!(classify_main_config(source), ConfigStatus::Visitor)
        {
            continue;
        }

        if file_has_divergence(source, target, filename)? {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Per-file divergence: does `source/<filename>` contribute config the
/// target's copy lacks? In provenance terms this is the *subsumption* check:
/// `false` means the source's content is already represented in the target,
/// so removing the source loses nothing — regardless of whether the copy was
/// edited since it was seeded. Callers own the in-scope gating (visitor
/// classification for daft.yml).
pub(crate) fn file_has_divergence(source: &Path, target: &Path, filename: &str) -> Result<bool> {
    let src_path = source.join(filename);
    let tgt_path = target.join(filename);

    if !src_path.is_file() {
        return Ok(false);
    }

    if !tgt_path.is_file() {
        // Source has a whole in-scope file the target lacks — removing the
        // source would lose it.
        return Ok(true);
    }

    let src_str = fs::read_to_string(&src_path)?;
    let tgt_str = fs::read_to_string(&tgt_path)?;

    // Compare semantically, not byte-for-byte. The guard's real question is
    // "would removing `source` lose any config not already in `target`?" —
    // a subset question, not equality. Overlay the source's config onto the
    // target's; if the merge changes nothing, the source contributes nothing
    // new and removal is safe. This is why formatting/comment/field-order
    // differences do not count (e.g. a worktree whose daft.yml was written
    // by an older canonicalizing propagate() is still recognised as
    // non-divergent), while a real added/changed job or setting does.
    //
    // Correctness depends on merge_configs being complete: a merge that
    // silently dropped a field would make a real refinement look like a
    // no-op and false-allow the removal. That is why the merge_configs /
    // merge_hook_defs field-drop fix is a prerequisite for this guard.
    //
    // Falls back to a byte comparison if either file fails to parse — we
    // cannot reason about malformed YAML, so any difference is treated as
    // divergence to stay on the safe side.
    match (
        parse_yaml_config_str(&src_str),
        parse_yaml_config_str(&tgt_str),
    ) {
        (Ok(src_cfg), Ok(tgt_cfg)) => Ok(merge_configs(tgt_cfg.clone(), src_cfg) != tgt_cfg),
        _ => Ok(src_str != tgt_str),
    }
}

/// Save target's in-scope daft files, propagate from source, run `action`,
/// and restore the saved content if `action` returns an error.
///
/// Used by `daft merge` so that a failed git merge (conflict, pre-merge hook
/// abort, dirty target) leaves the target worktree's untracked daft files in
/// their pre-merge state. The files written are the consolidation results
/// resolved by the caller — this helper owns only the snapshot/rollback
/// mechanics.
pub fn write_files_atomic<F>(target: &Path, files: &[(String, String)], action: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    // Snapshot the pre-existing content of each file about to be written.
    // `None` means the file didn't exist before.
    let saved: Vec<(std::path::PathBuf, Option<String>)> = files
        .iter()
        .map(|(filename, _)| {
            let p = target.join(filename);
            let content = if p.is_file() {
                fs::read_to_string(&p).ok()
            } else {
                None
            };
            (p, content)
        })
        .collect();

    for (filename, content) in files {
        let path = target.join(filename);
        fs::write(&path, content).with_context(|| format!("Failed to write {}", path.display()))?;
    }

    match action() {
        Ok(()) => Ok(()),
        Err(e) => {
            // Restore on failure.
            for (path, original) in &saved {
                match original {
                    Some(content) => {
                        if let Err(err) = fs::write(path, content) {
                            crate::log_debug!(
                                "visitor rollback: failed to restore {}: {err}",
                                path.display()
                            );
                        }
                    }
                    None => {
                        // File didn't exist originally — remove the one we wrote.
                        if let Err(err) = fs::remove_file(path) {
                            crate::log_debug!(
                                "visitor rollback: failed to remove {}: {err}",
                                path.display()
                            );
                        }
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
    fn test_write_files_atomic_restores_on_failure() {
        let dir = tempdir().unwrap();
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&tgt).unwrap();

        fs::write(
            tgt.join("daft.yml"),
            "hooks:\n  post-clone:\n    jobs:\n      - run: echo tgt-original\n",
        )
        .unwrap();
        let tgt_original = fs::read_to_string(tgt.join("daft.yml")).unwrap();

        // Run an atomic write whose action fails.
        let files = vec![("daft.yml".to_string(), "hooks: {}\n".to_string())];
        let result = write_files_atomic(&tgt, &files, || anyhow::bail!("simulated merge failure"));

        assert!(result.is_err());

        // Target file should be restored to its original content.
        let tgt_now = fs::read_to_string(tgt.join("daft.yml")).unwrap();
        assert_eq!(
            tgt_now, tgt_original,
            "target file should be restored on failure"
        );
    }

    #[test]
    fn test_write_files_atomic_persists_on_success() {
        let dir = tempdir().unwrap();
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&tgt).unwrap();
        fs::write(tgt.join("daft.yml"), "hooks: {}\n").unwrap();

        let files = vec![(
            "daft.yml".to_string(),
            "hooks:\n  worktree-post-create:\n    jobs:\n      - run: echo merged\n".to_string(),
        )];
        write_files_atomic(&tgt, &files, || Ok(())).unwrap();

        let written = fs::read_to_string(tgt.join("daft.yml")).unwrap();
        assert!(written.contains("worktree-post-create"));
    }

    #[test]
    fn test_write_files_atomic_removes_created_files_on_failure() {
        // When the target didn't have a file before the write and the action
        // then fails, the created file is removed (back to "didn't exist").
        let dir = tempdir().unwrap();
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&tgt).unwrap();
        // Target has no daft.local.yml originally.

        let files = vec![("daft.local.yml".to_string(), "hooks: {}\n".to_string())];
        let _ = write_files_atomic(&tgt, &files, || anyhow::bail!("fail"));

        assert!(
            !tgt.join("daft.local.yml").is_file(),
            "file created only by the atomic write should be removed on rollback"
        );
    }

    #[test]
    fn test_divergence_when_target_missing_source_present() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(src.join("daft.local.yml"), "hooks: {}").unwrap();

        assert!(has_inscope_divergence(&src, &tgt).unwrap());
    }

    #[test]
    fn test_no_divergence_when_both_missing() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        assert!(!has_inscope_divergence(&src, &tgt).unwrap());
    }

    #[test]
    fn test_no_divergence_when_content_matches() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(src.join("daft.local.yml"), "hooks: {}").unwrap();
        fs::write(tgt.join("daft.local.yml"), "hooks: {}").unwrap();

        assert!(!has_inscope_divergence(&src, &tgt).unwrap());
    }

    #[test]
    fn test_divergence_when_content_differs() {
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
        fs::write(
            tgt.join("daft.local.yml"),
            "hooks:\n  post-clone:\n    jobs:\n      - run: echo tgt\n",
        )
        .unwrap();

        assert!(has_inscope_divergence(&src, &tgt).unwrap());
    }

    #[test]
    fn test_propagate_copies_source_verbatim_when_target_absent() {
        // On checkout the new worktree has no daft file yet, so propagation must
        // copy the source byte-for-byte: comments and clean formatting
        // preserved, no `null`-littered canonicalization, no dropped fields.
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        let original = "# my visitor config\nshared: [.env]\n\nhooks:\n  \
                        worktree-post-create:\n    jobs:\n      - name: example\n        \
                        run: echo hi\n";
        fs::write(src.join("daft.yml"), original).unwrap();

        let result = propagate(&src, &tgt).unwrap();
        assert!(result.files_propagated.contains(&"daft.yml".to_string()));

        let copied = fs::read_to_string(tgt.join("daft.yml")).unwrap();
        assert_eq!(copied, original, "propagated file must be a verbatim copy");
        assert!(
            copied.contains("# my visitor config"),
            "comments must be preserved"
        );
        assert!(
            copied.contains("shared: [.env]"),
            "`shared` must not be dropped"
        );
        assert!(!copied.contains("null"), "no null-litter");
    }

    #[test]
    fn test_no_divergence_when_source_config_is_subset_of_target() {
        // Reproduces the field report: the worktree being removed has a daft.yml
        // that an older canonicalizing propagate() rewrote — it lost `shared`
        // and is null-littered — but it adds nothing the merge target lacks.
        // Removal must be allowed (no divergence).
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        // Source (worktree being removed): canonicalized / null-littered form,
        // with `shared` already lost — exactly what was found on disk.
        fs::write(
            src.join("daft.yml"),
            "min_version: null\nshared: null\nhooks:\n  worktree-post-create:\n    \
             jobs:\n    - name: example\n      run: echo hi\n",
        )
        .unwrap();
        // Target (merge target / main): the user's clean config.
        fs::write(
            tgt.join("daft.yml"),
            "shared: [.env]\nhooks:\n  worktree-post-create:\n    jobs:\n      \
             - name: example\n        run: echo hi\n",
        )
        .unwrap();

        assert!(
            !has_inscope_divergence(&src, &tgt).unwrap(),
            "a source whose config is a subset of the target must not block removal"
        );
    }

    #[test]
    fn test_divergence_when_source_adds_named_job() {
        // The source worktree has a real refinement (an extra named job) the
        // target lacks — removing it would lose work, so the guard must block.
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let tgt = dir.path().join("tgt");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&tgt).unwrap();
        init_git(&src);
        init_git(&tgt);

        fs::write(
            tgt.join("daft.local.yml"),
            "hooks:\n  worktree-post-create:\n    jobs:\n      - name: a\n        run: echo a\n",
        )
        .unwrap();
        fs::write(
            src.join("daft.local.yml"),
            "hooks:\n  worktree-post-create:\n    jobs:\n      - name: a\n        \
             run: echo a\n      - name: b\n        run: echo b\n",
        )
        .unwrap();

        assert!(
            has_inscope_divergence(&src, &tgt).unwrap(),
            "a source that adds a named job the target lacks must block removal"
        );
    }
}
