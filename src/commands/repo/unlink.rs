//! `daft repo unlink` — remove a relation from the current repo's manifest.
//!
//! The inverse of `daft repo link`. Matches the target against the current
//! worktree's `relations:` entries — first by friendly label, then by resolved
//! remote URL — and removes the edge, preserving the rest of daft.yml.

use anyhow::{Result, bail};
use clap::Parser;

use super::relation_io;
use crate::catalog::Catalog;
use crate::catalog::normalize::normalize_url;
use crate::catalog::relations_edit;
use crate::output::{CliOutput, Output, OutputConfig};

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-unlink")]
#[command(version = crate::VERSION)]
#[command(about = "Remove a relation from this repo")]
#[command(long_about = r#"
Removes a directed relation declared in the current worktree's daft.yml. The
target is matched against existing entries first by friendly label, then by
resolving it (catalog name, repo path, or remote URL) to a remote URL and
matching on that — so `unlink` accepts the same forms as `link`, plus the label
shown by `git daft repo info`.

Unlinking an edge that isn't there is a friendly no-op, not an error. Only the
`relations:` block is touched; the rest of daft.yml is left intact. Commit the
result to share the change with your team.
"#)]
pub struct Args {
    #[arg(
        value_name = "TARGET",
        help = "Relation label, catalog repo name, repo path, or remote URL to unlink"
    )]
    target: String,
}

pub fn run() -> Result<()> {
    let raw_args: Vec<String> = crate::cli::argv().to_vec();
    debug_assert!(
        raw_args.len() >= 3 && raw_args[1] == "repo" && raw_args[2] == "unlink",
        "repo::unlink::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft repo unlink ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-repo-unlink".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    let args = Args::parse_from(argv);
    let mut output = CliOutput::new(OutputConfig::default());

    let target = relation_io::locate_manifest()?;
    let current = relations_edit::parse_relations(&target.text)?;
    if current.is_empty() {
        output.info("this repo declares no relations — nothing to unlink");
        return Ok(());
    }

    // Single read-only catalog snapshot, shared by URL-form target resolution.
    let catalog = Catalog::open_ro().ok().flatten();
    let Some(index) = find_relation(catalog.as_ref(), &current, &args.target)? else {
        output.info(&format!(
            "no relation to '{}' — nothing to unlink",
            args.target
        ));
        return Ok(());
    };

    let removed = current[index].clone();
    let key = normalize_url(&removed.url);
    let new_text = relations_edit::remove_relation(&target.text, index)?;

    relation_io::write_manifest(&target, &new_text)?;
    output.result(&format!("Unlinked: {}", removed.label()));
    relation_io::post_write_hint(&mut output, &target.root, &key, false);
    Ok(())
}

/// Find the entry to remove: an exact label match first (erroring if a label
/// is ambiguous), else a match on the resolved remote URL. `Ok(None)` when
/// nothing matches — the caller reports a friendly no-op.
fn find_relation(
    catalog: Option<&Catalog>,
    current: &[crate::catalog::relations::RelationEntry],
    needle: &str,
) -> Result<Option<usize>> {
    let by_label: Vec<usize> = current
        .iter()
        .enumerate()
        .filter(|(_, e)| e.label() == needle)
        .map(|(i, _)| i)
        .collect();
    match by_label.as_slice() {
        [only] => return Ok(Some(*only)),
        [] => {}
        many => {
            let urls = many
                .iter()
                .map(|&i| format!("  {}", current[i].url))
                .collect::<Vec<_>>()
                .join("\n");
            bail!(
                "'{needle}' matches {} relations by label — unlink by URL instead:\n{urls}",
                many.len()
            );
        }
    }

    // No label match — resolve to a URL and match on the normalized key.
    // A target that doesn't resolve simply isn't linked here.
    let Ok(url) = relation_io::resolve_target_url(catalog, needle) else {
        return Ok(None);
    };
    let key = normalize_url(&url);
    Ok(current.iter().position(|e| normalize_url(&e.url) == key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::relations::RelationEntry;

    fn entry(url: &str, name: Option<&str>) -> RelationEntry {
        RelationEntry {
            url: url.into(),
            name: name.map(String::from),
            kind: None,
        }
    }

    #[test]
    fn matches_a_unique_label() {
        let rels = [
            entry("git@x:o/api.git", Some("client")),
            entry("git@x:o/lib.git", None), // label = "lib" (url tail)
        ];
        assert_eq!(find_relation(None, &rels, "client").unwrap(), Some(0));
        assert_eq!(find_relation(None, &rels, "lib").unwrap(), Some(1));
    }

    #[test]
    fn ambiguous_label_is_an_error_listing_urls() {
        let rels = [
            entry("git@x:o/a.git", Some("dup")),
            entry("git@x:o/b.git", Some("dup")),
        ];
        let err = find_relation(None, &rels, "dup").unwrap_err().to_string();
        assert!(err.contains("matches 2 relations"), "{err}");
        assert!(
            err.contains("git@x:o/a.git") && err.contains("git@x:o/b.git"),
            "{err}"
        );
    }
}
