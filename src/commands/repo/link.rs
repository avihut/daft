//! `daft repo link` — declare a relation from the current repo to another.
//!
//! Writes a well-formed, deduped entry into the current worktree's `daft.yml`
//! `relations:` list, so declaring an edge doesn't mean hand-editing YAML and
//! getting the remote-URL form right. The manifest stays committed and
//! team-shared; edges are directed (this writes only the current repo's side).

use anyhow::{Result, bail};
use clap::Parser;

use super::relation_io;
use crate::catalog::normalize::normalize_url;
use crate::catalog::relations::RelationEntry;
use crate::catalog::relations_edit;
use crate::output::{CliOutput, OutputConfig};

#[derive(Parser, Debug)]
#[command(name = "git-daft-repo-link")]
#[command(version = crate::VERSION)]
#[command(about = "Declare a relation from this repo to another")]
#[command(long_about = r#"
Adds a directed relation from the current repo to a target, writing a
well-formed entry into the current worktree's daft.yml `relations:` list. The
manifest is committed and team-shared; relations power `git daft exec
--related`, `git daft start --with-related`, and the `git daft repo info`
Relations section.

The target is resolved to a remote URL — the portable key relations match on —
in this order: a catalog repo name (or a repo path daft has cataloged), then a
path to a git repo on disk, then a remote URL used as-is. A URL that isn't
cloned yet is fine: the edge resolves as "not cloned" until it is.

Linking is idempotent. Re-linking an existing edge is a no-op; passing --name
or --kind updates that edge in place. Editing only touches the `relations:`
block — comments and formatting elsewhere in daft.yml are preserved. Commit the
result to share the relation with your team.
"#)]
pub struct Args {
    #[arg(
        value_name = "TARGET",
        help = "Catalog repo name, a repo path, or a remote URL to link to"
    )]
    target: String,

    #[arg(
        long,
        value_name = "LABEL",
        help = "Friendly label for the edge (defaults to the URL's last path segment)"
    )]
    name: Option<String>,

    #[arg(
        long,
        value_name = "KIND",
        help = "Free-form relationship kind (e.g. consumer, library)"
    )]
    kind: Option<String>,
}

pub fn run() -> Result<()> {
    let raw_args: Vec<String> = crate::cli::argv().to_vec();
    debug_assert!(
        raw_args.len() >= 3 && raw_args[1] == "repo" && raw_args[2] == "link",
        "repo::link::run() invoked with unexpected argv: {raw_args:?} \
         (expected `daft repo link ...`)"
    );
    let argv: Vec<String> = std::iter::once("git-daft-repo-link".to_string())
        .chain(raw_args.into_iter().skip(3))
        .collect();
    let args = Args::parse_from(argv);
    let mut output = CliOutput::new(OutputConfig::default());

    let target = relation_io::locate_manifest()?;
    let url = relation_io::resolve_target_url(&args.target)?;

    if let Some(own) = relation_io::current_repo_url()
        && normalize_url(&own) == normalize_url(&url)
    {
        bail!("a repo can't be linked to itself ({own})");
    }

    let current = relations_edit::parse_relations(&target.text)?;
    let key = normalize_url(&url);
    let existing = current.iter().position(|e| normalize_url(&e.url) == key);

    let (new_text, entry, verb) = match existing {
        None => {
            let entry = RelationEntry {
                url,
                name: args.name,
                kind: args.kind,
            };
            let text = relations_edit::append_relation(&target.text, &entry)?;
            (text, entry, "Linked")
        }
        Some(index) => {
            if args.name.is_none() && args.kind.is_none() {
                relation_io::report_edge(&mut output, "Already linked", &current[index]);
                return Ok(());
            }
            // Upsert: keep the stored URL form, apply the provided fields.
            let updated = upsert_entry(&current[index], args.name, args.kind);
            let text = relations_edit::update_relation(&target.text, index, &updated)?;
            (text, updated, "Updated")
        }
    };

    if new_text == target.text {
        relation_io::report_edge(&mut output, "Already linked", &entry);
        return Ok(());
    }

    relation_io::write_manifest(&target, &new_text)?;
    relation_io::report_edge(&mut output, verb, &entry);
    relation_io::post_write_hint(&mut output, &target.root, &key, true);
    Ok(())
}

/// Merge `--name`/`--kind` onto an existing edge: a provided field overrides,
/// an omitted one is left as it was. The stored URL is never rewritten.
fn upsert_entry(
    existing: &RelationEntry,
    name: Option<String>,
    kind: Option<String>,
) -> RelationEntry {
    RelationEntry {
        url: existing.url.clone(),
        name: name.or_else(|| existing.name.clone()),
        kind: kind.or_else(|| existing.kind.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str, name: Option<&str>, kind: Option<&str>) -> RelationEntry {
        RelationEntry {
            url: url.into(),
            name: name.map(String::from),
            kind: kind.map(String::from),
        }
    }

    #[test]
    fn upsert_overrides_provided_fields_and_keeps_the_rest() {
        let existing = entry("git@x:o/r.git", Some("client"), Some("consumer"));
        // Only --kind given: name is preserved, url untouched.
        let out = upsert_entry(&existing, None, Some("service".into()));
        assert_eq!(out, entry("git@x:o/r.git", Some("client"), Some("service")));
        // Only --name given: kind is preserved.
        let out = upsert_entry(&existing, Some("api".into()), None);
        assert_eq!(out, entry("git@x:o/r.git", Some("api"), Some("consumer")));
    }
}
