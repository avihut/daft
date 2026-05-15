//! Implementation of `daft file merge` — merge a source daft.yml into a target.
//!
//! Merge semantics: recursive YAML merge via the existing `merge_configs` /
//! `merge_hook_defs` functions used at load time. Source wins on conflicts.
//! After a successful merge the source file is deleted unless `--keep-source`
//! is passed. When the target is currently untracked the command prompts before
//! writing unless `--yes` / `--force` is passed.

use anyhow::{Context, Result};
use clap::Parser;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::hooks::yaml_config::YamlConfig;
use crate::hooks::yaml_config_loader::{merge_configs, parse_yaml_config_str};

/// Options controlling the merge behaviour.
pub struct MergeOptions {
    /// Keep the source file after a successful merge (do not delete it).
    pub keep_source: bool,
    /// Skip the untracked-target confirmation prompt.
    pub yes: bool,
}

/// CLI arguments for `daft file merge`.
#[derive(Parser)]
#[command(name = "daft file merge")]
#[command(about = "Merge a source daft.yml into a target daft.yml")]
#[command(long_about = "\
Merge SOURCE into TARGET using the same recursive YAML merge that daft uses\n\
at load time: source wins on conflicts, new hook sections are added wholesale.\n\
\n\
When TARGET is omitted, daft.yml in the current directory is used.\n\
\n\
By default the source file is deleted after a successful merge.\n\
When TARGET is untracked (visitor file) you are prompted for confirmation\n\
unless --yes / --force is passed.")]
pub struct Args {
    /// Target file to merge INTO, or source file when TARGET is omitted
    first: PathBuf,

    /// Source file to merge FROM (optional; when omitted, FIRST is the source
    /// and the target defaults to daft.yml in the current directory)
    second: Option<PathBuf>,

    /// Keep the source file after merging (do not delete it)
    #[arg(long)]
    keep_source: bool,

    /// Skip confirmation prompt when the target is untracked
    #[arg(long, short = 'y', alias = "force")]
    yes: bool,
}

/// Run `daft file merge` with the given CLI args (excluding the "merge" verb).
pub fn run(args: &[String]) -> Result<()> {
    let parsed = {
        let mut cli_args = vec!["daft file merge".to_string()];
        cli_args.extend_from_slice(args);
        match Args::try_parse_from(cli_args) {
            Ok(a) => a,
            Err(e) => {
                e.print().ok();
                if e.use_stderr() {
                    std::process::exit(1);
                } else {
                    return Ok(());
                }
            }
        }
    };

    let (target, source) = match parsed.second {
        Some(second) => (parsed.first, second),
        None => {
            // Implied-target form: first arg is the source, target = cwd/daft.yml
            let cwd = std::env::current_dir().context("Failed to determine current directory")?;
            (cwd.join("daft.yml"), parsed.first)
        }
    };

    merge_files(
        &target,
        &source,
        MergeOptions {
            keep_source: parsed.keep_source,
            yes: parsed.yes,
        },
    )
}

/// Merge `source` into `target`, writing the result back to `target`.
///
/// Source wins on all conflicts (both scalar fields and named hook jobs).
/// After a successful merge `source` is deleted unless `opts.keep_source`.
pub fn merge_files(target: &Path, source: &Path, opts: MergeOptions) -> Result<()> {
    // Guard: source must exist.
    if !source.exists() {
        anyhow::bail!("Source file does not exist: {}", source.display());
    }

    // Guard: source and target must not be the same path (lexical check).
    if target == source {
        anyhow::bail!("Target and source are the same file: {}", target.display());
    }

    // When target exists and is untracked, ask for confirmation (unless --yes).
    if target.is_file() && !opts.yes && is_target_untracked(target)? {
        eprint!(
            "Target '{}' is untracked. Overwrite it? [y/N] ",
            target.display()
        );
        let mut input = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut input)
            .context("Failed to read confirmation")?;
        let trimmed = input.trim().to_lowercase();
        if trimmed != "y" && trimmed != "yes" {
            eprintln!("Aborted by user.");
            return Ok(());
        }
    }

    // Load source config.
    let source_content = std::fs::read_to_string(source)
        .with_context(|| format!("Failed to read source file: {}", source.display()))?;
    let source_config = parse_yaml_config_str(&source_content)
        .with_context(|| format!("Failed to parse source file: {}", source.display()))?;

    // Load target config (or default if it doesn't exist yet).
    let base_config = if target.is_file() {
        let target_content = std::fs::read_to_string(target)
            .with_context(|| format!("Failed to read target file: {}", target.display()))?;
        parse_yaml_config_str(&target_content)
            .with_context(|| format!("Failed to parse target file: {}", target.display()))?
    } else {
        YamlConfig::default()
    };

    // Merge: base = target, overlay = source → source wins.
    let merged = merge_configs(base_config, source_config);

    // Serialize and write to target.
    let serialized = serde_yaml::to_string(&merged).context("Failed to serialize merged config")?;
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    std::fs::write(target, &serialized)
        .with_context(|| format!("Failed to write target file: {}", target.display()))?;

    // Delete source unless --keep-source.
    if !opts.keep_source {
        std::fs::remove_file(source)
            .with_context(|| format!("Failed to delete source file: {}", source.display()))?;
    }

    Ok(())
}

