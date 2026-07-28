//! Set and remove scalar values in `daft.yml` without disturbing the rest.
//!
//! `daft.yml` is unlike every other store daft writes. `.git/config`,
//! `~/.gitconfig`, `repos.json`, and the global TOML never appear in
//! `git status`; this file is **tracked**. A round-trip through a YAML
//! serializer would drop every comment and reflow every block, and the user
//! would find that in their diff — or, worse, not find it and commit it.
//!
//! So this edits text, not a document: it finds the one line the value lives
//! on and changes that line. Everything it cannot do that way it refuses,
//! pointing the user at the file. A conservative refusal costs one manual
//! edit; a clever rewrite costs a mangled config nobody notices until the
//! next merge gate does not fire.
//!
//! Two rules make the refusals trustworthy:
//!
//! 1. **Bail on anything exotic** — flow style, quoted keys, anchors, block
//!    scalars, a key that appears twice. These are all legal YAML and all
//!    beyond what line surgery can do safely.
//! 2. **Verify after editing.** The result must still parse as a
//!    [`YamlConfig`], and — the part that matters — it must be *identical to
//!    the original except at the target path*. An editor that drops a sibling
//!    key still produces a file that parses.
//!
//! Modelled on `catalog::relations_edit`, which does the same job for the
//! `relations:` block. That one is relations-shaped throughout, so this is a
//! sibling rather than a reuse.

use anyhow::{Result, bail};

use super::yaml_config::YamlConfig;

/// Paths this can edit: one or two segments, e.g. `rc` or `log.retention`.
///
/// Deeper nesting is where `daft.yml` stops being scalars and starts being
/// structure — `hooks:` jobs, `tasks:` — which have their own commands.
const MAX_DEPTH: usize = 2;

/// Set `path` to `value`, returning the new file contents.
pub fn set_scalar(text: &str, path: &str, value: &str) -> Result<String> {
    let segments = parse_path(path)?;
    let edited = match locate(text, &segments)? {
        Some(line) => replace_value(text, line, segments.last().unwrap(), value),
        None => insert(text, &segments, value)?,
    };
    verify(text, &edited, &segments)?;
    Ok(edited)
}

/// Remove `path`. `Ok(None)` when it was not there.
pub fn unset_scalar(text: &str, path: &str) -> Result<Option<String>> {
    let segments = parse_path(path)?;
    let Some(line) = locate(text, &segments)? else {
        return Ok(None);
    };

    let mut lines: Vec<&str> = text.lines().collect();
    lines.remove(line);
    let edited = rejoin(&lines, text);

    verify(text, &edited, &segments)?;
    Ok(Some(edited))
}

fn parse_path(path: &str) -> Result<Vec<String>> {
    let segments: Vec<String> = path.split('.').map(str::to_string).collect();
    if segments.is_empty() || segments.len() > MAX_DEPTH || segments.iter().any(|s| s.is_empty()) {
        bail!("{path} is not a scalar setting daft can edit — change it in daft.yml directly");
    }
    Ok(segments)
}

/// The index of the line holding `segments`' value, if it is there.
///
/// Returns an error the moment the file uses something line surgery cannot
/// handle, so a bail here means "edit it by hand", not "it is missing".
fn locate(text: &str, segments: &[String]) -> Result<Option<usize>> {
    let lines: Vec<&str> = text.lines().collect();

    match segments {
        [key] => find_key(&lines, key, 0),
        [parent, key] => {
            let Some(parent_line) = find_key(&lines, parent, 0)? else {
                return Ok(None);
            };
            // A parent whose value sits on the same line is flow style
            // (`log: {retention: 7d}`) — legal, and not something a line edit
            // can reach into.
            if !value_of(lines[parent_line], parent).trim().is_empty() {
                bail!(
                    "{parent}: is written inline in daft.yml — change {}.{key} in the file directly",
                    parent
                );
            }
            let indent = indent_of(lines[parent_line]);
            find_child(&lines, parent_line, indent, key)
        }
        _ => unreachable!("parse_path bounds the depth"),
    }
}

/// Find `key` at exactly `indent`, refusing anything the surgery cannot do.
fn find_key(lines: &[&str], key: &str, indent: usize) -> Result<Option<usize>> {
    let mut found = None;
    for (index, line) in lines.iter().enumerate() {
        if !is_key_line(line, key, indent) {
            continue;
        }
        check_editable(line, key)?;
        if found.is_some() {
            bail!("{key}: appears more than once in daft.yml — change it in the file directly");
        }
        found = Some(index);
    }
    Ok(found)
}

