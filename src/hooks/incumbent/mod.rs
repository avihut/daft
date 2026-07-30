//! Running the hooks config a repository already has.
//!
//! The adoption rung below committing to daft: a repository that already
//! describes its gates in another tool's file can have daft run *that*,
//! unchanged, so the question "would daft work for us" is answered by using
//! it rather than by porting first. Nothing is written to the tracked tree,
//! and `daft hooks uninstall` puts the repository back.
//!
//! Only [`self::lefthook`] knows which tool it is. Everything above this line
//! deals in "the incumbent", so a second reader would be a new submodule and
//! not a second vocabulary.

pub mod lefthook;

use crate::hooks::yaml_config::YamlConfig;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// A hooks config daft did not write, translated into daft's own shapes.
#[derive(Debug, Clone)]
pub struct IncumbentSpec {
    /// The file it came from, for messages that must name what is running.
    pub path: PathBuf,
    /// Which tool's format it is, for the same reason.
    pub tool: &'static str,
    /// The translation.
    pub config: YamlConfig,
    /// Things the file asked for that daft will not do. Reported wherever the
    /// spec is used rather than silently dropped: a gate that runs differently
    /// than its file says is worse than one that refuses to run.
    pub unsupported: Vec<String>,
}

/// Find and read an incumbent config in `root`, if there is one.
///
/// Returns `Err` only when a config was found and could not be read — a
/// repository with no incumbent is `Ok(None)`, which is the overwhelming
/// case and must not look like a failure.
pub fn detect(root: &Path) -> Result<Option<IncumbentSpec>> {
    lefthook::read(root)
}

/// Whether `root` has an incumbent config at all, without parsing it.
///
/// The stat-only question `manages_stage` and `hooks status` ask.
pub fn present(root: &Path) -> bool {
    lefthook::config_path(root).is_some()
}

/// Where a hook definition came from.
///
/// Never a merge of the two. A repository mid-migration would otherwise get a
/// gate assembled from two files, and neither file would describe what runs —
/// which is exactly the confusion the takeover rung exists to avoid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageSource {
    /// daft's own `hooks:` block.
    Native,
    /// A config daft did not write, run as-is.
    Incumbent(PathBuf),
    /// Neither — no stage definitions anywhere.
    None,
}

/// Decide which config a repository's stages come from.
///
/// **Native wins for every stage, not per stage.** Once `daft.yml` names a
/// single git stage, the repository has started saying what it wants in
/// daft's own terms, and the remaining incumbent entries are the ones not
/// migrated yet — running them alongside would make the migration's halfway
/// point behave like neither end of it. `daft hooks import` exists to move
/// them across in one step.
pub fn resolve_stage_source(root: &Path, config: Option<&YamlConfig>) -> StageSource {
    let native_stage = config.is_some_and(|c| {
        c.hooks
            .keys()
            .any(|name| crate::hooks::git_stage::GitStage::from_yaml_name(name).is_some())
    });
    if native_stage {
        return StageSource::Native;
    }
    match lefthook::config_path(root) {
        Some(path) => StageSource::Incumbent(path),
        None => StageSource::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn config_with(names: &[&str]) -> YamlConfig {
        let mut config = YamlConfig::default();
        for name in names {
            config.hooks.insert((*name).to_string(), Default::default());
        }
        config
    }

    #[test]
    fn a_native_git_stage_wins_over_an_incumbent_file() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("lefthook.yml"), "pre-commit:\n  jobs: []\n").unwrap();
        assert_eq!(
            resolve_stage_source(tmp.path(), Some(&config_with(&["pre-commit"]))),
            StageSource::Native
        );
    }

    #[test]
    fn one_native_stage_claims_all_of_them() {
        // Halfway through a migration, the file being migrated *from* still
        // has entries. Running both would make the halfway point behave like
        // neither end of the migration.
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("lefthook.yml"), "pre-push:\n  jobs: []\n").unwrap();
        assert_eq!(
            resolve_stage_source(tmp.path(), Some(&config_with(&["pre-commit"]))),
            StageSource::Native
        );
    }

    #[test]
    fn lifecycle_hooks_alone_do_not_claim_the_git_stages() {
        // A repository using daft for worktrees only has `hooks:` entries
        // already. Those must not stop an incumbent file being taken over —
        // that is precisely the orthogonal-adoption case.
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("lefthook.yml"), "pre-commit:\n  jobs: []\n").unwrap();
        let source = resolve_stage_source(
            tmp.path(),
            Some(&config_with(&["worktree-post-create", "pre-merge"])),
        );
        assert!(matches!(source, StageSource::Incumbent(_)), "{source:?}");
    }

    #[test]
    fn no_config_anywhere_resolves_to_nothing() {
        let tmp = tempdir().unwrap();
        assert_eq!(resolve_stage_source(tmp.path(), None), StageSource::None);
        assert!(!present(tmp.path()));
    }
}
