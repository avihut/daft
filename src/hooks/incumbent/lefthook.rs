//! Reading a lefthook config into daft's shapes.
//!
//! The only module above the output recognizer that names the tool. Its job
//! is translation, not compatibility: the schema daft implements was
//! deliberately forked from lefthook's documented interface, so most keys
//! land unchanged and the interesting code is the handful that do not.
//!
//! This is a reader, never a writer. daft does not modify a repository's
//! lefthook config, and `daft hooks import` produces daft's own file beside
//! it rather than converting in place.

use super::IncumbentSpec;
use crate::hooks::yaml_config::{HookDef, JobDef, RunCommand, YamlConfig};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Config filenames, in the order lefthook itself resolves them.
const CONFIG_NAMES: &[&str] = &[
    "lefthook.yml",
    "lefthook.yaml",
    ".lefthook.yml",
    ".lefthook.yaml",
];

/// Overlay filenames, applied on top of the main config.
const LOCAL_CONFIG_NAMES: &[&str] = &[
    "lefthook-local.yml",
    "lefthook-local.yaml",
    ".lefthook-local.yml",
    ".lefthook-local.yaml",
];

/// Formats lefthook accepts that daft does not read.
///
/// Detected so the error can say *why* nothing was found, rather than
/// reporting no incumbent config in a repository that visibly has one.
const OTHER_FORMATS: &[&str] = &[
    "lefthook.toml",
    "lefthook.json",
    ".lefthook.toml",
    ".lefthook.json",
];

/// Environment variable that disables the incumbent's hooks.
///
/// Honoured only when running an incumbent config — a repository that has
/// migrated to daft's own keys should not still be steerable by the old
/// tool's variables, or the migration is not finished.
const DISABLE_VAR: &str = "LEFTHOOK";

/// Environment variable naming jobs to skip.
///
/// Public so the skips it causes can be attributed to it by name — reporting
/// them against daft's own `--skip-hooks` would name a flag the user did not
/// pass.
pub const EXCLUDE_VAR: &str = "LEFTHOOK_EXCLUDE";

/// The incumbent config file in `root`, if there is one.
pub fn config_path(root: &Path) -> Option<PathBuf> {
    CONFIG_NAMES
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
}

/// A config file in a format daft does not read, if one is there instead.
fn other_format_path(root: &Path) -> Option<PathBuf> {
    OTHER_FORMATS
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
}

/// Read and translate the incumbent config in `root`.
pub fn read(root: &Path) -> Result<Option<IncumbentSpec>> {
    let Some(path) = config_path(root) else {
        if let Some(other) = other_format_path(root) {
            anyhow::bail!(
                "{} is in a format daft does not read — only YAML. Convert it to \
                 lefthook.yml, or define the stages in daft.yml directly.",
                other.display()
            );
        }
        return Ok(None);
    };

    let mut unsupported = Vec::new();
    let mut config = parse_file(&path, &mut unsupported)?;

    // The local overlay, if present. Same precedence lefthook gives it:
    // later wins, per key.
    if let Some(local) = LOCAL_CONFIG_NAMES
        .iter()
        .map(|name| root.join(name))
        .find(|p| p.is_file())
    {
        let overlay = parse_file(&local, &mut unsupported)?;
        config = crate::hooks::config_merge::merge_configs(config, overlay);
    }

    Ok(Some(IncumbentSpec {
        path,
        tool: "lefthook",
        config,
        unsupported,
    }))
}

/// Whether the incumbent's own environment variables disable this run.
///
/// Only consulted in takeover mode; see [`DISABLE_VAR`].
pub fn disabled_by_env() -> bool {
    std::env::var(DISABLE_VAR).is_ok_and(|v| matches!(v.trim(), "0" | "false" | "off" | "no"))
}

