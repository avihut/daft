//! The relations manifest — committed, team-shared edges between repos.
//!
//! Relations are declared in `daft.yml` under a top-level `relations:` key
//! (parsed into [`RelationEntry`] by the hooks config loader; old daft
//! versions ignore unknown keys):
//!
//! ```yaml
//! relations:
//!   - url: git@github.com:org/api-client.git
//!     name: client        # optional friendly label
//!     kind: consumer      # optional, free-form
//! ```
//!
//! Edges are **directed** (declaring A→B does not imply B→A) and keyed by
//! **remote URL**, so a committed manifest is portable across machines:
//! resolution matches the normalized URL against the local catalog's
//! normalized remotes, landing on wherever that repo is cloned locally —
//! or reporting it as not cloned.

use crate::catalog::normalize::normalize_url;
use crate::store::CatalogRepoRow;
use serde::{Deserialize, Serialize};

/// One edge in the relations manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationEntry {
    /// Remote URL of the related repo — the portable identity and the
    /// resolution key.
    pub url: String,

    /// Optional friendly label, used in output when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Optional free-form relationship kind (`client`, `library`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl RelationEntry {
    /// Display label: the manifest's `name` if given, else the last URL
    /// path component, else the raw URL.
    pub fn label(&self) -> &str {
        if let Some(name) = &self.name {
            return name;
        }
        self.url
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .rsplit(['/', ':'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.url)
    }
}

/// A manifest edge resolved against the local catalog.
#[derive(Debug, Clone)]
pub struct ResolvedRelation {
    pub entry: RelationEntry,
    /// The local clone, when one exists (live catalog entry with a
    /// matching normalized remote URL).
    pub repo: Option<CatalogRepoRow>,
}

/// Resolve manifest entries against live catalog rows by normalized URL.
/// Pure — callers supply the rows (typically `catalog.list(false)`).
pub fn resolve_relations(
    entries: &[RelationEntry],
    live_rows: &[CatalogRepoRow],
) -> Vec<ResolvedRelation> {
    entries
        .iter()
        .map(|entry| {
            let key = normalize_url(&entry.url);
            let repo = live_rows
                .iter()
                .find(|row| row.remote_url_normalized.as_deref() == Some(key.as_str()))
                .cloned();
            ResolvedRelation {
                entry: entry.clone(),
                repo,
            }
        })
        .collect()
}

/// The current repo's manifest entries, from its merged `daft.yml`.
/// `Ok(vec![])` when there is no config or no `relations:` key.
///
/// daft.yml is a per-worktree file, so discovery anchors on the current
/// worktree's root; from a container root (no enclosing worktree) it falls
/// back to the project root.
pub fn current_repo_relations() -> anyhow::Result<Vec<RelationEntry>> {
    let root =
        crate::get_current_worktree_path().or_else(|_| crate::core::repo::get_project_root())?;
    Ok(crate::hooks::yaml_config_loader::load_merged_config(&root)?
        .and_then(|config| config.relations)
        .unwrap_or_default())
}

/// Resolve the current repo's relations against the live catalog.
pub fn current_repo_resolved_relations() -> anyhow::Result<Vec<ResolvedRelation>> {
    let entries = current_repo_relations()?;
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let rows = match crate::catalog::Catalog::open_ro()? {
        Some(catalog) => catalog.list(false)?,
        None => Vec::new(),
    };
    Ok(resolve_relations(&entries, &rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn row(name: &str, url_normalized: &str) -> CatalogRepoRow {
        CatalogRepoRow {
            uuid: format!("u-{name}"),
            name: name.into(),
            path: format!("/w/{name}"),
            git_common_dir: format!("/w/{name}/.git"),
            remote_url: Some(format!("git@example.com:{url_normalized}.git")),
            remote_url_normalized: Some(url_normalized.into()),
            default_branch: Some("main".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            removed_at: None,
        }
    }

    #[test]
    fn resolves_across_url_forms() {
        let rows = vec![row("client", "example.com/org/client")];
        let entries = vec![
            RelationEntry {
                url: "https://example.com/org/client.git".into(),
                name: None,
                kind: Some("consumer".into()),
            },
            RelationEntry {
                url: "git@example.com:org/unknown.git".into(),
                name: Some("mystery".into()),
                kind: None,
            },
        ];
        let resolved = resolve_relations(&entries, &rows);
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].repo.as_ref().unwrap().name, "client");
        assert!(resolved[1].repo.is_none());
    }

    #[test]
    fn label_prefers_name_then_url_tail() {
        let named = RelationEntry {
            url: "git@x.com:org/api.git".into(),
            name: Some("the-api".into()),
            kind: None,
        };
        assert_eq!(named.label(), "the-api");
        let bare = RelationEntry {
            url: "git@x.com:org/api.git".into(),
            name: None,
            kind: None,
        };
        assert_eq!(bare.label(), "api");
        let https = RelationEntry {
            url: "https://x.com/org/client".into(),
            name: None,
            kind: None,
        };
        assert_eq!(https.label(), "client");
    }

    #[test]
    fn yaml_shape_parses() {
        let yaml = r#"
- url: git@github.com:org/api.git
  kind: service
- url: https://github.com/org/client
  name: web-client
"#;
        let entries: Vec<RelationEntry> = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind.as_deref(), Some("service"));
        assert_eq!(entries[1].name.as_deref(), Some("web-client"));
    }
}