/// Find `key` among `parent`'s children.
fn find_child(
    lines: &[&str],
    parent_line: usize,
    parent_indent: usize,
    key: &str,
) -> Result<Option<usize>> {
    let mut found = None;
    for (index, line) in lines.iter().enumerate().skip(parent_line + 1) {
        if line.trim().is_empty() || is_comment(line) {
            continue;
        }
        // The block ends at the first line indented no further than its parent.
        if indent_of(line) <= parent_indent {
            break;
        }
        if !is_key_line(line, key, indent_of(line)) {
            continue;
        }
        check_editable(line, key)?;
        if found.is_some() {
            bail!("{key}: appears more than once in daft.yml — change it in the file directly");
        }
        found = Some(index);
    }
    Ok(found)
}

/// Whether `line` is `key:` at exactly `indent`, unquoted.
fn is_key_line(line: &str, key: &str, indent: usize) -> bool {
    if is_comment(line) || indent_of(line) != indent {
        return false;
    }
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix(key)
        .is_some_and(|rest| rest.starts_with(':'))
}

/// Refuse the shapes a line edit must not touch.
fn check_editable(line: &str, key: &str) -> Result<()> {
    let value = value_of(line, key);
    let value = value.trim();

    if value.starts_with('&') || value.starts_with('*') {
        bail!("{key}: uses a YAML anchor — change it in daft.yml directly");
    }
    if value.starts_with('|') || value.starts_with('>') {
        bail!("{key}: is a block scalar — change it in daft.yml directly");
    }
    if value.starts_with('{') || value.starts_with('[') {
        bail!("{key}: is written inline in daft.yml — change it in the file directly");
    }
    Ok(())
}

/// Everything after `key:` on a line, comment included.
fn value_of<'a>(line: &'a str, key: &str) -> &'a str {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or("")
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// Rewrite one line's value, keeping its indentation and trailing comment.
fn replace_value(text: &str, line_index: usize, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let line = &lines[line_index];
    let indent = " ".repeat(indent_of(line));

    // A comment after the value is the author explaining the setting; it
    // belongs to the key, not to the old value, so it is carried over —
    // separator included, because reflowing someone's alignment is a diff
    // they did not ask for.
    //
    // Only a `#` preceded by whitespace starts a comment in YAML, which is
    // also what keeps a `#` inside the old value from being mistaken for one.
    let raw = value_of(line, key);
    let comment = raw
        .char_indices()
        .find(|(at, ch)| {
            *ch == '#'
                && raw[..*at]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace)
        })
        .map(|(at, _)| raw[raw[..at].trim_end().len()..].to_string())
        .unwrap_or_default();

    lines[line_index] = format!("{indent}{key}: {}{comment}", quote_if_needed(value));
    rejoin_owned(&lines, text)
}

/// Add the key where it does not exist yet.
fn insert(text: &str, segments: &[String], value: &str) -> Result<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let rendered = quote_if_needed(value);

    match segments {
        [key] => lines.push(format!("{key}: {rendered}")),
        [parent, key] => {
            let parent_line = find_key(
                &lines.iter().map(String::as_str).collect::<Vec<_>>(),
                parent,
                0,
            )?;
            match parent_line {
                // Append as the block's last child, matching the indentation
                // its existing children already use.
                Some(at) => {
                    let mut insert_at = at + 1;
                    let mut indent = indent_of(&lines[at]) + 2;
                    while insert_at < lines.len() {
                        let line = &lines[insert_at];
                        if line.trim().is_empty() || is_comment(line) {
                            insert_at += 1;
                            continue;
                        }
                        if indent_of(line) <= indent_of(&lines[at]) {
                            break;
                        }
                        indent = indent_of(line);
                        insert_at += 1;
                    }
                    lines.insert(
                        insert_at,
                        format!("{}{key}: {rendered}", " ".repeat(indent)),
                    );
                }
                None => {
                    lines.push(format!("{parent}:"));
                    lines.push(format!("  {key}: {rendered}"));
                }
            }
        }
        _ => unreachable!("parse_path bounds the depth"),
    }

    Ok(rejoin_owned(&lines, text))
}

/// Quote a value YAML would otherwise read as something else.
fn quote_if_needed(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value.trim() != value
        || value.starts_with(['#', '&', '*', '{', '[', '\'', '"', '|', '>', '%', '@', '`'])
        || value.contains(": ")
        || value.ends_with(':');
    if needs_quotes {
        format!("{:?}", value)
    } else {
        value.to_string()
    }
}

fn rejoin(lines: &[&str], original: &str) -> String {
    let owned: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();
    rejoin_owned(&owned, original)
}

/// Reassemble, keeping the file's trailing newline exactly as it was.
fn rejoin_owned(lines: &[String], original: &str) -> String {
    let mut out = lines.join("\n");
    if original.ends_with('\n') || original.is_empty() {
        out.push('\n');
    }
    out
}

