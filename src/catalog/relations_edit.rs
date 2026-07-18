//! Comment-preserving surgical editor for the `relations:` block in a
//! hand-authored `daft.yml`.
//!
//! `daft repo link`/`unlink` must add and remove entries in the committed
//! `daft.yml` without disturbing the rest of the file — comments, key order,
//! blank lines, and formatting all survive. A naive serde round-trip
//! (`from_str` → mutate → `to_string`) reserializes the whole document and
//! drops every comment, so these functions instead edit only the text lines
//! that make up the `relations:` block.
//!
//! ## Safety model
//!
//! Textual heuristics alone are not trustworthy — a legally-quoted multi-line
//! scalar elsewhere in the file can contain a line that looks exactly like a
//! `relations:` key. So the load-bearing guarantee is **post-edit
//! validation** ([`validate`]): every operation re-parses its own output and
//! asserts the resulting [`YamlConfig`] equals the original with only
//! `relations` changed to the expected value. Any edit that landed in the
//! wrong place, broke an anchor, or mis-quoted a value fails this check and is
//! rejected with [`RelationsEditError::Validation`] before anything is
//! written. The textual guards below exist mostly to turn "internal
//! validation failed" into a friendly "edit daft.yml by hand" message for the
//! handful of exotic YAML shapes we decline to touch.
//!
//! Recognised bail cases ([`RelationsEditError::Unsupported`]): flow-style
//! `relations: [...]`, quoted/explicit keys, multiple `relations:` lines,
//! zero-indent list items, an item carrying fields other than
//! `url`/`name`/`kind` (upsert only — it would silently drop them), and an
//! explicit `...` document-end marker when appending a fresh block.
//!
//! All functions are pure text → text; file IO (atomic write, permission
//! preservation) lives in the command layer.

use super::relations::RelationEntry;
use crate::hooks::yaml_config::YamlConfig;

/// Why a relations edit could not be performed.
#[derive(Debug)]
pub enum RelationsEditError {
    /// The input `daft.yml` is not valid YAML — refuse to touch it.
    Parse(serde_yaml::Error),
    /// Valid YAML, but written in a shape daft declines to edit
    /// automatically. The message is user-facing and ends by pointing at a
    /// hand edit.
    Unsupported(&'static str),
    /// The edit was applied but failed post-edit validation. This is an
    /// internal invariant breach: nothing is returned to be written.
    Validation(String),
}

impl std::fmt::Display for RelationsEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelationsEditError::Parse(e) => write!(f, "daft.yml is not valid YAML: {e}"),
            RelationsEditError::Unsupported(msg) => write!(f, "{msg}"),
            RelationsEditError::Validation(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for RelationsEditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RelationsEditError::Parse(e) => Some(e),
            _ => None,
        }
    }
}

/// The `relations:` entries currently declared in `text`.
///
/// `Ok(vec![])` when there is no `relations:` key. Errors only when the file
/// is not valid YAML.
pub fn parse_relations(text: &str) -> Result<Vec<RelationEntry>, RelationsEditError> {
    let cfg: YamlConfig = serde_yaml::from_str(text).map_err(RelationsEditError::Parse)?;
    Ok(cfg.relations.unwrap_or_default())
}

/// Append `entry` to the `relations:` block, creating the block (at end of
/// file) when the document has none. Callers dedup by normalized URL first;
/// this appends unconditionally.
pub fn append_relation(text: &str, entry: &RelationEntry) -> Result<String, RelationsEditError> {
    let original: YamlConfig = serde_yaml::from_str(text).map_err(RelationsEditError::Parse)?;
    let current = original.relations.clone().unwrap_or_default();

    let mut lines = split_lines(text);
    match analyze(&lines, &original.relations)? {
        Some(block) => {
            let indent = block.item_indent.unwrap_or(DEFAULT_INDENT);
            let eol = detect_eol(&lines[block.key_line]);
            let rendered = render_item(entry, indent, eol)?;
            let insert_at = match block.block_end {
                Some(end) => end + 1,
                None => block.key_line + 1,
            };
            ensure_prev_terminated(&mut lines, insert_at, eol);
            lines.insert(insert_at, rendered);
        }
        None => append_new_block(&mut lines, entry)?,
    }

    let mut expected = current;
    expected.push(entry.clone());
    let edited = lines.concat();
    validate(&original, &edited, &expected)?;
    Ok(edited)
}

