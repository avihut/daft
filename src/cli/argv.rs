//! Pure parser for top-level CLI flags that must be handled before clap dispatch.
//!
//! Currently handles `-C <path>` with `git -C` semantics (multiple flags compose,
//! empty path is a no-op). The imperative shell that applies the chdir and
//! installs the stripped argv lives in `super`.

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, PartialEq, Eq)]
pub struct ParseResult {
    /// Paths to chdir into, in argv order. Each is applied as a separate
    /// `set_current_dir` call so relative paths compose (matching git).
    /// Empty paths (from `-C ""`) are retained so the caller can choose how to
    /// handle them; the standard git behavior is no-op.
    pub chdir_paths: Vec<PathBuf>,
    /// argv with the consumed `-C <path>` pairs removed. argv[0] (program name)
    /// is always preserved.
    pub stripped: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("option requires an argument")]
    MissingPathAfterC,
}

/// Parse top-level options off the front of argv.
///
/// Stops at the first non-option token (the subcommand name) so that
/// subcommand-local flags with the same name are preserved untouched.
/// `-C "" ` is preserved as an empty `PathBuf` so the caller can implement
/// git's "no-op" semantic without losing the fact that the flag was used.
pub fn parse_top_level_cwd(argv: &[String]) -> Result<ParseResult, ParseError> {
    let mut chdir_paths = Vec::new();
    let mut stripped = Vec::with_capacity(argv.len());

    let Some((program, rest)) = argv.split_first() else {
        return Ok(ParseResult {
            chdir_paths,
            stripped,
        });
    };
    stripped.push(program.clone());

    let mut iter = rest.iter();
    while let Some(tok) = iter.next() {
        if tok == "-C" {
            let path = iter.next().ok_or(ParseError::MissingPathAfterC)?;
            chdir_paths.push(PathBuf::from(path));
            continue;
        }
        // First non-option token (or unknown option) stops the scan. Push it
        // and the rest verbatim — those belong to the subcommand.
        stripped.push(tok.clone());
        stripped.extend(iter.cloned());
        break;
    }

    Ok(ParseResult {
        chdir_paths,
        stripped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn paths(items: &[&str]) -> Vec<PathBuf> {
        items.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn empty_argv() {
        let r = parse_top_level_cwd(&[]).unwrap();
        assert!(r.chdir_paths.is_empty());
        assert!(r.stripped.is_empty());
    }

    #[test]
    fn no_flags() {
        let r = parse_top_level_cwd(&s(&["daft", "list"])).unwrap();
        assert!(r.chdir_paths.is_empty());
        assert_eq!(r.stripped, s(&["daft", "list"]));
    }

    #[test]
    fn single_c_flag() {
        let r = parse_top_level_cwd(&s(&["daft", "-C", "/tmp", "list"])).unwrap();
        assert_eq!(r.chdir_paths, paths(&["/tmp"]));
        assert_eq!(r.stripped, s(&["daft", "list"]));
    }

    #[test]
    fn compose_two_c_flags() {
        let r = parse_top_level_cwd(&s(&["daft", "-C", "/tmp", "-C", "foo", "list"])).unwrap();
        assert_eq!(r.chdir_paths, paths(&["/tmp", "foo"]));
        assert_eq!(r.stripped, s(&["daft", "list"]));
    }

    #[test]
    fn stops_at_subcommand_preserves_inner_c() {
        // Once `bar` is hit (non-option), the scan stops. The inner `-C baz`
        // belongs to the subcommand and must be preserved verbatim.
        let r = parse_top_level_cwd(&s(&[
            "daft", "-C", "/tmp", "-C", "foo", "bar", "list", "-C", "baz",
        ]))
        .unwrap();
        assert_eq!(r.chdir_paths, paths(&["/tmp", "foo"]));
        assert_eq!(r.stripped, s(&["daft", "bar", "list", "-C", "baz"]));
    }

    #[test]
    fn empty_path_preserved() {
        // git's `-C ""` semantic: the flag was used, but it's a no-op. The
        // imperative shell decides how to handle the empty path; the parser
        // just preserves it.
        let r = parse_top_level_cwd(&s(&["daft", "-C", "", "list"])).unwrap();
        assert_eq!(r.chdir_paths, paths(&[""]));
        assert_eq!(r.stripped, s(&["daft", "list"]));
    }

    #[test]
    fn trailing_c_missing_path() {
        let err = parse_top_level_cwd(&s(&["daft", "-C"])).unwrap_err();
        assert_eq!(err, ParseError::MissingPathAfterC);
    }

    #[test]
    fn unknown_long_option_stops_scan() {
        // Any token that isn't `-C` stops the scan. `--version` belongs to
        // the dispatch layer in main.rs (or to a subcommand) — the parser
        // doesn't know about it and must not consume it.
        let r = parse_top_level_cwd(&s(&["daft", "--version"])).unwrap();
        assert!(r.chdir_paths.is_empty());
        assert_eq!(r.stripped, s(&["daft", "--version"]));
    }

    #[test]
    fn symlink_entry_with_c() {
        // Invocation via a symlinked entry: argv[0] is the symlink name, not
        // "daft". Behavior must be identical.
        let r = parse_top_level_cwd(&s(&[
            "git-worktree-checkout",
            "-C",
            "/tmp/repo",
            "newbranch",
        ]))
        .unwrap();
        assert_eq!(r.chdir_paths, paths(&["/tmp/repo"]));
        assert_eq!(r.stripped, s(&["git-worktree-checkout", "newbranch"]));
    }

    #[test]
    fn program_name_only() {
        let r = parse_top_level_cwd(&s(&["daft"])).unwrap();
        assert!(r.chdir_paths.is_empty());
        assert_eq!(r.stripped, s(&["daft"]));
    }

    #[test]
    fn relative_paths_passed_through() {
        // Resolution happens in the imperative shell (`set_current_dir`); the
        // parser just records the strings.
        let r = parse_top_level_cwd(&s(&["daft", "-C", "../sibling", "list"])).unwrap();
        assert_eq!(r.chdir_paths, paths(&["../sibling"]));
    }
}
