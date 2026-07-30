//! Comment-preserving append of `hooks:` and `tasks:` blocks to a
//! hand-authored `daft.yml`.
//!
//! Same doctrine as [`crate::catalog::relations_edit`], and for the same
//! reason: a serde round-trip reserializes the whole document and drops every
//! comment, which is not an acceptable thing to do to a file someone wrote.
//! So the edit is textual, and the load-bearing guarantee is that the result
//! is re-parsed and compared against the input — an edit that landed in the
//! wrong place, broke an anchor, or mis-quoted a value is rejected before
//! anything reaches disk.
//!
//! Appending only. `daft hooks import` runs against a config that does not
//! yet define the stages being imported (a native stage would have taken
//! precedence and made the import pointless), so there is no merge case to
//! get wrong.
//!
//! Pure text → text; file IO lives in the command layer.

use crate::hooks::yaml_config::{HookDef, YamlConfig};
use std::collections::BTreeMap;

/// Why an import edit could not be performed.
#[derive(Debug)]
pub enum BlockEditError {
    /// The input is not valid YAML — refuse to touch it.
    Parse(serde_yaml::Error),
    /// Valid YAML in a shape daft declines to edit automatically. The message
    /// is user-facing and ends by pointing at a hand edit.
    Unsupported(String),
    /// The edit applied but failed post-edit validation. An internal
    /// invariant breach: nothing is returned to be written.
    Validation(String),
}

impl std::fmt::Display for BlockEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockEditError::Parse(e) => write!(f, "daft.yml is not valid YAML: {e}"),
            BlockEditError::Unsupported(msg) | BlockEditError::Validation(msg) => {
                write!(f, "{msg}")
            }
        }
    }
}

impl std::error::Error for BlockEditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BlockEditError::Parse(e) => Some(e),
            _ => None,
        }
    }
}

/// Entries to append, keyed by name.
pub type Entries = BTreeMap<String, HookDef>;

/// Append `hooks` and `tasks` entries to `text`, preserving everything else.
///
/// Returns the new document. `provenance` is written as a comment above the
/// appended block — an imported gate should say where it came from, because
/// the next person to read it will want the original file to compare against.
pub fn append_blocks(
    text: &str,
    hooks: &Entries,
    tasks: &Entries,
    provenance: &str,
) -> Result<String, BlockEditError> {
    let before: YamlConfig = serde_yaml::from_str(text).map_err(BlockEditError::Parse)?;

    for (name, existing) in [("hooks", &before.hooks), ("tasks", &before.tasks)] {
        let incoming: &Entries = if name == "hooks" { hooks } else { tasks };
        if let Some(clash) = incoming.keys().find(|k| existing.contains_key(*k)) {
            return Err(BlockEditError::Unsupported(format!(
                "daft.yml already defines {name}.{clash}; merge it by hand rather than \
                 letting an import guess which version you meant"
            )));
        }
    }

    // Editing an existing block textually would mean finding it, matching its
    // indentation, and threading around comments inside it. Appending a
    // second `hooks:` key is not valid YAML, so a document that already has
    // one is declined with a snippet instead — a hand merge the user can see
    // beats a clever edit they cannot.
    if !hooks.is_empty() && has_key(text, "hooks") {
        return Err(BlockEditError::Unsupported(unsupported_message(
            "hooks", hooks, provenance,
        )));
    }
    if !tasks.is_empty() && has_key(text, "tasks") {
        return Err(BlockEditError::Unsupported(unsupported_message(
            "tasks", tasks, provenance,
        )));
    }

    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !hooks.is_empty() || !tasks.is_empty() {
        if !out.trim().is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("# {provenance}\n"));
    }
    if !hooks.is_empty() {
        out.push_str(&render_block("hooks", hooks)?);
    }
    if !tasks.is_empty() {
        out.push_str(&render_block("tasks", tasks)?);
    }

    validate(text, &out, hooks, tasks)?;
    Ok(out)
}

