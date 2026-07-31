//! `daft hooks import` — the rung from assessment to commitment.
//!
//! Takeover answers "would this work for us" without touching the tracked
//! tree. Import answers "yes" by writing the answer down in daft's own
//! vocabulary, so the team's gates stop depending on a second tool's file.
//!
//! The incumbent file is never deleted. Removing someone's config is their
//! decision, and precedence already makes the import take effect — so the
//! command explains that and prints the `git rm` rather than running it.

use crate::hooks::hooks_block_edit::{self, Entries};
use crate::hooks::incumbent;
use crate::output::Output;
use crate::styles::{bold, cyan, dim, green};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Fold a definition's legacy `commands:` map into its `jobs:` list.
///
/// The map is unordered, so the jobs are sorted by name to keep the written
/// file stable across runs — an import that reshuffled its output every time
/// would make the diff unreadable.
fn commands_to_jobs(
    mut def: crate::hooks::yaml_config::HookDef,
) -> crate::hooks::yaml_config::HookDef {
    let Some(commands) = def.commands.take() else {
        return def;
    };
    let mut converted: Vec<_> = commands
        .iter()
        .map(|(name, cmd)| cmd.to_job_def(name))
        .collect();
    converted.sort_by(|a, b| a.name.cmp(&b.name));
    def.jobs.get_or_insert_with(Vec::new).extend(converted);
    def
}

/// Convert an incumbent hooks config into `daft.yml` entries.
pub(super) fn cmd_import(dry_run: bool, output: &mut dyn Output) -> Result<()> {
    let root = super::find_worktree_root()?;

    let Some(spec) = incumbent::detect(&root)? else {
        anyhow::bail!(
            "no hooks config to import — daft reads lefthook.yml (and its .yaml / \
             dot-prefixed spellings). Write the stages directly in daft.yml's `hooks:` \
             block instead."
        );
    };

    // Emitted in daft's own idiom, not transliterated. The incumbent's older
    // `commands:` map is an accepted alias on a hook, but `tasks:` rejects it
    // outright — an import that wrote it would produce a daft.yml that
    // `daft hooks validate` fails on, one command after telling the user it
    // succeeded.
    let hooks: Entries = spec
        .config
        .hooks
        .clone()
        .into_iter()
        .map(|(name, def)| (name, commands_to_jobs(def)))
        .collect();
    let tasks: Entries = spec
        .config
        .tasks
        .clone()
        .into_iter()
        .map(|(name, def)| (name, commands_to_jobs(def)))
        .collect();
    if hooks.is_empty() && tasks.is_empty() {
        anyhow::bail!(
            "{} defines no hooks daft can run — nothing to import",
            spec.path.display()
        );
    }

    let provenance = format!(
        "Imported from {} by `{}`.",
        spec.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| spec.path.display().to_string()),
        crate::daft_cmd("hooks import")
    );

    let (target, existing) = match crate::hooks::yaml_config_loader::find_config_file(&root) {
        Some((path, _)) => {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            (path, Some(text))
        }
        None => (root.join("daft.yml"), None),
    };

    let rendered = match existing.as_deref() {
        Some(text) => hooks_block_edit::append_blocks(text, &hooks, &tasks, &provenance),
        None => hooks_block_edit::fresh_document(&hooks, &tasks, &provenance),
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Everything the file asked for that daft will not do, before the write,
    // so a `--dry-run` reader sees the same list as someone who commits.
    for note in &spec.unsupported {
        output.warning(&format!("{}: {note}", spec.path.display()));
    }

    let hook_names: Vec<&str> = hooks.keys().map(String::as_str).collect();
    let task_names: Vec<&str> = tasks.keys().map(String::as_str).collect();

    if dry_run {
        output.info(&format!(
            "{} {} → {}",
            bold("Would import"),
            spec.path.display(),
            bold(&target.display().to_string())
        ));
        report_names(&hook_names, &task_names, output);
        output.info("");
        output.info(&dim("--- daft.yml (preview) ---"));
        for line in rendered.lines() {
            output.info(line);
        }
        return Ok(());
    }

    write_config(&target, &rendered)?;

    output.success(&format!(
        "Imported {} into {}",
        spec.path.display(),
        bold(&target.display().to_string())
    ));
    report_names(&hook_names, &task_names, output);
    output.info("");
    // The precedence rule is what makes the incumbent file inert, and it is
    // not obvious from anything on disk — say it rather than leaving the
    // reader to wonder which file is running.
    output.info(&format!(
        "{} now wins for every git stage: a native definition takes precedence over \
         {}, which daft no longer reads.",
        bold("daft.yml"),
        spec.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    output.info(&format!(
        "  {} {} — daft will not delete a file it did not write.",
        dim("Remove it when you are ready:"),
        cyan(&format!(
            "git rm {}",
            spec.path
                .strip_prefix(&root)
                .unwrap_or(&spec.path)
                .display()
        ))
    ));
    output.info(&format!(
        "  {} {}",
        dim("Check the result:"),
        cyan(&crate::daft_cmd("hooks validate"))
    ));
    Ok(())
}

fn report_names(hooks: &[&str], tasks: &[&str], output: &mut dyn Output) {
    if !hooks.is_empty() {
        output.info(&format!("  {} {}", green("hooks:"), hooks.join(", ")));
    }
    if !tasks.is_empty() {
        // Custom names are not git events, so they become tasks — say so,
        // because `daft run <name>` is not where a reader would look.
        output.info(&format!(
            "  {} {} {}",
            green("tasks:"),
            tasks.join(", "),
            dim("(not git events — run with `daft run <name>`)")
        ));
    }
}

/// Write the config, preserving the file's permissions when it existed.
fn write_config(path: &PathBuf, contents: &str) -> Result<()> {
    std::fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}
