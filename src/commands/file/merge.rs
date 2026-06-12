//! Implementation of `daft file merge` — merge a source daft.yml into a target.
//!
//! Merge semantics: recursive YAML merge via the existing `merge_configs` /
//! `merge_hook_defs` functions used at load time. Source wins on conflicts.
//! After a successful merge the source file is deleted unless `--keep-source`
//! is passed. When the target is currently untracked the command prompts before
//! writing unless `--yes` / `--force` is passed.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::{Path, PathBuf};

use crate::hooks::config_merge::merge_configs;
use crate::hooks::yaml_config::YamlConfig;
use crate::hooks::yaml_config_loader::parse_yaml_config_str;

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
/// When the source is a worktree-root daft file with seed provenance, the
/// merge is THREE-WAY against the seed: only genuine refinements move, a
/// key-level preview is printed first, conflicting keys require a side
/// choice (`-y` = source wins), and the target is backed up before writing.
/// Without provenance the legacy two-way merge applies (source wins on all
/// conflicts), guarded by the untracked-target confirmation. After a
/// successful merge `source` is deleted unless `opts.keep_source`.
pub fn merge_files(target: &Path, source: &Path, opts: MergeOptions) -> Result<()> {
    // Guard: source must exist.
    if !source.exists() {
        anyhow::bail!("Source file does not exist: {}", source.display());
    }

    // Guard: source and target must not be the same path (lexical check).
    if target == source {
        anyhow::bail!("Target and source are the same file: {}", target.display());
    }

    // Three-way path: the source has seed provenance and the target exists
    // to merge into.
    if target.is_file()
        && let Some(provenance) = resolve_source_provenance(source)
        && let Some(seed) = provenance
            .seeds
            .get_seed(&provenance.branch, &provenance.filename)
        && let Ok(base_config) = parse_yaml_config_str(&seed.content)
    {
        return merge_three_way(target, source, &opts, &provenance, base_config);
    }

    // Legacy two-way path. When target exists and is untracked, ask for
    // confirmation (unless --yes).
    if target.is_file() && !opts.yes && !confirm_untracked_overwrite(target)? {
        return Ok(());
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

/// Seed provenance of the source file: the store handle plus the branch and
/// filename keying its seed.
struct SourceProvenance {
    seeds: crate::hooks::visitor_seeds::SeedsContext,
    branch: String,
    filename: String,
    /// Git common dir of the source's repo (also the target's in the
    /// in-repo consolidation flow) — where backups land.
    git_common_dir: std::path::PathBuf,
}

/// Resolve seed provenance for `source`: it must sit at the root of a git
/// worktree with a resolvable branch and an openable seed store. Any failure
/// returns `None` → the legacy two-way path.
fn resolve_source_provenance(source: &Path) -> Option<SourceProvenance> {
    let canonical = std::fs::canonicalize(source).ok()?;
    let parent = canonical.parent()?;
    let filename = canonical.file_name()?.to_str()?.to_string();

    let toplevel = git_stdout(parent, &["rev-parse", "--show-toplevel"])?;
    let toplevel = std::fs::canonicalize(toplevel).ok()?;
    if toplevel != parent {
        // Seeds only exist for worktree-root daft files.
        return None;
    }
    let branch = git_stdout(parent, &["symbolic-ref", "--short", "HEAD"])?;
    let common = git_stdout(
        parent,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let git_common_dir = std::path::PathBuf::from(common);
    let seeds = crate::hooks::visitor_seeds::SeedsContext::open(&git_common_dir)?;
    Some(SourceProvenance {
        seeds,
        branch,
        filename,
        git_common_dir,
    })
}

/// Run a git query at `dir`, returning trimmed stdout on success. Both
/// pipes are captured (Test Hygiene: never leak `fatal:` probes to stderr).
fn git_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    let out = crate::utils::git_command_at(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// The seeded three-way merge: preview, conflict resolution, backup, write,
/// seed bookkeeping.
fn merge_three_way(
    target: &Path,
    source: &Path,
    opts: &MergeOptions,
    provenance: &SourceProvenance,
    base_config: YamlConfig,
) -> Result<()> {
    use crate::hooks::config_merge::merge3;

    let source_content = std::fs::read_to_string(source)
        .with_context(|| format!("Failed to read source file: {}", source.display()))?;
    let source_config = parse_yaml_config_str(&source_content)
        .with_context(|| format!("Failed to parse source file: {}", source.display()))?;
    let target_content = std::fs::read_to_string(target)
        .with_context(|| format!("Failed to read target file: {}", target.display()))?;
    let target_config = parse_yaml_config_str(&target_content)
        .with_context(|| format!("Failed to parse target file: {}", target.display()))?;

    // ours = target (it survives), theirs = source.
    let outcome = merge3(&base_config, &target_config, &source_config);

    eprintln!(
        "Merging {} into {} (three-way against its seed)",
        source.display(),
        target.display()
    );
    if !outcome.took_from_theirs.is_empty() {
        eprintln!("  will adopt: {}", outcome.took_from_theirs.join(", "));
    }
    if !outcome.conflicts.is_empty() {
        eprintln!("  conflicting keys: {}", outcome.conflicts.join(", "));
    }

    let resolved = if outcome.conflicts.is_empty() {
        if outcome.took_from_theirs.is_empty() && outcome.merged == target_config {
            // Nothing the target lacks: don't rewrite (and re-canonicalize)
            // a file that would not change.
            eprintln!("  nothing to adopt — target already covers the source");
            finish_seed_bookkeeping(source, opts, provenance)?;
            return Ok(());
        }
        outcome.merged
    } else if opts.yes {
        // Explicit -y: the user asked for source-wins resolution.
        merge3(&base_config, &source_config, &target_config).merged
    } else {
        match prompt_conflict_side(&provenance.filename, &outcome.conflicts) {
            Some(ConflictResolution::Source) => {
                merge3(&base_config, &source_config, &target_config).merged
            }
            Some(ConflictResolution::Target) => outcome.merged,
            None => anyhow::bail!(
                "{} has conflicting keys ({}); pick a side interactively or pass -y to \
                 take the source's values",
                provenance.filename,
                outcome.conflicts.join(", ")
            ),
        }
    };

    // Back up the target before the only copy of its content is rewritten —
    // these files are untracked, so daft provides the undo.
    if let Some(dest) = crate::hooks::visitor_seeds::stash_file(
        &provenance.git_common_dir,
        crate::hooks::visitor_seeds::StashKind::Backup,
        "file-merge",
        target,
    ) {
        eprintln!("  backed up target to {}", dest.display());
    }

    let serialized =
        serde_yaml::to_string(&resolved).context("Failed to serialize merged config")?;
    std::fs::write(target, &serialized)
        .with_context(|| format!("Failed to write target file: {}", target.display()))?;

    finish_seed_bookkeeping(source, opts, provenance)?;
    Ok(())
}

/// Delete or keep the source per options and keep the seed store coherent:
/// a deleted source drops its seed row; a kept source is re-seeded with its
/// current content (it is now consolidated — pristine relative to the new
/// seed).
fn finish_seed_bookkeeping(
    source: &Path,
    opts: &MergeOptions,
    provenance: &SourceProvenance,
) -> Result<()> {
    if opts.keep_source {
        if let Ok(content) = std::fs::read_to_string(source) {
            provenance.seeds.record_seed_content(
                &provenance.branch,
                &provenance.filename,
                &content,
            );
        }
    } else {
        std::fs::remove_file(source)
            .with_context(|| format!("Failed to delete source file: {}", source.display()))?;
        provenance
            .seeds
            .delete_seed(&provenance.branch, &provenance.filename);
    }
    Ok(())
}

enum ConflictResolution {
    Source,
    Target,
}

/// Ask which side wins the conflicting keys. `None` = abort (default, and
/// the answer in every non-interactive context).
fn prompt_conflict_side(filename: &str, keys: &[String]) -> Option<ConflictResolution> {
    use crate::prompt::{PromptConfig, PromptOption, PromptResult, single_key_select};
    eprint!(
        "{filename}: keep the target's version or take the source's for {}? [s/t/A] ",
        keys.join(", ")
    );
    let result = single_key_select(&PromptConfig {
        options: vec![
            PromptOption {
                key: 's',
                label: "source",
                is_default: false,
            },
            PromptOption {
                key: 't',
                label: "target",
                is_default: false,
            },
            PromptOption {
                key: 'a',
                label: "abort",
                is_default: true,
            },
        ],
        cancel_message: Some("Aborted.".to_string()),
    });
    eprintln!();
    match result {
        PromptResult::Selected('s') => Some(ConflictResolution::Source),
        PromptResult::Selected('t') => Some(ConflictResolution::Target),
        _ => None,
    }
}

/// Untracked-target overwrite confirmation for the legacy two-way path.
/// Returns `false` when the user declines (the command exits successfully
/// without writing, preserving the historical behavior).
fn confirm_untracked_overwrite(target: &Path) -> Result<bool> {
    use crate::prompt::{PromptConfig, PromptOption, PromptResult, single_key_select};
    if !is_target_untracked(target)? {
        return Ok(true);
    }
    eprint!(
        "Target '{}' is untracked. Overwrite it? [y/N] ",
        target.display()
    );
    let result = single_key_select(&PromptConfig {
        options: vec![
            PromptOption {
                key: 'y',
                label: "yes",
                is_default: false,
            },
            PromptOption {
                key: 'n',
                label: "no",
                is_default: true,
            },
        ],
        cancel_message: Some("Aborted.".to_string()),
    });
    eprintln!();
    match result {
        PromptResult::Selected('y') => Ok(true),
        _ => {
            eprintln!("Aborted by user.");
            Ok(false)
        }
    }
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