/// Serialize a fresh `daft.yml` for a repository that has none.
pub fn fresh_document(
    hooks: &Entries,
    tasks: &Entries,
    provenance: &str,
) -> Result<String, BlockEditError> {
    append_blocks("", hooks, tasks, provenance)
}

/// Render one top-level block.
fn render_block(key: &str, entries: &Entries) -> Result<String, BlockEditError> {
    // Serialized as a map so serde does the quoting, then indented under the
    // block key. Hand-rendering the bodies would mean reimplementing YAML
    // escaping for every field a hook can carry.
    let body = serde_yaml::to_string(entries)
        .map_err(|e| BlockEditError::Validation(format!("could not serialize {key}: {e}")))?;
    let mut out = format!("{key}:\n");
    for line in body.lines() {
        if line.trim().is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("  {line}\n"));
        }
    }
    Ok(out)
}

/// Whether `text` has a top-level `key:` line.
///
/// Deliberately crude — it only decides whether to decline, and the
/// post-edit validation is what makes the *accepted* path safe.
fn has_key(text: &str, key: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_end();
        trimmed == format!("{key}:") || trimmed.starts_with(&format!("{key}: "))
    })
}

/// The decline message, carrying the snippet to paste.
fn unsupported_message(key: &str, entries: &Entries, provenance: &str) -> String {
    let snippet = render_block(key, entries).unwrap_or_default();
    format!(
        "daft.yml already has a `{key}:` block, and daft will not rewrite one it did \
         not author. Paste this under it:\n\n# {provenance}\n{snippet}"
    )
}

