//! Worktree slug derivation.
//!
//! The slug is the canonical sanitized identity of a worktree — safe to embed
//! in docker compose project names, DB schema names, DNS labels, and temp-dir
//! names, and the identity input for derived env values (`daft env`). The
//! `{worktree_slug}` template variable and the env-value derivation MUST agree
//! byte-for-byte, which is why this lives in one pure module rather than being
//! reimplemented per consumer.

use std::path::Path;

/// Slug for a worktree at `worktree_path` under `project_root`.
///
/// The raw name is the worktree path relative to the project root when the
/// worktree lives under it (so a nested worktree like `feature/new` slugs to
/// `feature-new`), otherwise the final path component. The raw name then goes
/// through [`slugify`].
pub fn worktree_slug_from(worktree_path: &Path, project_root: &Path) -> String {
    let raw = worktree_path
        .strip_prefix(project_root)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            worktree_path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    slugify(&raw)
}

/// Lowercase, collapse non-alphanumeric runs to single `-`, trim `-`, cap at
/// 63 chars (the DNS-label limit). Empty input (or input that reduces to
/// nothing) yields `"worktree"`.
pub fn slugify(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            // Collapse any run of separators/other chars into one dash.
            // Leading separators are suppressed (out is still empty).
            out.push('-');
            prev_dash = true;
        }
    }
    // Cap at the DNS-label length, then trim any trailing dash the cap or the
    // collapse may have left.
    out.truncate(63);
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "worktree".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn slug_from_nested_relative_path() {
        assert_eq!(
            worktree_slug_from(
                &PathBuf::from("/project/feature/new"),
                &PathBuf::from("/project")
            ),
            "feature-new"
        );
    }

    #[test]
    fn slug_outside_root_uses_basename() {
        assert_eq!(
            worktree_slug_from(
                &PathBuf::from("/elsewhere/My-WT"),
                &PathBuf::from("/project")
            ),
            "my-wt"
        );
    }

    #[test]
    fn slug_of_root_itself_uses_basename() {
        // strip_prefix succeeds but yields "" — fall through to file_name.
        assert_eq!(
            worktree_slug_from(&PathBuf::from("/project"), &PathBuf::from("/project")),
            "project"
        );
    }

    #[test]
    fn slugify_cases() {
        assert_eq!(slugify("Feature/New"), "feature-new");
        assert_eq!(slugify("API_Server 2"), "api-server-2");
        assert_eq!(slugify("feat/ABC-123"), "feat-abc-123");
        // Pure-separator and empty inputs fall back.
        assert_eq!(slugify("---"), "worktree");
        assert_eq!(slugify(""), "worktree");
        assert_eq!(slugify("!!!@@@"), "worktree");
        // Unicode reduces to the fallback (no ascii-alphanumerics).
        assert_eq!(slugify("日本語"), "worktree");
        // 63-char DNS-label cap.
        assert_eq!(slugify(&"a".repeat(100)).len(), 63);
        // A trailing dash left by the cap is trimmed.
        let capped = format!("{}-tail", "a".repeat(62));
        assert_eq!(slugify(&capped), "a".repeat(62));
    }
}
