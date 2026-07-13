//! The embedded daft agent skill and its install/freshness logic.
//!
//! `SKILL.md` at the repository root is compiled into the binary, so the skill
//! a binary installs is — by construction — the skill that documents that
//! binary's command surface. The `daft skill` command group
//! (`src/commands/skill/`) and the doctor freshness check
//! (`src/doctor/installation.rs`) both go through this module; neither reads
//! the network or the source tree at runtime.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// The agent skill exactly as shipped in this binary.
pub const SKILL_MD: &str = include_str!("../SKILL.md");

/// Skill folder name. Must match the frontmatter `name:` key — the folder
/// name is what agents resolve skills by, so it is not user-configurable.
pub const SKILL_DIR_NAME: &str = "daft-worktree-workflow";

/// The `daft_version:` stamp of the embedded skill.
///
/// The stamp is regenerated in every release commit (release.toml
/// pre-release-hook), and a unit test below pins it to [`crate::VERSION`], so
/// the `expect` can only fire on a build whose tests never ran.
pub fn embedded_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        parse_frontmatter_version(SKILL_MD)
            .expect("embedded SKILL.md must carry a daft_version frontmatter stamp")
    })
}

/// Extract the `daft_version` key from a SKILL.md's `---`-delimited YAML
/// frontmatter. `None` for missing frontmatter, unparseable YAML, or a
/// frontmatter without the key (pre-stamp copies in the field).
pub fn parse_frontmatter_version(content: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Frontmatter {
        daft_version: Option<String>,
    }
    let rest = content.strip_prefix("---\n")?;
    let end = match rest.find("\n---\n") {
        Some(i) => i,
        None => rest.strip_suffix("\n---").map(str::len)?,
    };
    serde_yaml::from_str::<Frontmatter>(&rest[..end])
        .ok()?
        .daft_version
}

/// Three-way semver-ish comparison. Tolerates a leading `v` and ignores
/// pre-release/build suffixes (`1.20.0-rc.1` compares as `1.20.0`); a missing
/// patch component counts as `0`. `None` when either side does not parse —
/// callers decide what unparseable means for them.
pub fn cmp_versions(a: &str, b: &str) -> Option<Ordering> {
    Some(parse_version(a)?.cmp(&parse_version(b)?))
}

fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let s = s.split(['-', '+']).next().unwrap_or(s);
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = match parts.next() {
        Some(p) => p.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Claude Code's user-global skills root (`~/.claude/skills`). Not a daft
/// state directory — it is a foreign tool's convention, hence plain
/// `home_dir` rather than the XDG resolvers.
pub fn user_skills_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join("skills"))
}

/// Where the skill file lands under a given skills root.
pub fn skill_file_path(skills_root: &Path) -> PathBuf {
    skills_root.join(SKILL_DIR_NAME).join("SKILL.md")
}

/// What [`install_to`] found on disk and did about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// No prior copy — written fresh.
    Installed,
    /// A copy with a different (or missing) version stamp — overwritten.
    /// `from` is the prior stamp; `None` for pre-stamp copies.
    Updated { from: Option<String> },
    /// Same stamp but different bytes (hand-edited copy) — normalized.
    Refreshed,
    /// Byte-identical — nothing written.
    UpToDate,
}

/// Write the embedded skill under `skills_root` (creating
/// `<root>/daft-worktree-workflow/`), classifying what happened. Install
/// doubles as update: an existing copy is always overwritten unless it is
/// already byte-identical.
pub fn install_to(skills_root: &Path) -> Result<(PathBuf, InstallOutcome)> {
    let target = skill_file_path(skills_root);
    let existing = match std::fs::read_to_string(&target) {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(e).with_context(|| format!("could not read {}", target.display()));
        }
    };

    let outcome = classify(existing.as_deref());
    if outcome != InstallOutcome::UpToDate {
        let parent = target
            .parent()
            .expect("skill file path always has a parent");
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
        std::fs::write(&target, SKILL_MD)
            .with_context(|| format!("could not write {}", target.display()))?;
    }
    Ok((target, outcome))
}