/// Re-parse the edit and assert it changed exactly what it claimed to.
///
/// The whole safety model: textual heuristics cannot be trusted (a multi-line
/// scalar elsewhere can contain a line that looks like a top-level key), so
/// nothing is written until the result has been read back and compared.
fn validate(
    before_text: &str,
    after_text: &str,
    hooks: &Entries,
    tasks: &Entries,
) -> Result<(), BlockEditError> {
    let before: YamlConfig = serde_yaml::from_str(before_text).map_err(BlockEditError::Parse)?;
    let after: YamlConfig = serde_yaml::from_str(after_text).map_err(|e| {
        BlockEditError::Validation(format!("the edited daft.yml would not parse: {e}"))
    })?;

    let mut expected = before.clone();
    expected.hooks.extend(hooks.clone());
    expected.tasks.extend(tasks.clone());

    if after != expected {
        return Err(BlockEditError::Validation(
            "the edit changed more than the hooks and tasks blocks; refusing to write. \
             Add the entries to daft.yml by hand."
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::yaml_config::{JobDef, RunCommand};

    fn entry(name: &str, run: &str) -> (String, HookDef) {
        (
            name.to_string(),
            HookDef {
                jobs: Some(vec![JobDef {
                    name: Some("job".into()),
                    run: Some(RunCommand::Simple(run.into())),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        )
    }

    fn hooks(entries: &[(&str, &str)]) -> Entries {
        entries.iter().map(|(n, r)| entry(n, r)).collect()
    }

    #[test]
    fn appending_preserves_every_comment_and_blank_line() {
        // The entire reason this module exists instead of a serde round-trip.
        let original = "\
# The team's daft config.
#
# Worktree layout: keep it contained.
layout: contained

shared:
  # local overrides, never committed
  - .env
";
        let out = append_blocks(
            original,
            &hooks(&[("pre-commit", "lint")]),
            &Entries::new(),
            "from x",
        )
        .unwrap();
        assert!(
            out.starts_with(original),
            "the original must be untouched:\n{out}"
        );
        assert!(out.contains("# The team's daft config."));
        assert!(out.contains("# local overrides, never committed"));
        assert!(out.contains("# from x"));
        assert!(out.contains("hooks:"));
    }

    #[test]
    fn the_appended_block_round_trips_to_the_same_definitions() {
        let incoming = hooks(&[
            ("pre-commit", "eslint {staged_files}"),
            ("pre-push", "npm test"),
        ]);
        let out =
            append_blocks("layout: contained\n", &incoming, &Entries::new(), "from x").unwrap();
        let parsed: YamlConfig = serde_yaml::from_str(&out).unwrap();
        assert_eq!(parsed.hooks.len(), 2);
        assert_eq!(parsed.hooks["pre-commit"], incoming["pre-commit"]);
        assert_eq!(parsed.layout.as_deref(), Some("contained"));
    }

    #[test]
    fn tasks_land_in_their_own_block() {
        let out = append_blocks(
            "layout: contained\n",
            &Entries::new(),
            &hooks(&[("deploy", "make deploy")]),
            "from x",
        )
        .unwrap();
        let parsed: YamlConfig = serde_yaml::from_str(&out).unwrap();
        assert!(parsed.hooks.is_empty());
        assert_eq!(parsed.tasks.len(), 1);
        assert!(parsed.tasks.contains_key("deploy"));
    }

    #[test]
    fn a_fresh_document_is_valid_on_its_own() {
        let out =
            fresh_document(&hooks(&[("pre-commit", "lint")]), &Entries::new(), "from x").unwrap();
        let parsed: YamlConfig = serde_yaml::from_str(&out).unwrap();
        assert!(parsed.hooks.contains_key("pre-commit"));
        assert!(out.starts_with("# from x"), "{out}");
    }

    #[test]
    fn an_existing_hooks_block_is_declined_with_a_snippet_to_paste() {
        // Appending a second `hooks:` key is not valid YAML, and editing the
        // existing one textually means threading around its comments. A hand
        // merge the user can see beats a clever edit they cannot.
        let original = "hooks:\n  worktree-post-create:\n    jobs: []\n";
        let err = append_blocks(
            original,
            &hooks(&[("pre-commit", "lint")]),
            &Entries::new(),
            "from x",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("already has a `hooks:` block"), "{msg}");
        assert!(
            msg.contains("pre-commit"),
            "the snippet must be included:\n{msg}"
        );
    }

    #[test]
    fn a_name_that_already_exists_is_refused_before_anything_else() {
        // Guessing which version was meant is exactly what an import must not
        // do with a gate.
        let original = "hooks:\n  pre-commit:\n    jobs: []\n";
        let err = append_blocks(
            original,
            &hooks(&[("pre-commit", "lint")]),
            &Entries::new(),
            "x",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("already defines hooks.pre-commit"), "{msg}");
    }

    #[test]
    fn invalid_input_yaml_is_refused_rather_than_appended_to() {
        let err = append_blocks(
            "hooks: [[[\n",
            &hooks(&[("pre-commit", "l")]),
            &Entries::new(),
            "x",
        )
        .unwrap_err();
        assert!(matches!(err, BlockEditError::Parse(_)), "{err}");
    }

    #[test]
    fn appending_nothing_leaves_the_document_alone() {
        let original = "# just a comment\nlayout: contained\n";
        let out = append_blocks(original, &Entries::new(), &Entries::new(), "x").unwrap();
        assert_eq!(out, original);
    }

    #[test]
    fn a_document_without_a_trailing_newline_still_produces_valid_yaml() {
        let out = append_blocks(
            "layout: contained",
            &hooks(&[("pre-commit", "l")]),
            &Entries::new(),
            "x",
        )
        .unwrap();
        let parsed: YamlConfig = serde_yaml::from_str(&out).unwrap();
        assert_eq!(parsed.layout.as_deref(), Some("contained"));
        assert!(parsed.hooks.contains_key("pre-commit"));
    }

    #[test]
    fn a_run_command_needing_quotes_survives_serialization() {
        // Hand-rendering the bodies would mean reimplementing YAML escaping;
        // this is the case that would break first.
        let tricky = hooks(&[("pre-commit", "echo 'a: b' && printf \"%s\\n\" x")]);
        let out = append_blocks("", &tricky, &Entries::new(), "x").unwrap();
        let parsed: YamlConfig = serde_yaml::from_str(&out).unwrap();
        assert_eq!(parsed.hooks["pre-commit"], tricky["pre-commit"]);
    }
}