/// Returns true when the target file is currently untracked by git.
///
/// Conservative: if git cannot definitively answer (no git binary, parent
/// directory not inside a git repo), returns `false` so the prompt is
/// skipped. Mirrors the two-stage probe pattern from
/// `classify_main_config` in yaml_config_loader.rs.
fn is_target_untracked(target: &Path) -> Result<bool> {
    let dir = target
        .parent()
        .and_then(|p| {
            if p.as_os_str().is_empty() {
                None
            } else {
                Some(p)
            }
        })
        .unwrap_or(Path::new("."));
    let basename = target
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("target has no filename"))?;

    // Stage 1: are we even inside a git work tree?
    let probe = crate::utils::git_command_at(dir)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    let inside_repo = matches!(probe, Ok(s) if s.success());
    if !inside_repo {
        return Ok(false);
    }

    // Stage 2: is the file tracked?
    let ls = crate::utils::git_command_at(dir)
        .args(["ls-files", "--error-unmatch"])
        .arg(basename)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match ls {
        Ok(s) => Ok(!s.success()),
        Err(_) => Ok(false), // git vanished mid-run — conservative
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(dir: &std::path::Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn test_merge_adds_new_hook_from_source() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "target.yml",
            "hooks:\n  post-clone:\n    jobs:\n      - name: a\n        run: echo a\n",
        );
        write(
            dir.path(),
            "source.yml",
            "hooks:\n  worktree-post-create:\n    jobs:\n      - name: b\n        run: echo b\n",
        );

        merge_files(
            &dir.path().join("target.yml"),
            &dir.path().join("source.yml"),
            MergeOptions {
                keep_source: false,
                yes: true,
            },
        )
        .unwrap();

        let merged = fs::read_to_string(dir.path().join("target.yml")).unwrap();
        assert!(merged.contains("post-clone"));
        assert!(merged.contains("worktree-post-create"));
        assert!(
            !dir.path().join("source.yml").exists(),
            "source should be deleted by default"
        );
    }

    #[test]
    fn test_merge_keep_source() {
        let dir = tempdir().unwrap();
        write(dir.path(), "target.yml", "hooks: {}");
        write(dir.path(), "source.yml", "hooks: {}");

        merge_files(
            &dir.path().join("target.yml"),
            &dir.path().join("source.yml"),
            MergeOptions {
                keep_source: true,
                yes: true,
            },
        )
        .unwrap();

        assert!(
            dir.path().join("source.yml").exists(),
            "source should be kept"
        );
    }

    #[test]
    fn test_merge_source_wins_on_conflict() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "target.yml",
            "hooks:\n  post-clone:\n    jobs:\n      - name: lint\n        run: echo target\n",
        );
        write(
            dir.path(),
            "source.yml",
            "hooks:\n  post-clone:\n    jobs:\n      - name: lint\n        run: echo source\n",
        );

        merge_files(
            &dir.path().join("target.yml"),
            &dir.path().join("source.yml"),
            MergeOptions {
                keep_source: true,
                yes: true,
            },
        )
        .unwrap();

        let merged = fs::read_to_string(dir.path().join("target.yml")).unwrap();
        assert!(merged.contains("echo source"));
        assert!(!merged.contains("echo target"));
    }

    #[test]
    fn test_merge_outside_git_repo_does_not_prompt() {
        // When called from a non-git directory, the function should NOT
        // require --yes/--force. The merge proceeds without prompting.
        let dir = tempdir().unwrap();
        // No git init — pure filesystem dir.
        write(dir.path(), "target.yml", "hooks: {}");
        write(
            dir.path(),
            "source.yml",
            "hooks:\n  post-clone:\n    jobs:\n      - run: echo s\n",
        );

        // yes: false — yet should succeed without a prompt, because the
        // target is not in a git repo, so it's not "untracked" in the
        // visitor sense.
        merge_files(
            &dir.path().join("target.yml"),
            &dir.path().join("source.yml"),
            MergeOptions {
                keep_source: false,
                yes: false,
            },
        )
        .unwrap();

        let merged = fs::read_to_string(dir.path().join("target.yml")).unwrap();
        assert!(merged.contains("post-clone"));
    }
}
