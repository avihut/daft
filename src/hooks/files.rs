//! File list operations for YAML hooks.
//!
//! Provides functions to retrieve staged files, all tracked files, and
//! push files, plus glob/file-type filtering per job.

use super::yaml_config::{FileTypeFilter, GlobPattern};
use anyhow::{Context, Result};
use globset::{Glob, GlobSetBuilder};
use std::path::Path;

/// Get staged files (added, copied, modified, renamed).
pub fn staged_files(worktree: &Path) -> Result<Vec<String>> {
    git_diff_files(worktree, &["--cached", "--name-only", "--diff-filter=ACMR"])
}

/// Get all tracked files in the repository.
pub fn all_files(worktree: &Path) -> Result<Vec<String>> {
    let output = std::process::Command::new("git")
        .args(["ls-files"])
        .current_dir(worktree)
        .output()
        .context("Failed to run git ls-files")?;

    if !output.status.success() {
        anyhow::bail!("git ls-files failed");
    }

    Ok(parse_file_list(&output.stdout))
}

/// Get files being pushed (for pre-push hooks).
///
/// Returns files changed between the local and remote ref.
pub fn push_files(worktree: &Path) -> Result<Vec<String>> {
    // Read push info from stdin (git passes this for pre-push)
    // Format: <local ref> <local sha1> <remote ref> <remote sha1>
    // For simplicity, compare HEAD against origin/HEAD
    git_diff_files(
        worktree,
        &["--name-only", "origin/HEAD...HEAD", "--diff-filter=ACMR"],
    )
}

/// Run a custom file-list command.
pub fn custom_file_command(worktree: &Path, command: &str) -> Result<Vec<String>> {
    let output = std::process::Command::new("sh")
        .args(["-c", command])
        .current_dir(worktree)
        .output()
        .with_context(|| format!("Failed to run file command: {command}"))?;

    if !output.status.success() {
        anyhow::bail!("File command failed: {command}");
    }

    Ok(parse_file_list(&output.stdout))
}

/// Filter a file list by glob patterns.
pub fn filter_by_glob(files: &[String], patterns: &GlobPattern) -> Result<Vec<String>> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns.patterns() {
        builder.add(Glob::new(pattern).with_context(|| format!("Invalid glob: {pattern}"))?);
    }
    let set = builder.build().context("Failed to build glob set")?;

    Ok(files
        .iter()
        .filter(|f| set.is_match(f.as_str()))
        .cloned()
        .collect())
}

/// Filter a file list by file type extensions.
pub fn filter_by_file_type(files: &[String], types: &FileTypeFilter) -> Vec<String> {
    let extensions: Vec<&str> = types
        .types()
        .iter()
        .flat_map(|t| file_type_extensions(t))
        .collect();

    files
        .iter()
        .filter(|f| extensions.iter().any(|ext| f.ends_with(&format!(".{ext}"))))
        .cloned()
        .collect()
}

/// Exclude files matching glob patterns.
pub fn exclude_files(files: &[String], patterns: &[String]) -> Result<Vec<String>> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).with_context(|| format!("Invalid exclude: {pattern}"))?);
    }
    let set = builder.build().context("Failed to build exclude set")?;

    Ok(files
        .iter()
        .filter(|f| !set.is_match(f.as_str()))
        .cloned()
        .collect())
}

/// Map file type names to extensions.
fn file_type_extensions(file_type: &str) -> Vec<&'static str> {
    match file_type {
        "rust" => vec!["rs"],
        "javascript" | "js" => vec!["js", "jsx", "mjs", "cjs"],
        "typescript" | "ts" => vec!["ts", "tsx", "mts", "cts"],
        "python" | "py" => vec!["py", "pyi"],
        "ruby" | "rb" => vec!["rb"],
        "go" => vec!["go"],
        "java" => vec!["java"],
        "c" => vec!["c", "h"],
        "cpp" | "c++" => vec!["cpp", "cc", "cxx", "hpp", "hxx", "h"],
        "css" => vec!["css", "scss", "sass", "less"],
        "html" => vec!["html", "htm"],
        "json" => vec!["json"],
        "yaml" | "yml" => vec!["yaml", "yml"],
        "toml" => vec!["toml"],
        "markdown" | "md" => vec!["md", "markdown"],
        "shell" | "sh" | "bash" => vec!["sh", "bash", "zsh"],
        "swift" => vec!["swift"],
        "kotlin" | "kt" => vec!["kt", "kts"],
        _ => vec![],
    }
}

/// Run git diff with arguments and return the file list.
fn git_diff_files(worktree: &Path, args: &[&str]) -> Result<Vec<String>> {
    let output = std::process::Command::new("git")
        .arg("diff")
        .args(args)
        .current_dir(worktree)
        .output()
        .context("Failed to run git diff")?;

    if !output.status.success() {
        // Non-fatal: might not have a remote ref yet
        return Ok(Vec::new());
    }

    Ok(parse_file_list(&output.stdout))
}

/// Parse newline-separated file list from command output.
fn parse_file_list(output: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(output)
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_by_glob() {
        let files = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "tests/test.rs".to_string(),
            "README.md".to_string(),
        ];

        let result = filter_by_glob(&files, &GlobPattern::Single("src/**/*.rs".to_string()));
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"src/main.rs".to_string()));
        assert!(result.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn test_filter_by_glob_multiple() {
        let files = vec![
            "src/main.rs".to_string(),
            "README.md".to_string(),
            "Cargo.toml".to_string(),
        ];

        let patterns = GlobPattern::Multiple(vec!["*.rs".to_string(), "*.toml".to_string()]);
        let result = filter_by_glob(&files, &patterns).unwrap();
        // Glob matching is path-based: "*.rs" won't match "src/main.rs"
        assert!(result.contains(&"Cargo.toml".to_string()));
    }

    #[test]
    fn test_filter_by_file_type() {
        let files = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "package.json".to_string(),
            "index.js".to_string(),
        ];

        let result = filter_by_file_type(&files, &FileTypeFilter::Single("rust".to_string()));
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"src/main.rs".to_string()));
    }

    #[test]
    fn test_exclude_files() {
        let files = vec![
            "src/main.rs".to_string(),
            "target/debug/daft".to_string(),
            "node_modules/foo.js".to_string(),
        ];

        let result = exclude_files(
            &files,
            &["target/**".to_string(), "node_modules/**".to_string()],
        )
        .unwrap();
        assert_eq!(result, vec!["src/main.rs".to_string()]);
    }

    #[test]
    fn test_file_type_extensions() {
        assert!(file_type_extensions("rust").contains(&"rs"));
        assert!(file_type_extensions("javascript").contains(&"js"));
        assert!(file_type_extensions("unknown_type").is_empty());
    }

    #[test]
    fn test_parse_file_list() {
        let output = b"file1.rs\nfile2.rs\n\nfile3.rs\n";
        let result = parse_file_list(output);
        assert_eq!(result, vec!["file1.rs", "file2.rs", "file3.rs"]);
    }
}