/// Job names the incumbent's environment excludes from this run.
pub fn excluded_jobs() -> Vec<String> {
    std::env::var(EXCLUDE_VAR)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// The shape of a lefthook config file, as far as daft reads it.
///
/// Hook entries live at the top level rather than under a `hooks:` key, so
/// this captures the known top-level settings and sweeps the rest into
/// `rest`, where each entry is a hook (or a refused key).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    min_version: Option<String>,
    colors: Option<bool>,
    no_tty: Option<bool>,
    rc: Option<String>,
    source_dir: Option<String>,
    source_dir_local: Option<String>,
    extends: Option<Vec<String>>,
    output: Option<crate::hooks::yaml_config::OutputSetting>,
    templates: Option<HashMap<String, String>>,
    skip_lfs: Option<bool>,
    #[serde(flatten)]
    rest: HashMap<String, serde_yaml::Value>,
}

/// Parse one file into daft's config shape.
fn parse_file(path: &Path, unsupported: &mut Vec<String>) -> Result<YamlConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let parsed: RawConfig = serde_yaml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let mut config = YamlConfig {
        // `min_version` is deliberately dropped rather than enforced: it
        // constrains the tool that wrote the file, and daft is not that tool.
        // Refusing to run over a version number would fail every takeover of
        // a config pinned above whatever daft happened to claim.
        colors: parsed.colors,
        no_tty: parsed.no_tty,
        rc: parsed.rc,
        source_dir: parsed.source_dir,
        source_dir_local: parsed.source_dir_local,
        output: parsed.output,
        templates: parsed.templates,
        skip_lfs: parsed.skip_lfs,
        ..Default::default()
    };
    if let Some(version) = parsed.min_version {
        unsupported.push(format!(
            "min_version: {version} is a constraint on the tool that wrote this file, \
             not on daft — ignored"
        ));
    }
    if parsed.extends.is_some() {
        unsupported.push(
            "extends: is not followed when running this config directly — inline the \
             fragments, or import into daft.yml, which does support it"
                .to_string(),
        );
    }

    for (key, value) in parsed.rest {
        translate_entry(&key, value, &mut config, unsupported)?;
    }
    Ok(config)
}

/// Translate one top-level entry: a hook, a refused key, or a custom name.
fn translate_entry(
    key: &str,
    value: serde_yaml::Value,
    config: &mut YamlConfig,
    unsupported: &mut Vec<String>,
) -> Result<()> {
    use crate::hooks::git_stage::{self, GitStage};

    // `remotes:` fetches and runs config from somewhere else. Daft will not
    // do that — see the refusal note where it is surfaced. Recorded rather
    // than dropped so the user learns the takeover is not faithful.
    if key == "remotes" {
        unsupported.push(
            "remotes: pulls hook definitions from another repository at run time; daft \
             will not fetch and execute remote configuration"
                .to_string(),
        );
        return Ok(());
    }
    // Keys that manage the other tool's own installation. Meaningless here
    // and harmless to ignore, but named so a reader is not left guessing.
    if matches!(key, "assert_lefthook_installed" | "lefthook" | "rc_local") {
        unsupported.push(format!(
            "{key}: manages the other tool's installation — ignored"
        ));
        return Ok(());
    }

    let def: HookDef = serde_yaml::from_value(value)
        .with_context(|| format!("failed to read the '{key}' entry"))?;

    // git's own `post-merge` becomes daft's qualified key, because the plain
    // one is already daft's merge lifecycle hook.
    if key == GitStage::PostMerge.git_hook_filename() {
        config
            .hooks
            .insert(GitStage::PostMerge.yaml_name().to_string(), def);
        return Ok(());
    }
    if GitStage::from_git_filename(key).is_some() {
        config.hooks.insert(key.to_string(), def);
        return Ok(());
    }
    if let Some(reason) = git_stage::rejection_reason(key) {
        unsupported.push(format!("{key}: {reason}"));
        return Ok(());
    }

    // Anything else is a custom hook — a name the other tool runs on demand
    // rather than on a git event. `tasks:` is daft's word for exactly that,
    // so it lands there and `daft run <name>` invokes it.
    config.tasks.insert(key.to_string(), def);
    Ok(())
}