/// Replace the entry at `index` (relations are ordered; the index matches
/// [`parse_relations`] order) with `entry`. A no-op change returns the input
/// unchanged.
pub fn update_relation(
    text: &str,
    index: usize,
    entry: &RelationEntry,
) -> Result<String, RelationsEditError> {
    let original: YamlConfig = serde_yaml::from_str(text).map_err(RelationsEditError::Parse)?;
    let current = original.relations.clone().unwrap_or_default();
    if index >= current.len() {
        return Err(RelationsEditError::Validation(
            "relation index out of range".into(),
        ));
    }
    if current[index] == *entry {
        return Ok(text.to_string());
    }

    let mut lines = split_lines(text);
    let block =
        analyze(&lines, &original.relations)?.ok_or(RelationsEditError::Unsupported(NO_BLOCK))?;
    let indent = block.item_indent.unwrap_or(DEFAULT_INDENT);
    let dash = block.dashes[index];
    let region_end = field_region_end(&lines, &block, index, indent);

    for line in &lines[dash..region_end] {
        if !is_plain_region_line(line, indent) {
            return Err(RelationsEditError::Unsupported(UNMANAGED_FIELDS));
        }
    }

    let eol = detect_eol(&lines[dash]);
    let rendered = render_item(entry, indent, eol)?;
    lines.splice(dash..region_end, std::iter::once(rendered));

    let mut expected = current;
    expected[index] = entry.clone();
    let edited = lines.concat();
    validate(&original, &edited, &expected)?;
    Ok(edited)
}

/// Remove the entry at `index`. When it is the last remaining entry the whole
/// `relations:` block (including the key line) is removed.
pub fn remove_relation(text: &str, index: usize) -> Result<String, RelationsEditError> {
    let original: YamlConfig = serde_yaml::from_str(text).map_err(RelationsEditError::Parse)?;
    let current = original.relations.clone().unwrap_or_default();
    if index >= current.len() {
        return Err(RelationsEditError::Validation(
            "relation index out of range".into(),
        ));
    }

    let mut lines = split_lines(text);
    let block =
        analyze(&lines, &original.relations)?.ok_or(RelationsEditError::Unsupported(NO_BLOCK))?;

    if current.len() == 1 {
        let end = block.block_end.unwrap_or(block.key_line);
        lines.drain(block.key_line..=end);
    } else {
        let start = block.core_starts[index];
        let end = if index + 1 < block.dashes.len() {
            block.core_starts[index + 1]
        } else {
            block.block_end.expect("multi-item block has content") + 1
        };
        lines.drain(start..end);
    }

    let mut expected = current;
    expected.remove(index);
    let edited = lines.concat();
    validate(&original, &edited, &expected)?;
    Ok(edited)
}

// ── internals ─────────────────────────────────────────────────────────────

const DEFAULT_INDENT: usize = 2;
const NO_BLOCK: &str = "no `relations:` block found in daft.yml to edit";
const UNMANAGED_FIELDS: &str = "this relation entry has fields daft doesn't manage, or unusual formatting — edit daft.yml by hand";
const UNSUPPORTED_KEY: &str = "daft.yml declares `relations:` in a form daft can't safely edit (flow style, or a quoted or non-standard key) — edit the file by hand";
const MULTIPLE_KEYS: &str = "daft.yml has more than one `relations:` line — edit the file by hand";
const UNUSUAL_LIST: &str = "daft.yml's `relations:` list uses formatting daft can't safely edit (flow style or unusual indentation) — edit the file by hand";
const DOC_END_MARKER: &str =
    "daft.yml uses an explicit `...` document-end marker — edit the file by hand to add relations";

/// A located `relations:` block. Indices are into the line vector.
struct Block {
    /// The `relations:` key line.
    key_line: usize,
    /// Leading-space count of the list items, or `None` for an empty block
    /// (bare `relations:` with a null value).
    item_indent: Option<usize>,
    /// One index per item, pointing at the `- ` line, in document order.
    dashes: Vec<usize>,
    /// One index per item, pointing at the first line of the item's span
    /// (its leading comment run, when present, else the `- ` line).
    core_starts: Vec<usize>,
    /// The last content (non-blank, non-comment) line of the block, or `None`
    /// for an empty block. Trailing comments/blanks after it belong to the
    /// next section and are never touched.
    block_end: Option<usize>,
}