/// The check that makes the whole approach safe.
///
/// The result must parse as a real [`YamlConfig`] — not merely as YAML — and
/// must be identical to the original everywhere except the path that was
/// edited. Comparing whole documents with the target path removed is what
/// catches a surgery that dropped a sibling, reindented a block, or landed
/// the value under the wrong parent, all of which still parse.
fn verify(before: &str, after: &str, segments: &[String]) -> Result<()> {
    if serde_yaml::from_str::<YamlConfig>(after).is_err() {
        bail!("that edit would not have produced a valid daft.yml — nothing was written");
    }

    // An empty file parses as Null, not as an empty document — comparing it
    // against a mapping would report a difference the edit did not make.
    let mut original = as_mapping(serde_yaml::from_str(before).unwrap_or_default());
    let mut edited = as_mapping(serde_yaml::from_str(after).unwrap_or_default());

    remove_path(&mut original, segments);
    remove_path(&mut edited, segments);

    if original != edited {
        bail!(
            "that edit would have changed more of daft.yml than {} — nothing was written",
            segments.join(".")
        );
    }
    Ok(())
}

/// Treat an empty document as an empty mapping.
fn as_mapping(value: serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::Null => serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        other => other,
    }
}

/// Drop `segments` from a parsed document, so the rest can be compared.
fn remove_path(value: &mut serde_yaml::Value, segments: &[String]) {
    let serde_yaml::Value::Mapping(map) = value else {
        return;
    };
    match segments {
        [key] => {
            map.remove(serde_yaml::Value::String(key.clone()));
        }
        [parent, key] => {
            let parent_key = serde_yaml::Value::String(parent.clone());
            if let Some(child) = map.get_mut(&parent_key) {
                remove_path(child, &segments[1..]);
                // A block that is now empty is the same as one that was never
                // there, and only one of the two files will have it.
                if matches!(child, serde_yaml::Value::Mapping(m) if m.is_empty())
                    || matches!(child, serde_yaml::Value::Null)
                {
                    map.remove(&parent_key);
                }
            }
            let _ = key;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_an_existing_top_level_key_keeps_everything_around_it() {
        let before = "\
# how daft runs hooks here
rc: .envrc  # sourced first
source_dir: .daft
";
        let after = set_scalar(before, "rc", ".bashrc").unwrap();
        assert_eq!(
            after,
            "\
# how daft runs hooks here
rc: .bashrc  # sourced first
source_dir: .daft
"
        );
    }

    #[test]
    fn setting_a_nested_key_finds_it_inside_its_block() {
        let before = "\
log:
  # keep a fortnight
  retention: 14d
  max_log_size: 10MB
";
        let after = set_scalar(before, "log.retention", "30d").unwrap();
        assert!(after.contains("  retention: 30d"));
        assert!(after.contains("# keep a fortnight"), "comments survive");
        assert!(after.contains("max_log_size: 10MB"), "siblings survive");
    }

    #[test]
    fn a_missing_top_level_key_is_appended() {
        let before = "source_dir: .daft\n";
        let after = set_scalar(before, "rc", ".envrc").unwrap();
        assert_eq!(after, "source_dir: .daft\nrc: .envrc\n");
    }

    #[test]
    fn a_missing_child_joins_its_existing_block_at_the_right_indent() {
        let before = "\
log:
    retention: 7d
shared:
  - .env
";
        let after = set_scalar(before, "log.max_log_size", "5MB").unwrap();
        assert!(
            after.contains("\n    max_log_size: 5MB\n"),
            "the new child matches its siblings' indent: {after}"
        );
        assert!(after.contains("shared:"), "the next block is untouched");
    }

    #[test]
    fn a_missing_block_is_created() {
        let before = "rc: .envrc\n";
        let after = set_scalar(before, "log.retention", "7d").unwrap();
        assert_eq!(after, "rc: .envrc\nlog:\n  retention: 7d\n");
    }

    #[test]
    fn an_empty_file_can_be_written_to() {
        let after = set_scalar("", "rc", ".envrc").unwrap();
        assert_eq!(after, "rc: .envrc\n");
    }

    #[test]
    fn a_file_without_a_trailing_newline_keeps_not_having_one() {
        let after = set_scalar("rc: .envrc", "rc", ".bashrc").unwrap();
        assert_eq!(after, "rc: .bashrc");
    }

    #[test]
    fn unsetting_removes_the_line_and_nothing_else() {
        let before = "\
rc: .envrc
source_dir: .daft
";
        let after = unset_scalar(before, "rc").unwrap().unwrap();
        assert_eq!(after, "source_dir: .daft\n");
    }

    #[test]
    fn unsetting_a_child_leaves_its_siblings() {
        let before = "\
log:
  retention: 7d
  max_log_size: 10MB
";
        let after = unset_scalar(before, "log.retention").unwrap().unwrap();
        assert_eq!(after, "log:\n  max_log_size: 10MB\n");
    }

    #[test]
    fn unsetting_what_is_not_there_reports_rather_than_rewriting() {
        assert!(
            unset_scalar("rc: .envrc\n", "source_dir")
                .unwrap()
                .is_none()
        );
        assert!(
            unset_scalar("rc: .envrc\n", "log.retention")
                .unwrap()
                .is_none()
        );
    }

    // ── Refusals ─────────────────────────────────────────────────────────

    #[test]
    fn flow_style_is_refused_rather_than_reached_into() {
        let before = "log: {retention: 7d}\n";
        let err = set_scalar(before, "log.retention", "30d")
            .unwrap_err()
            .to_string();
        assert!(err.contains("inline"), "unhelpful: {err}");
        assert!(err.contains("directly"), "the message names the way out");
    }

    #[test]
    fn a_duplicate_key_is_refused_because_the_target_is_ambiguous() {
        let before = "rc: .envrc\nrc: .bashrc\n";
        let err = set_scalar(before, "rc", ".zshrc").unwrap_err().to_string();
        assert!(err.contains("more than once"), "unhelpful: {err}");
    }

    #[test]
    fn anchors_and_block_scalars_are_refused() {
        let err = set_scalar("rc: &base .envrc\n", "rc", ".bashrc")
            .unwrap_err()
            .to_string();
        assert!(err.contains("anchor"), "unhelpful: {err}");

        let err = set_scalar("rc: |\n  .envrc\n", "rc", ".bashrc")
            .unwrap_err()
            .to_string();
        assert!(err.contains("block scalar"), "unhelpful: {err}");
    }

    #[test]
    fn a_path_deeper_than_two_levels_is_refused() {
        let err = set_scalar("a: 1\n", "one.two.three", "x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("daft.yml"), "unhelpful: {err}");
    }

    // ── The verification that makes it safe ──────────────────────────────

    #[test]
    fn a_value_that_would_break_the_document_is_refused() {
        // `merge.ff` only accepts `only`; writing anything else produces a
        // file that parses as YAML but not as a daft config.
        let err = set_scalar("merge:\n  ff: only\n", "merge.ff", "sometimes")
            .unwrap_err()
            .to_string();
        assert!(err.contains("valid daft.yml"), "unhelpful: {err}");
    }

    #[test]
    fn the_result_is_compared_against_the_original_everywhere_else() {
        // Directly exercise the guard: an "edit" that also dropped a sibling
        // has to be caught, because it still parses.
        let before = "rc: .envrc\nsource_dir: .daft\n";
        let sabotaged = "rc: .bashrc\n";
        let err = verify(before, sabotaged, &["rc".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("changed more of daft.yml"), "unhelpful: {err}");

        // And the honest edit passes the same check.
        verify(
            before,
            "rc: .bashrc\nsource_dir: .daft\n",
            &["rc".to_string()],
        )
        .unwrap();
    }

    #[test]
    fn every_edit_round_trips_through_the_real_config_type() {
        let before = "\
# team conventions
rc: .envrc
source_dir: .daft
shared:
  - .env
log:
  retention: 7d
merge:
  ff: only
";
        for (path, value) in [
            ("rc", ".bashrc"),
            ("source_dir", "scripts"),
            ("log.retention", "30d"),
            ("log.max_log_size", "20MB"),
            ("merge.ff", "only"),
        ] {
            let after = set_scalar(before, path, value).unwrap_or_else(|e| panic!("{path}: {e}"));
            let parsed: YamlConfig = serde_yaml::from_str(&after).unwrap();
            // The parts that were not the target survive intact.
            assert_eq!(parsed.shared.as_deref(), Some(&[".env".to_string()][..]));
            assert!(
                after.contains("# team conventions"),
                "{path} lost a comment"
            );
        }
    }

    #[test]
    fn values_that_yaml_would_misread_are_quoted() {
        let after = set_scalar("", "rc", "# not a comment").unwrap();
        let parsed: YamlConfig = serde_yaml::from_str(&after).unwrap();
        assert_eq!(parsed.rc.as_deref(), Some("# not a comment"));

        let after = set_scalar("", "source_dir", "path: with colon").unwrap();
        let parsed: YamlConfig = serde_yaml::from_str(&after).unwrap();
        assert_eq!(parsed.source_dir.as_deref(), Some("path: with colon"));
    }

    #[test]
    fn a_key_whose_name_prefixes_another_is_not_confused_for_it() {
        // `source_dir` and `source_dir_local` both start the same way.
        let before = "source_dir_local: .daft-local\nsource_dir: .daft\n";
        let after = set_scalar(before, "source_dir", "scripts").unwrap();
        assert!(after.contains("source_dir_local: .daft-local"), "{after}");
        assert!(after.contains("source_dir: scripts"), "{after}");
    }
}