/// Translate `scripts:` entries into jobs.
///
/// Exposed for the importer, which needs the same conversion. A script is a
/// file under `source_dir` invoked by name, which is a `run:` of that path —
/// there is no separate concept to preserve.
pub fn script_job(name: &str, runner: Option<&str>, source_dir: &str) -> JobDef {
    let path = format!("{source_dir}/{name}");
    JobDef {
        name: Some(name.to_string()),
        run: Some(RunCommand::Simple(match runner {
            Some(runner) => format!("{runner} {path}"),
            None => path,
        })),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_config(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn no_config_is_not_an_error() {
        let tmp = tempdir().unwrap();
        assert!(read(tmp.path()).unwrap().is_none());
        assert!(config_path(tmp.path()).is_none());
    }

    #[test]
    fn every_filename_variant_is_found() {
        for name in CONFIG_NAMES {
            let tmp = tempdir().unwrap();
            write_config(tmp.path(), name, "pre-commit:\n  jobs: []\n");
            assert_eq!(
                config_path(tmp.path()),
                Some(tmp.path().join(name)),
                "{name}"
            );
        }
    }

    #[test]
    fn a_format_daft_cannot_read_says_so_rather_than_reporting_nothing() {
        // Reporting "no incumbent config" in a repository that visibly has
        // one sends the reader looking for a bug in detection.
        let tmp = tempdir().unwrap();
        write_config(tmp.path(), "lefthook.toml", "[pre-commit]\n");
        let err = read(tmp.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("lefthook.toml"), "{msg}");
        assert!(msg.contains("only YAML"), "{msg}");
    }

    #[test]
    fn hook_entries_become_daft_hooks() {
        let tmp = tempdir().unwrap();
        write_config(
            tmp.path(),
            "lefthook.yml",
            r#"
pre-commit:
  parallel: true
  jobs:
    - name: lint
      run: eslint {staged_files}
      glob: "*.js"
pre-push:
  jobs:
    - name: test
      run: npm test
"#,
        );
        let spec = read(tmp.path()).unwrap().unwrap();
        assert_eq!(spec.tool, "lefthook");
        let pre_commit = &spec.config.hooks["pre-commit"];
        assert_eq!(pre_commit.parallel, Some(true));
        assert_eq!(pre_commit.jobs.as_ref().unwrap().len(), 1);
        assert!(spec.config.hooks.contains_key("pre-push"));
    }

    #[test]
    fn git_post_merge_is_requalified_on_the_way_in() {
        // The incumbent file uses git's name; daft's key is qualified because
        // the plain one is already daft's own merge hook.
        let tmp = tempdir().unwrap();
        write_config(
            tmp.path(),
            "lefthook.yml",
            "post-merge:\n  jobs:\n    - name: deps\n      run: npm ci\n",
        );
        let spec = read(tmp.path()).unwrap().unwrap();
        assert!(spec.config.hooks.contains_key("git-post-merge"));
        assert!(!spec.config.hooks.contains_key("post-merge"));
    }

    #[test]
    fn a_custom_hook_name_becomes_a_task() {
        // Not a git event, so not a hook — but still something the user
        // invokes by name, which is what `tasks:` is.
        let tmp = tempdir().unwrap();
        write_config(
            tmp.path(),
            "lefthook.yml",
            "deploy:\n  jobs:\n    - name: ship\n      run: make deploy\n",
        );
        let spec = read(tmp.path()).unwrap().unwrap();
        assert!(spec.config.tasks.contains_key("deploy"));
        assert!(spec.config.hooks.is_empty());
    }

    #[test]
    fn remotes_is_refused_and_reported() {
        // Fetching and executing configuration from elsewhere is the one
        // thing a takeover will not reproduce, so it must be loud.
        let tmp = tempdir().unwrap();
        write_config(
            tmp.path(),
            "lefthook.yml",
            "remotes:\n  - git_url: https://example.com/x.git\npre-commit:\n  jobs: []\n",
        );
        let spec = read(tmp.path()).unwrap().unwrap();
        assert!(
            spec.unsupported.iter().any(|u| u.starts_with("remotes:")),
            "{:?}",
            spec.unsupported
        );
        assert!(spec.config.hooks.contains_key("pre-commit"));
    }

    #[test]
    fn min_version_is_reported_rather_than_enforced() {
        // It constrains the tool that wrote the file, and daft is not that
        // tool — enforcing it would fail every takeover of a pinned config.
        let tmp = tempdir().unwrap();
        write_config(
            tmp.path(),
            "lefthook.yml",
            "min_version: 1.5.0\npre-commit:\n  jobs: []\n",
        );
        let spec = read(tmp.path()).unwrap().unwrap();
        assert!(
            spec.unsupported.iter().any(|u| u.contains("min_version")),
            "{:?}",
            spec.unsupported
        );
        assert!(spec.config.hooks.contains_key("pre-commit"));
    }

    #[test]
    fn a_server_side_hook_is_reported_not_silently_dropped() {
        let tmp = tempdir().unwrap();
        write_config(
            tmp.path(),
            "lefthook.yml",
            "pre-receive:\n  jobs: []\npre-commit:\n  jobs: []\n",
        );
        let spec = read(tmp.path()).unwrap().unwrap();
        assert!(
            spec.unsupported
                .iter()
                .any(|u| u.starts_with("pre-receive:")),
            "{:?}",
            spec.unsupported
        );
        assert!(!spec.config.hooks.contains_key("pre-receive"));
    }

    #[test]
    fn top_level_settings_carry_across() {
        let tmp = tempdir().unwrap();
        write_config(
            tmp.path(),
            "lefthook.yml",
            "colors: false\nno_tty: true\nrc: ~/.hookrc\nsource_dir: .hooks\n",
        );
        let spec = read(tmp.path()).unwrap().unwrap();
        assert_eq!(spec.config.colors, Some(false));
        assert_eq!(spec.config.no_tty, Some(true));
        assert_eq!(spec.config.rc.as_deref(), Some("~/.hookrc"));
        assert_eq!(spec.config.source_dir.as_deref(), Some(".hooks"));
    }

    #[test]
    fn the_local_overlay_wins_per_key() {
        let tmp = tempdir().unwrap();
        write_config(
            tmp.path(),
            "lefthook.yml",
            "pre-commit:\n  parallel: false\n  jobs:\n    - name: a\n      run: \"true\"\n",
        );
        write_config(
            tmp.path(),
            "lefthook-local.yml",
            "pre-commit:\n  parallel: true\n",
        );
        let spec = read(tmp.path()).unwrap().unwrap();
        let hook = &spec.config.hooks["pre-commit"];
        assert_eq!(hook.parallel, Some(true), "the overlay's value wins");
        assert_eq!(
            hook.jobs.as_ref().unwrap().len(),
            1,
            "and the rest survives"
        );
    }

    #[test]
    fn extends_is_reported_rather_than_half_followed() {
        let tmp = tempdir().unwrap();
        write_config(
            tmp.path(),
            "lefthook.yml",
            "extends:\n  - ./shared.yml\npre-commit:\n  jobs: []\n",
        );
        let spec = read(tmp.path()).unwrap().unwrap();
        assert!(
            spec.unsupported.iter().any(|u| u.starts_with("extends:")),
            "{:?}",
            spec.unsupported
        );
    }

    #[test]
    fn script_jobs_translate_to_a_run_of_their_path() {
        let job = script_job("check.sh", Some("bash"), ".lefthook");
        assert_eq!(job.name.as_deref(), Some("check.sh"));
        assert_eq!(
            job.run,
            Some(RunCommand::Simple("bash .lefthook/check.sh".into()))
        );
        let bare = script_job("check.sh", None, ".lefthook");
        assert_eq!(
            bare.run,
            Some(RunCommand::Simple(".lefthook/check.sh".into()))
        );
    }
}