fn classify(existing: Option<&str>) -> InstallOutcome {
    let Some(existing) = existing else {
        return InstallOutcome::Installed;
    };
    if existing == SKILL_MD {
        return InstallOutcome::UpToDate;
    }
    match parse_frontmatter_version(existing) {
        Some(stamp) if stamp == embedded_version() => InstallOutcome::Refreshed,
        other => InstallOutcome::Updated { from: other },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // The anti-drift guard: the stamp shipped inside the binary must always
    // equal the crate version. The release pre-release-hook restamps
    // SKILL.md in the same commit that bumps Cargo.toml, so the only way to
    // break this is a manual edit — which this test turns into a loud
    // failure instead of a silently wrong doctor verdict.
    #[test]
    fn embedded_stamp_matches_binary_version() {
        assert_eq!(embedded_version(), crate::VERSION);
    }

    #[test]
    fn embedded_skill_has_expected_name() {
        assert!(SKILL_MD.contains("name: daft-worktree-workflow"));
    }

    // --- parse_frontmatter_version ---

    #[test]
    fn parses_version_from_frontmatter() {
        let content = "---\nname: x\ndaft_version: \"1.19.0\"\n---\n\n# Body\n";
        assert_eq!(
            parse_frontmatter_version(content).as_deref(),
            Some("1.19.0")
        );
    }

    #[test]
    fn missing_key_yields_none() {
        let content = "---\nname: x\ndescription: y\n---\n\n# Body\n";
        assert_eq!(parse_frontmatter_version(content), None);
    }

    #[test]
    fn no_frontmatter_yields_none() {
        assert_eq!(parse_frontmatter_version("# Just a heading\n"), None);
        assert_eq!(parse_frontmatter_version(""), None);
    }

    #[test]
    fn unterminated_frontmatter_yields_none() {
        assert_eq!(parse_frontmatter_version("---\nname: x\n"), None);
    }

    #[test]
    fn malformed_yaml_yields_none() {
        let content = "---\n: : :\n\t- {\n---\n";
        assert_eq!(parse_frontmatter_version(content), None);
    }

    // --- cmp_versions ---

    #[test]
    fn version_comparison_matrix() {
        use std::cmp::Ordering::*;
        assert_eq!(cmp_versions("1.19.0", "1.19.0"), Some(Equal));
        assert_eq!(cmp_versions("1.18.2", "1.19.0"), Some(Less));
        assert_eq!(cmp_versions("1.20.0", "1.19.9"), Some(Greater));
        assert_eq!(cmp_versions("v1.19.0", "1.19.0"), Some(Equal));
        assert_eq!(cmp_versions("1.20.0-rc.1", "1.20.0"), Some(Equal));
        assert_eq!(cmp_versions("1.19", "1.19.0"), Some(Equal));
        assert_eq!(cmp_versions("2.0.0", "1.99.99"), Some(Greater));
        assert_eq!(cmp_versions("not-a-version", "1.19.0"), None);
        assert_eq!(cmp_versions("1.19.0", ""), None);
        assert_eq!(cmp_versions("1.2.3.4", "1.2.3"), None);
    }

    // --- install_to ---

    fn stamped_copy(version: &str) -> String {
        format!(
            "---\nname: daft-worktree-workflow\ndaft_version: \"{version}\"\n---\n\n# Old body\n"
        )
    }

    #[test]
    fn fresh_install_writes_file() {
        let tmp = TempDir::new().unwrap();
        let (target, outcome) = install_to(tmp.path()).unwrap();
        assert_eq!(outcome, InstallOutcome::Installed);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), SKILL_MD);
        assert!(target.ends_with("daft-worktree-workflow/SKILL.md"));
    }

    #[test]
    fn second_install_is_up_to_date_and_idempotent() {
        let tmp = TempDir::new().unwrap();
        install_to(tmp.path()).unwrap();
        let (target, outcome) = install_to(tmp.path()).unwrap();
        assert_eq!(outcome, InstallOutcome::UpToDate);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), SKILL_MD);
    }

    #[test]
    fn older_stamp_is_updated_with_prior_version() {
        let tmp = TempDir::new().unwrap();
        let target = skill_file_path(tmp.path());
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, stamped_copy("1.18.0")).unwrap();

        let (_, outcome) = install_to(tmp.path()).unwrap();
        assert_eq!(
            outcome,
            InstallOutcome::Updated {
                from: Some("1.18.0".into())
            }
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), SKILL_MD);
    }

    #[test]
    fn unstamped_copy_is_updated_with_unknown_prior() {
        let tmp = TempDir::new().unwrap();
        let target = skill_file_path(tmp.path());
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(
            &target,
            "---\nname: daft-worktree-workflow\n---\n\n# Ancient copy\n",
        )
        .unwrap();

        let (_, outcome) = install_to(tmp.path()).unwrap();
        assert_eq!(outcome, InstallOutcome::Updated { from: None });
    }

    #[test]
    fn same_stamp_different_bytes_is_refreshed() {
        let tmp = TempDir::new().unwrap();
        let target = skill_file_path(tmp.path());
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let mut edited = SKILL_MD.to_string();
        edited.push_str("\n<!-- local note -->\n");
        std::fs::write(&target, edited).unwrap();

        let (_, outcome) = install_to(tmp.path()).unwrap();
        assert_eq!(outcome, InstallOutcome::Refreshed);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), SKILL_MD);
    }
}
