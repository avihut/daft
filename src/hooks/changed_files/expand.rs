//! Expanding a file list into a job's command, and splitting it when the
//! result would not survive `exec`.

use super::SourceKind;

/// The largest command string daft will hand to one `sh -c`.
///
/// Linux caps a *single* argv element at `MAX_ARG_STRLEN` — 32 pages, 128 KiB
/// on every common configuration — and `sh -c <script>` passes the whole
/// command as one element. Exceeding it is `E2BIG` at exec time, which
/// surfaces as a gate that mysteriously stops working once a repository grows
/// past a few thousand staged files. The budget is deliberately the hard
/// limit rather than a fraction of it: chunking is cheap and the alternative
/// failure is silent-looking.
pub const MAX_EXPANDED_COMMAND_BYTES: usize = 131_072;

/// How a placeholder was written, which decides how its paths are rendered.
///
/// The quoting variants are not decoration. `{files}` produces a
/// shell-quoted, space-separated list — right for a command that takes many
/// paths. But a job doing `for f in {files}` and one doing
/// `grep -l TODO "{files}"` want different things, and a schema that offers
/// only one of them forces the other to be written by hand, wrongly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quoting {
    /// `{files}` — each path shell-quoted as needed, space-joined.
    Shell,
    /// `"{files}"` — each path wrapped in double quotes, space-joined, with
    /// the surrounding quotes consumed.
    Double,
    /// `'{files}'` — each path wrapped in single quotes, space-joined, with
    /// the surrounding quotes consumed.
    Single,
}

/// One placeholder occurrence found in a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placeholder {
    /// Which file list it names.
    pub kind: SourceKind,
    /// How its paths should be rendered.
    pub quoting: Quoting,
    /// Byte range in the command string, including any consumed quotes.
    pub start: usize,
    pub end: usize,
}

/// Every file placeholder in `command`, in order.
///
/// `{changed_files}` is scanned as an exact synonym of `{files}` — the
/// original spelling, kept because repositories are already written against
/// it and a rename that silently expands to nothing is the worst kind.
pub fn find_placeholders(command: &str) -> Vec<Placeholder> {
    let mut spellings: Vec<(&str, SourceKind)> = SourceKind::all()
        .iter()
        .map(|k| (k.placeholder(), *k))
        .collect();
    spellings.push((super::CHANGED_FILES_TEMPLATE, SourceKind::Operation));

    let mut found = Vec::new();
    for (token, kind) in spellings {
        let mut from = 0;
        while let Some(rel) = command[from..].find(token) {
            let start = from + rel;
            let end = start + token.len();
            // A quote on both sides is part of the placeholder, and the
            // rendering it asks for.
            let quoting = match (
                command[..start].chars().next_back(),
                command[end..].chars().next(),
            ) {
                (Some('"'), Some('"')) => Quoting::Double,
                (Some('\''), Some('\'')) => Quoting::Single,
                _ => Quoting::Shell,
            };
            let (start, end) = match quoting {
                Quoting::Shell => (start, end),
                _ => (start - 1, end + 1),
            };
            found.push(Placeholder {
                kind,
                quoting,
                start,
                end,
            });
            from = end;
        }
    }
    found.sort_by_key(|p| p.start);
    found
}