/// Split into lines, each retaining its terminator (so `concat()` round-trips
/// byte-for-byte). An empty input yields no lines.
fn split_lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(String::from).collect()
}

fn strip_eol(line: &str) -> &str {
    let l = line.strip_suffix('\n').unwrap_or(line);
    l.strip_suffix('\r').unwrap_or(l)
}

fn detect_eol(line: &str) -> &'static str {
    if line.ends_with("\r\n") { "\r\n" } else { "\n" }
}

fn leading_spaces(line: &str) -> usize {
    strip_eol(line).bytes().take_while(|&b| b == b' ').count()
}

fn is_blank(line: &str) -> bool {
    strip_eol(line).trim().is_empty()
}

fn is_comment_line(line: &str) -> bool {
    strip_eol(line)
        .trim_start_matches([' ', '\t'])
        .starts_with('#')
}

/// A line that starts a new top-level construct (column 0, not a comment,
/// not blank) — the terminator for the block's textual extent. Document
/// markers `---`/`...` count, matching where libyaml ends the mapping.
fn is_column0_content(line: &str) -> bool {
    match strip_eol(line).as_bytes().first() {
        None => false,
        Some(b' ' | b'\t' | b'#') => false,
        Some(_) => true,
    }
}

/// A top-level `relations:` key line: `relations:` at column 0 with only
/// whitespace or a trailing comment after the colon. Flow values
/// (`relations: [..]`), scalars, anchors, and tags all fail this and route to
/// a bail.
fn is_relations_key_line(line: &str) -> bool {
    let Some(rest) = strip_eol(line).strip_prefix("relations:") else {
        return false;
    };
    let rest = rest.trim_start_matches([' ', '\t']);
    rest.is_empty() || rest.starts_with('#')
}

/// If `line` is a block-sequence item (`<spaces>- ` or a bare `<spaces>-`),
/// its leading-space indent.
fn dash_indent(line: &str) -> Option<usize> {
    let body = strip_eol(line);
    let n = leading_spaces(body);
    let rest = &body.as_bytes()[n..];
    if rest.first() != Some(&b'-') {
        return None;
    }
    match rest.get(1) {
        None | Some(b' ' | b'\t') => Some(n),
        _ => None,
    }
}

fn is_managed_field(rest: &str) -> bool {
    let rest = rest.trim_start_matches(' ');
    rest.starts_with("url:") || rest.starts_with("name:") || rest.starts_with("kind:")
}

/// Whether a line within an item's span is one daft manages: a comment, a
/// blank, the `- ` line introducing `url:`/`name:`/`kind:` (or a bare dash),
/// or a deeper `url:`/`name:`/`kind:` field. Anything else (a foreign field,
/// a block scalar) makes an in-place rewrite unsafe.
fn is_plain_region_line(line: &str, indent: usize) -> bool {
    let body = strip_eol(line);
    if body.trim().is_empty() {
        return true;
    }
    let ind = leading_spaces(body);
    let rest = &body[ind..];
    if rest.starts_with('#') {
        return true;
    }
    if ind == indent {
        let Some(after) = rest.strip_prefix('-') else {
            return false;
        };
        let after = after.trim_start_matches(' ');
        after.is_empty() || is_managed_field(after)
    } else if ind > indent {
        is_managed_field(rest)
    } else {
        false
    }
}

/// Walk up from a `- ` line to include a contiguous comment run directly above
/// it (no intervening blank line, never crossing the `relations:` key line).
///
/// Only the second and later items call this; the first item never absorbs a
/// leading run (see [`analyze`]) so a block-header comment isn't dragged out
/// when its first entry is unlinked.
fn leading_comment_start(lines: &[String], dash: usize, key_line: usize) -> usize {
    let mut start = dash;
    while start > key_line + 1 && is_comment_line(&lines[start - 1]) {
        start -= 1;
    }
    start
}

