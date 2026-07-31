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
//! Adding only, never replacing. `daft hooks import` runs against a config
//! that does not yet define the stages being imported (a native stage would
//! have taken precedence and made the import pointless), so a name collision
//! is refused outright rather than merged.
//!
//! Where the entries land depends on the document: a file with no `hooks:`
//! key gets one appended, and a file that already has one gets them inserted
//! inside it at its own indentation. The second case is the common one —
//! anyone already using daft for worktree hooks has that block.
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

    // A second top-level `hooks:` key is not valid YAML, so when the document
    // already has one the entries go *inside* it, at its own indentation.
    // That is the common case, not an exotic one: anyone already using daft
    // for worktree hooks has a `hooks:` block, and telling them to paste by
    // hand would make the import useless to exactly the people it is for.
    // Shapes the insertion cannot read (a flow mapping, an inline scalar) are
    // still declined with a snippet.
    let mut out = text.to_string();
    let mut inserted_any = false;
    for (key, entries) in [("hooks", hooks), ("tasks", tasks)] {
        if entries.is_empty() || !has_key(&out, key) {
            continue;
        }
        match insert_into_block(&out, key, entries, provenance) {
            Some(edited) => {
                out = edited;
                inserted_any = true;
            }
            None => {
                return Err(BlockEditError::Unsupported(unsupported_message(
                    key, entries, provenance,
                )));
            }
        }
    }
    if inserted_any {
        // Whatever is left goes through the append path below; both halves are
        // validated together at the end.
        let hooks_left: Entries = if has_key(text, "hooks") {
            Entries::new()
        } else {
            hooks.clone()
        };
        let tasks_left: Entries = if has_key(text, "tasks") {
            Entries::new()
        } else {
            tasks.clone()
        };
        if !hooks_left.is_empty() || !tasks_left.is_empty() {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
            out.push_str(&format!("# {provenance}\n"));
            if !hooks_left.is_empty() {
                out.push_str(&render_block("hooks", &hooks_left)?);
            }
            if !tasks_left.is_empty() {
                out.push_str(&render_block("tasks", &tasks_left)?);
            }
        }
        validate(text, &out, hooks, tasks)?;
        return Ok(out);
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
    Ok(format!("{key}:\n{}", render_entries(entries, 2)?))
}

/// Render entries as YAML at `indent` spaces, with no enclosing key.
fn render_entries(entries: &Entries, indent: usize) -> Result<String, BlockEditError> {
    // Serialized as a map so serde does the quoting, then indented. Hand-
    // rendering the bodies would mean reimplementing YAML escaping for every
    // field a hook can carry.
    let body = serde_yaml::to_string(entries)
        .map_err(|e| BlockEditError::Validation(format!("could not serialize entries: {e}")))?;
    let pad = " ".repeat(indent);
    let mut out = String::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("{pad}{line}\n"));
        }
    }
    Ok(out)
}

/// Insert `entries` at the end of the existing top-level `key:` block.
///
/// Returns `None` for a shape this cannot read — an inline value after the
/// colon (`hooks: {…}`), where finding "the end of the block" has no textual
/// answer. The caller declines with a paste-me snippet in that case.
///
/// A wrong insertion point is not a silent corruption risk: [`validate`]
/// re-parses the result and compares it against the input before anything is
/// written.
fn insert_into_block(text: &str, key: &str, entries: &Entries, provenance: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim_end() == format!("{key}:"))?;

    // The block runs until the next line at column zero that is not blank and
    // not a comment. A trailing comment belongs to whatever follows it, so
    // the insert goes after the last *indented* line instead.
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        let is_top_level = !line.starts_with([' ', '\t']) && !line.trim().is_empty();
        if is_top_level {
            end = i;
            break;
        }
    }
    let last_content = (start + 1..end)
        .rev()
        .find(|&i| !lines[i].trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(start + 1);

    // Match the block's own indentation rather than assuming two spaces — a
    // config indented four keeps looking indented four.
    let indent = lines[start + 1..end]
        .iter()
        .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|line| line.len() - line.trim_start().len())
        .filter(|n| *n > 0)
        .unwrap_or(2);

    let body = render_entries(entries, indent).ok()?;
    let pad = " ".repeat(indent);

    let mut out: Vec<String> = lines[..last_content]
        .iter()
        .map(|s| s.to_string())
        .collect();
    out.push(String::new());
    out.push(format!("{pad}# {provenance}"));
    for line in body.lines() {
        out.push(line.to_string());
    }
    out.extend(lines[last_content..].iter().map(|s| s.to_string()));

    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    Some(joined)
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
    fn an_existing_hooks_block_receives_the_entries_inside_it() {
        // The mainline case: a repository already using daft for worktree
        // hooks has a `hooks:` block, and an import that told those users to
        // paste by hand would be useless to exactly the people it is for.
        let original = "# keep me\nhooks:\n  # and me\n  worktree-post-create:\n    jobs: []\n";
        let out = append_blocks(
            original,
            &hooks(&[("pre-commit", "lint")]),
            &Entries::new(),
            "from x",
        )
        .unwrap();

        let parsed: YamlConfig = serde_yaml::from_str(&out).unwrap();
        assert!(parsed.hooks.contains_key("pre-commit"));
        assert!(
            parsed.hooks.contains_key("worktree-post-create"),
            "the entry that was already there must survive"
        );
        assert!(out.contains("# keep me"), "{out}");
        assert!(out.contains("# and me"), "{out}");
        assert!(
            out.contains("  # from x"),
            "provenance sits at the block's indentation:\n{out}"
        );
        assert_eq!(
            out.matches("hooks:").count(),
            1,
            "a second top-level hooks: key would not be valid YAML:\n{out}"
        );
    }

    #[test]
    fn an_existing_block_keeps_its_own_indentation() {
        let original = "hooks:\n    worktree-post-create:\n        jobs: []\n";
        let out = append_blocks(
            original,
            &hooks(&[("pre-commit", "lint")]),
            &Entries::new(),
            "x",
        )
        .unwrap();
        assert!(out.contains("\n    pre-commit:"), "{out}");
        let parsed: YamlConfig = serde_yaml::from_str(&out).unwrap();
        assert!(parsed.hooks.contains_key("pre-commit"));
    }

    #[test]
    fn a_flow_mapping_block_is_declined_with_a_snippet_to_paste() {
        // `hooks: {…}` has no textual "end of block" to insert before, so it
        // gets the hand-merge path rather than a guess.
        let original = "hooks: {worktree-post-create: {jobs: []}}\n";
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
    fn a_trailing_comment_stays_attached_to_what_follows_it() {
        let original = "hooks:\n  worktree-post-create:\n    jobs: []\n\n# about tasks\ntasks:\n  t:\n    jobs: []\n";
        let out = append_blocks(
            original,
            &hooks(&[("pre-commit", "lint")]),
            &Entries::new(),
            "x",
        )
        .unwrap();
        let comment = out.find("# about tasks").expect("comment survives");
        let tasks_key = out.find("\ntasks:").expect("tasks survives");
        assert!(
            comment < tasks_key,
            "the comment must stay above tasks:, not be orphaned by the insert:\n{out}"
        );
        assert!(
            out.find("pre-commit:").unwrap() < comment,
            "the insert belongs inside hooks:, above that comment:\n{out}"
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