/// Render one file list per this placeholder's quoting.
pub fn render(files: &[String], quoting: Quoting) -> String {
    match quoting {
        Quoting::Shell => crate::utils::quote_argv(files),
        Quoting::Double => files
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect::<Vec<_>>()
            .join(" "),
        Quoting::Single => files
            .iter()
            .map(|f| format!("'{f}'"))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Substitute every placeholder in `command`, splitting into several commands
/// when one would exceed [`MAX_EXPANDED_COMMAND_BYTES`].
///
/// `resolved` supplies the filtered list for each placeholder, in the order
/// [`find_placeholders`] returned them.
///
/// Splitting is only attempted when exactly one placeholder is present.
/// Chunking a command with two different file lists in it has no single right
/// answer — pairing them up assumes a correspondence that does not exist, and
/// crossing them multiplies the runs — so such a command is expanded whole,
/// and if it is too long the job fails loudly at exec rather than running a
/// partition nobody asked for.
///
/// The returned vector is never empty; its first element is the command as
/// executed, the rest are follow-on chunks run in sequence, stopping at the
/// first failure.
pub fn expand_and_chunk(command: &str, placeholders: &[(Placeholder, Vec<String>)]) -> Vec<String> {
    if placeholders.is_empty() {
        return vec![command.to_string()];
    }

    let whole = substitute_all(command, placeholders);
    if whole.len() <= MAX_EXPANDED_COMMAND_BYTES || placeholders.len() != 1 {
        return vec![whole];
    }

    let (placeholder, files) = &placeholders[0];
    // Budget for the paths themselves: the command minus the placeholder it
    // replaces. A command whose fixed part alone exceeds the limit cannot be
    // helped by chunking, so it goes out whole and fails honestly.
    let fixed = command.len() - (placeholder.end - placeholder.start);
    if fixed >= MAX_EXPANDED_COMMAND_BYTES {
        return vec![whole];
    }
    let budget = MAX_EXPANDED_COMMAND_BYTES - fixed;

    let mut chunks = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_len = 0usize;
    for file in files {
        // +1 for the joining space; the rendered form is at least as long as
        // the raw path, so this under-counts quoting and over-chunks slightly
        // rather than over-filling.
        let cost = render(std::slice::from_ref(file), placeholder.quoting).len() + 1;
        if !current.is_empty() && current_len + cost > budget {
            chunks.push(substitute_all(
                command,
                &[(*placeholder, std::mem::take(&mut current))],
            ));
            current_len = 0;
        }
        current.push(file.clone());
        current_len += cost;
    }
    if !current.is_empty() {
        chunks.push(substitute_all(command, &[(*placeholder, current)]));
    }
    if chunks.is_empty() {
        vec![whole]
    } else {
        chunks
    }
}

/// Replace each placeholder's byte range with its rendered list, right to
/// left so earlier offsets stay valid.
fn substitute_all(command: &str, placeholders: &[(Placeholder, Vec<String>)]) -> String {
    let mut out = command.to_string();
    let mut ordered: Vec<&(Placeholder, Vec<String>)> = placeholders.iter().collect();
    ordered.sort_by_key(|(p, _)| std::cmp::Reverse(p.start));
    for (placeholder, files) in ordered {
        out.replace_range(
            placeholder.start..placeholder.end,
            &render(files, placeholder.quoting),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_bare_placeholder_is_shell_quoted() {
        let found = find_placeholders("eslint {files}");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, SourceKind::Operation);
        assert_eq!(found[0].quoting, Quoting::Shell);
        let out = expand_and_chunk(
            "eslint {files}",
            &[(found[0], strs(&["a.ts", "my dir/b.ts"]))],
        );
        assert_eq!(out, vec!["eslint a.ts 'my dir/b.ts'"]);
    }

    #[test]
    fn surrounding_quotes_are_consumed_and_applied_per_path() {
        // Without consuming them, `"{files}"` would produce one double-quoted
        // string containing every path — a single argument, not a list.
        let cmd = "grep -l TODO \"{files}\"";
        let found = find_placeholders(cmd);
        assert_eq!(found[0].quoting, Quoting::Double);
        let out = expand_and_chunk(cmd, &[(found[0], strs(&["a.rs", "b.rs"]))]);
        assert_eq!(out, vec!["grep -l TODO \"a.rs\" \"b.rs\""]);

        let cmd = "printf '%s\\n' '{files}'";
        let found = find_placeholders(cmd);
        // The `'%s\n'` literal is not a placeholder, so only one is found.
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].quoting, Quoting::Single);
        let out = expand_and_chunk(cmd, &[(found[0], strs(&["a.rs"]))]);
        assert_eq!(out, vec!["printf '%s\\n' 'a.rs'"]);
    }

    #[test]
    fn changed_files_is_an_exact_synonym_of_files() {
        let found = find_placeholders("lint {changed_files}");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, SourceKind::Operation);
    }

    #[test]
    fn each_named_source_is_recognised() {
        let found = find_placeholders("a {staged_files} b {push_files} c {all_files}");
        let kinds: Vec<SourceKind> = found.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SourceKind::Staged,
                SourceKind::Pushed,
                SourceKind::AllTracked
            ]
        );
    }

    #[test]
    fn placeholders_come_back_in_command_order() {
        // Ordering matters: the caller pairs resolved lists with placeholders
        // positionally, and substitution walks them right to left.
        let found = find_placeholders("cat {all_files} {staged_files} {files}");
        assert!(found.windows(2).all(|w| w[0].start < w[1].start));
        assert_eq!(found[0].kind, SourceKind::AllTracked);
        assert_eq!(found[2].kind, SourceKind::Operation);
    }

    #[test]
    fn two_different_lists_expand_in_one_command() {
        let cmd = "diff <(cat {staged_files}) <(cat {all_files})";
        let found = find_placeholders(cmd);
        let out = expand_and_chunk(
            cmd,
            &[
                (found[0], strs(&["s.rs"])),
                (found[1], strs(&["a.rs", "b.rs"])),
            ],
        );
        assert_eq!(out, vec!["diff <(cat s.rs) <(cat a.rs b.rs)"]);
    }

    #[test]
    fn a_short_command_is_one_chunk() {
        let found = find_placeholders("fmt {files}");
        let out = expand_and_chunk("fmt {files}", &[(found[0], strs(&["a", "b", "c"]))]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn an_oversized_list_is_split_into_executable_chunks() {
        // The failure this prevents: one `sh -c` argument past MAX_ARG_STRLEN
        // is E2BIG, which reads as "the gate stopped working" once a repo
        // grows past a few thousand files.
        let files: Vec<String> = (0..20_000).map(|i| format!("src/file_{i:06}.rs")).collect();
        let found = find_placeholders("fmt {files}");
        let chunks = expand_and_chunk("fmt {files}", &[(found[0], files.clone())]);

        assert!(chunks.len() > 1, "expected a split, got {}", chunks.len());
        for chunk in &chunks {
            assert!(
                chunk.len() <= MAX_EXPANDED_COMMAND_BYTES,
                "chunk of {} bytes exceeds the exec budget",
                chunk.len()
            );
            assert!(chunk.starts_with("fmt "));
        }
        // Every file appears exactly once across the chunks — a split that
        // drops or duplicates a path is a gate that checked the wrong set.
        let joined = chunks.join(" ");
        for file in &files {
            assert_eq!(
                joined.matches(file.as_str()).count(),
                1,
                "{file} appears the wrong number of times"
            );
        }
    }

    #[test]
    fn an_oversized_multi_placeholder_command_is_not_partitioned() {
        // Pairing two lists chunk-for-chunk would assume a correspondence
        // that does not exist; crossing them would multiply the runs. Neither
        // is what the author wrote, so it goes out whole.
        let many: Vec<String> = (0..20_000).map(|i| format!("src/file_{i:06}.rs")).collect();
        let cmd = "diff <(cat {staged_files}) <(cat {all_files})";
        let found = find_placeholders(cmd);
        let out = expand_and_chunk(
            cmd,
            &[(found[0], many.clone()), (found[1], strs(&["a.rs"]))],
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_command_too_long_without_its_files_is_left_alone() {
        // Chunking cannot help when the fixed part alone busts the budget.
        let filler = "x".repeat(MAX_EXPANDED_COMMAND_BYTES + 10);
        let cmd = format!("echo {filler} {{files}}");
        let found = find_placeholders(&cmd);
        let out = expand_and_chunk(&cmd, &[(found[0], strs(&["a.rs", "b.rs"]))]);
        assert_eq!(out.len(), 1);
        assert!(out[0].ends_with("a.rs b.rs"));
    }

    #[test]
    fn no_placeholders_returns_the_command_unchanged() {
        assert_eq!(expand_and_chunk("cargo test", &[]), vec!["cargo test"]);
    }
}