/// Locate the single `relations:` block, or `Ok(None)` when the document has
/// no editable block (and none parsed) — the append-a-new-block case.
fn analyze(
    lines: &[String],
    parsed: &Option<Vec<RelationEntry>>,
) -> Result<Option<Block>, RelationsEditError> {
    // Every column-0 `relations:` line, whatever follows the colon.
    let key_lines: Vec<usize> = (0..lines.len())
        .filter(|&i| strip_eol(&lines[i]).strip_prefix("relations:").is_some())
        .collect();

    // A `relations:` line carrying an inline value (flow list, scalar,
    // anchor, `~`) is not a block we can extend line-by-line.
    if key_lines.iter().any(|&i| !is_relations_key_line(&lines[i])) {
        return Err(RelationsEditError::Unsupported(UNSUPPORTED_KEY));
    }

    let key_line = match key_lines.as_slice() {
        [] => {
            // No textual key. If serde still parsed a `relations:` value, it
            // is written in a form we can't map to lines (quoted or explicit
            // key, e.g. `"relations":` or `relations :`).
            return if parsed.is_some() {
                Err(RelationsEditError::Unsupported(UNSUPPORTED_KEY))
            } else {
                Ok(None)
            };
        }
        [k] => *k,
        _ => return Err(RelationsEditError::Unsupported(MULTIPLE_KEYS)),
    };

    let parsed_len = parsed.as_ref().map_or(0, Vec::len);

    // Textual extent: from just after the key line to the next column-0
    // construct (or end of file).
    let scan_end = ((key_line + 1)..lines.len())
        .find(|&idx| is_column0_content(&lines[idx]))
        .unwrap_or(lines.len());

    let mut item_indent: Option<usize> = None;
    let mut dashes = Vec::new();
    let mut block_end: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate().take(scan_end).skip(key_line + 1) {
        if is_blank(line) || is_comment_line(line) {
            continue;
        }
        block_end = Some(idx);
        if let Some(n) = dash_indent(line) {
            match item_indent {
                None => {
                    item_indent = Some(n);
                    dashes.push(idx);
                }
                Some(existing) if n == existing => dashes.push(idx),
                Some(_) => {} // deeper nested dash: item content, not a list entry
            }
        }
    }

    // The textual item count must match what serde parsed; any mismatch means
    // the list is shaped in a way our line surgery would corrupt.
    if dashes.len() != parsed_len {
        return Err(RelationsEditError::Unsupported(UNUSUAL_LIST));
    }

    // The first item never absorbs a leading comment run: a comment between
    // the `relations:` key and the first item heads the block (or is
    // block-level guidance), so it must survive unlinking that item — the
    // block, and its heading, remain for the other items. Second and later
    // items do absorb the contiguous comment run directly above them, which
    // describes that entry.
    let core_starts = dashes
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            if i == 0 {
                d
            } else {
                leading_comment_start(lines, d, key_line)
            }
        })
        .collect();

    Ok(Some(Block {
        key_line,
        item_indent,
        dashes,
        core_starts,
        block_end,
    }))
}

/// The exclusive end of the item's field region (the `- ` line and its deeper
/// continuation fields), bounded by the next item or the block end.
fn field_region_end(lines: &[String], block: &Block, index: usize, indent: usize) -> usize {
    let dash = block.dashes[index];
    let limit = if index + 1 < block.dashes.len() {
        block.core_starts[index + 1]
    } else {
        block.block_end.map_or(dash + 1, |end| end + 1)
    };
    let mut end = dash + 1;
    while end < limit {
        let line = &lines[end];
        if is_blank(line) || leading_spaces(line) <= indent {
            break;
        }
        end += 1;
    }
    end
}

/// Append a fresh `relations:` block at end of file (the document had none).
fn append_new_block(
    lines: &mut Vec<String>,
    entry: &RelationEntry,
) -> Result<(), RelationsEditError> {
    if lines.iter().any(|l| strip_eol(l) == "...") {
        return Err(RelationsEditError::Unsupported(DOC_END_MARKER));
    }
    let eol = lines.last().map_or("\n", |l| detect_eol(l));
    if let Some(last) = lines.last_mut()
        && !last.ends_with('\n')
    {
        last.push_str(eol);
    }
    // Separate the new block from existing content with one blank line.
    let has_content = lines.iter().any(|l| !is_blank(l));
    if has_content && lines.last().is_some_and(|l| !is_blank(l)) {
        lines.push(eol.to_string());
    }
    lines.push(format!("relations:{eol}"));
    lines.push(render_item(entry, DEFAULT_INDENT, eol)?);
    Ok(())
}

/// Render one entry as YAML sequence-item lines at `indent`, terminated with
/// `eol`. Serialization via serde handles scalar quoting (`name: "true"`,
/// `kind: "a: b"`, …); we only re-indent and re-terminate.
fn render_item(
    entry: &RelationEntry,
    indent: usize,
    eol: &str,
) -> Result<String, RelationsEditError> {
    let fragment =
        serde_yaml::to_string(std::slice::from_ref(entry)).map_err(RelationsEditError::Parse)?;
    let pad = " ".repeat(indent);
    let mut out = String::new();
    for line in fragment.lines() {
        if line == "---" {
            continue;
        }
        out.push_str(&pad);
        out.push_str(line);
        out.push_str(eol);
    }
    Ok(out)
}

/// Ensure the line preceding an insertion point ends with a newline, so the
/// inserted content starts on its own line.
fn ensure_prev_terminated(lines: &mut [String], insert_at: usize, eol: &str) {
    if insert_at == 0 {
        return;
    }
    let prev = &mut lines[insert_at - 1];
    if !prev.ends_with('\n') {
        prev.push_str(eol);
    }
}

/// The load-bearing guard: re-parse the edited text and require it to equal
/// the original config with only `relations` changed to `expected`.
fn validate(
    original: &YamlConfig,
    edited: &str,
    expected: &[RelationEntry],
) -> Result<(), RelationsEditError> {
    let reparsed: YamlConfig = serde_yaml::from_str(edited).map_err(|e| {
        RelationsEditError::Validation(format!("edited daft.yml no longer parses: {e}"))
    })?;
    let mut want = original.clone();
    want.relations = if expected.is_empty() {
        None
    } else {
        Some(expected.to_vec())
    };
    if reparsed != want {
        return Err(RelationsEditError::Validation(
            "edit did not produce the expected relations — aborted to avoid corrupting daft.yml"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str) -> RelationEntry {
        RelationEntry {
            url: url.into(),
            name: None,
            kind: None,
        }
    }

    fn entry_full(url: &str, name: Option<&str>, kind: Option<&str>) -> RelationEntry {
        RelationEntry {
            url: url.into(),
            name: name.map(String::from),
            kind: kind.map(String::from),
        }
    }

    fn unsupported(r: &Result<String, RelationsEditError>) -> bool {
        matches!(r, Err(RelationsEditError::Unsupported(_)))
    }

    // 1. Append preserves every other byte of the file.
    #[test]
    fn append_preserves_everything_else() {
        let input = "\
# top comment
hooks:
  post-clone:
    jobs:
      - name: setup   # keep fast
        run: ./setup.sh

relations:
  - url: git@github.com:org/a.git   # first
  - url: git@github.com:org/b.git
";
        let out = append_relation(input, &entry("git@github.com:org/c.git")).unwrap();
        let expected = "\
# top comment
hooks:
  post-clone:
    jobs:
      - name: setup   # keep fast
        run: ./setup.sh

relations:
  - url: git@github.com:org/a.git   # first
  - url: git@github.com:org/b.git
  - url: git@github.com:org/c.git
";
        assert_eq!(out, expected);
    }

    // 2. New item matches the existing item indentation.
    #[test]
    fn append_detects_existing_indent() {
        let input = "relations:\n    - url: a\n";
        let out = append_relation(input, &entry("b")).unwrap();
        assert_eq!(out, "relations:\n    - url: a\n    - url: b\n");
    }

    // 3. Appending to the all-comments starter keeps every comment.
    #[test]
    fn append_to_all_comments_starter() {
        let input = "\
# daft.yml — per-repo daft configuration.
#
# Sections below are commented placeholders. Uncomment what you need.

# shared: []
";
        let out = append_relation(input, &entry("git@github.com:org/a.git")).unwrap();
        let expected = "\
# daft.yml — per-repo daft configuration.
#
# Sections below are commented placeholders. Uncomment what you need.

# shared: []

relations:
  - url: git@github.com:org/a.git
";
        assert_eq!(out, expected);
    }

    // 4. Empty file gets a block with no leading blank line.
    #[test]
    fn append_to_empty_string() {
        let out = append_relation("", &entry("git@github.com:org/a.git")).unwrap();
        assert_eq!(out, "relations:\n  - url: git@github.com:org/a.git\n");
    }

    // 5. Bare key with a trailing comment: item lands right after it.
    #[test]
    fn append_under_bare_key_with_inline_comment() {
        let input = "relations:   # cross-repo edges\nhooks: {}\n";
        let out = append_relation(input, &entry("a")).unwrap();
        assert_eq!(
            out,
            "relations:   # cross-repo edges\n  - url: a\nhooks: {}\n"
        );
    }

    // 6. Appending to a file with no trailing newline adds one.
    #[test]
    fn append_file_without_trailing_newline() {
        let input = "layout: sibling";
        let out = append_relation(input, &entry("a")).unwrap();
        assert_eq!(out, "layout: sibling\n\nrelations:\n  - url: a\n");
    }

    // 7. Removing a middle item leaves its neighbours byte-identical.
    #[test]
    fn remove_middle_item_neighbors_byte_identical() {
        let input = "\
relations:
  - url: a
  # note for b
  - url: b
  - url: c
next: x
";
        let out = remove_relation(input, 1).unwrap();
        let expected = "\
relations:
  - url: a
  - url: c
next: x
";
        assert_eq!(out, expected);
    }

    // 8. Removing the last item keeps a following section banner.
    #[test]
    fn remove_last_item_keeps_section_banner() {
        let input = "\
relations:
  - url: a
  - url: b

# --- hooks ---
hooks: {}
";
        let out = remove_relation(input, 1).unwrap();
        let expected = "\
relations:
  - url: a

# --- hooks ---
hooks: {}
";
        assert_eq!(out, expected);
    }

    // 9. Removing the only item drops the block but keeps the banner above it.
    #[test]
    fn remove_only_item_removes_key_keeps_banner_above() {
        let input = "\
# --- graph ---
relations:
  - url: a
hooks: {}
";
        let out = remove_relation(input, 0).unwrap();
        let expected = "\
# --- graph ---
hooks: {}
";
        assert_eq!(out, expected);
    }

    // 10. An unknown extra field line is removed with its item.
    #[test]
    fn remove_item_with_unknown_extra_field_lines() {
        let input = "\
relations:
  - url: a
    team_owner: payments
  - url: b
";
        let out = remove_relation(input, 0).unwrap();
        assert_eq!(out, "relations:\n  - url: b\n");
    }

    // 10b. Unlinking the *first* item keeps a block-header comment that sits
    //      directly under the `relations:` key — it heads the block, not the
    //      entry, so it must survive while the other entries remain. (A comment
    //      above a later item is that item's, and goes with it — see test 7.)
    #[test]
    fn remove_first_item_keeps_block_header_comment() {
        let input = "\
relations:
  # external service dependencies
  - url: a
  - url: b
";
        let out = remove_relation(input, 0).unwrap();
        let expected = "\
relations:
  # external service dependencies
  - url: b
";
        assert_eq!(out, expected);
    }

    // 11. Upsert rewrites only the target item.
    #[test]
    fn upsert_rewrites_only_target_item() {
        let input = "\
relations:
  - url: a
  - url: b
    name: old
  - url: c
";
        let out =
            update_relation(input, 1, &entry_full("b", Some("new"), Some("consumer"))).unwrap();
        let expected = "\
relations:
  - url: a
  - url: b
    name: new
    kind: consumer
  - url: c
";
        assert_eq!(out, expected);
    }

    // 12. Upsert bails when the item carries an unmanaged field.
    #[test]
    fn upsert_bails_on_unknown_field_in_item() {
        let input = "relations:\n  - url: a\n    team_owner: payments\n";
        let r = update_relation(input, 0, &entry_full("a", Some("x"), None));
        assert!(unsupported(&r), "got {r:?}");
    }

    // 13. A no-op upsert returns the input verbatim.
    #[test]
    fn noop_upsert_returns_input_verbatim() {
        let input = "relations:\n  - url: a\n    name: client\n";
        let out = update_relation(input, 0, &entry_full("a", Some("client"), None)).unwrap();
        assert_eq!(out, input);
    }

    // 14. Flow-style list is refused.
    #[test]
    fn bail_flow_style() {
        let input = "relations: [{url: a}, {url: b}]\n";
        assert!(unsupported(&append_relation(input, &entry("c"))));
    }

    // 15. Anchor / alias / tag on the key value is refused.
    #[test]
    fn bail_anchor_or_tag_value() {
        for input in [
            "relations: &deps\n  - url: a\n",
            "relations: !!seq\n  - url: a\n",
        ] {
            assert!(
                unsupported(&append_relation(input, &entry("z"))),
                "input: {input:?}"
            );
        }
    }

    // 16. Zero-indent list items are refused (count cross-check).
    #[test]
    fn bail_zero_indent_items() {
        let input = "relations:\n- url: a\n- url: b\nnext: x\n";
        assert!(unsupported(&append_relation(input, &entry("c"))));
    }

    // 17. Quoted or space-before-colon keys are refused.
    #[test]
    fn bail_quoted_or_spaced_key() {
        for input in ["\"relations\":\n  - url: a\n", "relations :\n  - url: a\n"] {
            assert!(
                unsupported(&append_relation(input, &entry("z"))),
                "input: {input:?}"
            );
        }
    }

    // 18. A duplicate `relations:` key fails at parse time.
    #[test]
    fn bail_duplicate_relations_key() {
        let input = "relations:\n  - url: a\nrelations:\n  - url: b\n";
        assert!(matches!(
            append_relation(input, &entry("c")),
            Err(RelationsEditError::Parse(_))
        ));
    }

    // 19. Multi-document files fail at parse time.
    #[test]
    fn bail_multidocument() {
        let input = "relations:\n  - url: a\n---\nrelations:\n  - url: b\n";
        assert!(matches!(
            append_relation(input, &entry("c")),
            Err(RelationsEditError::Parse(_))
        ));
    }

    // 20. Tab-indented files fail at parse time.
    #[test]
    fn bail_tab_indented_file() {
        let input = "hooks:\n\tpost-clone: {}\n";
        assert!(matches!(
            append_relation(input, &entry("a")),
            Err(RelationsEditError::Parse(_))
        ));
    }

    // 21. A `relations:`-looking line inside a quoted scalar never corrupts.
    #[test]
    fn shadow_key_in_multiline_quoted_scalar_bails() {
        // A quoted scalar whose continuation lines mimic a relations block.
        // Parsed relations is None, but a textual `relations:` + item exist.
        let input = "note: \"line one\nrelations:\n  - url: fake\"\n";
        let r = append_relation(input, &entry("real"));
        assert!(r.is_err(), "must never silently edit: {r:?}");
    }

    // 22. An anchor defined on an item and used elsewhere is protected.
    #[test]
    fn anchor_defined_on_item_used_elsewhere_bails() {
        let input = "relations:\n  - &u\n    url: a\nlayout: *u\n";
        // Removing the anchor-defining item would break the alias; validation
        // (or the parse of the result) rejects it.
        let r = remove_relation(input, 0);
        assert!(r.is_err(), "got {r:?}");
    }

    // 23. CRLF files round-trip with CRLF line endings.
    #[test]
    fn crlf_file_roundtrip() {
        let input = "relations:\r\n  - url: a\r\nnext: x\r\n";
        let appended = append_relation(input, &entry("b")).unwrap();
        assert_eq!(
            appended,
            "relations:\r\n  - url: a\r\n  - url: b\r\nnext: x\r\n"
        );
        let removed = remove_relation(&appended, 0).unwrap();
        assert_eq!(removed, "relations:\r\n  - url: b\r\nnext: x\r\n");
    }

    // 24. Values needing quotes round-trip exactly through render + parse.
    #[test]
    fn quoting_torture_roundtrip() {
        let cases = [
            entry_full("git@github.com:org/a.git", Some("true"), Some("a: b")),
            entry_full("https://x.example/o/r", Some("123"), None),
            entry_full("git@h:o/r.git", Some("- dashy"), Some("has #hash")),
        ];
        for e in cases {
            let out = append_relation("", &e).unwrap();
            let parsed = parse_relations(&out).unwrap();
            assert_eq!(parsed, vec![e]);
        }
    }

    // 25. Null-value key forms.
    #[test]
    fn null_value_key_forms() {
        // Bare `relations:` (null) → append inserts the first item.
        let out = append_relation("relations:\nhooks: {}\n", &entry("a")).unwrap();
        assert_eq!(out, "relations:\n  - url: a\nhooks: {}\n");
        // `relations: ~` is an explicit null scalar value → not an editable
        // bare key; refused.
        assert!(unsupported(&append_relation("relations: ~\n", &entry("a"))));
    }

    // Extra: parse_relations reads what serde would.
    #[test]
    fn parse_relations_reads_entries() {
        let input = "relations:\n  - url: a\n    kind: consumer\n  - url: b\n";
        let got = parse_relations(input).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].kind.as_deref(), Some("consumer"));
        assert_eq!(got[1].url, "b");
        assert!(parse_relations("hooks: {}\n").unwrap().is_empty());
    }
}
