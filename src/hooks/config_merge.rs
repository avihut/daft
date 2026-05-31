//! Pure merge semantics for YAML hooks configuration.
//!
//! Field-level merge of two `YamlConfig`s (and their nested `HookDef` /
//! `LogConfig`), with the overlay taking precedence over the base. These
//! functions are pure — no filesystem, no git, no I/O — so the loader, the
//! `daft file merge` command, and cross-worktree visitor propagation can all
//! share one definition of "what merging two configs means".

use crate::hooks::yaml_config::{HookDef, LogConfig, YamlConfig};

/// Merge two configs, with `overlay` taking precedence over `base`.
pub fn merge_configs(base: YamlConfig, overlay: YamlConfig) -> YamlConfig {
    // Destructure the overlay WITHOUT a `..` rest pattern so that adding a new
    // field to `YamlConfig` fails to compile here until it is explicitly merged.
    // This guards against the silent-drop bug class: `shared` and `extends` were
    // previously omitted from this function, so they were discarded on every
    // merge — corrupting `daft merge` / `daft file merge` output and (via the
    // checkout-time propagation that used to route through here) destroying the
    // user's untracked `daft.yml`.
    let YamlConfig {
        min_version,
        colors,
        no_tty,
        rc,
        output,
        extends,
        source_dir,
        source_dir_local,
        layout,
        shared,
        log,
        hooks,
    } = overlay;

    let mut merged = base;

    // Scalar / list fields: overlay wins if set.
    if min_version.is_some() {
        merged.min_version = min_version;
    }
    if colors.is_some() {
        merged.colors = colors;
    }
    if no_tty.is_some() {
        merged.no_tty = no_tty;
    }
    if rc.is_some() {
        merged.rc = rc;
    }
    if output.is_some() {
        merged.output = output;
    }
    if extends.is_some() {
        merged.extends = extends;
    }
    if source_dir.is_some() {
        merged.source_dir = source_dir;
    }
    if source_dir_local.is_some() {
        merged.source_dir_local = source_dir_local;
    }
    if layout.is_some() {
        merged.layout = layout;
    }
    if shared.is_some() {
        merged.shared = shared;
    }

    // Merge log config (field-level merge)
    merged.log = match (merged.log, log) {
        (Some(b), Some(o)) => Some(merge_log_configs(o, b)),
        (b, o) => o.or(b),
    };

    // Hooks: merge each hook definition
    for (name, overlay_hook) in hooks {
        if let Some(base_hook) = merged.hooks.remove(&name) {
            merged
                .hooks
                .insert(name, merge_hook_defs(base_hook, overlay_hook));
        } else {
            merged.hooks.insert(name, overlay_hook);
        }
    }

    merged
}

/// Merge two log configs, with `o` (overlay) taking precedence over `b` (base).
///
/// Field-level merge: each field uses the overlay value if set, otherwise the
/// base value.
pub fn merge_log_configs(o: LogConfig, b: LogConfig) -> LogConfig {
    LogConfig {
        retention: o.retention.or(b.retention),
        max_log_size: o.max_log_size.or(b.max_log_size),
        max_total_size: o.max_total_size.or(b.max_total_size),
        keep_last: o.keep_last.or(b.keep_last),
        stale_running_after: o.stale_running_after.or(b.stale_running_after),
        sampling_every_nth: o.sampling_every_nth.or(b.sampling_every_nth),
    }
}

/// Merge two hook definitions, with `overlay` taking precedence.
///
/// Named jobs merge by name (overlay replaces base with same name).
/// Unnamed jobs from overlay are appended.
pub fn merge_hook_defs(base: HookDef, overlay: HookDef) -> HookDef {
    // Destructure WITHOUT a `..` rest pattern (see `merge_configs`): adding a
    // field to `HookDef` must fail to compile here until it is handled.
    // `background` was previously dropped exactly the way `shared`/`extends`
    // were in `merge_configs`.
    let HookDef {
        background,
        parallel,
        piped,
        follow,
        exclude_tags,
        exclude,
        skip,
        only,
        jobs,
        commands,
    } = overlay;

    let mut merged = base;

    // Scalar fields: overlay wins if set
    if background.is_some() {
        merged.background = background;
    }
    if parallel.is_some() {
        merged.parallel = parallel;
    }
    if piped.is_some() {
        merged.piped = piped;
    }
    if follow.is_some() {
        merged.follow = follow;
    }
    if exclude_tags.is_some() {
        merged.exclude_tags = exclude_tags;
    }
    if exclude.is_some() {
        merged.exclude = exclude;
    }
    if skip.is_some() {
        merged.skip = skip;
    }
    if only.is_some() {
        merged.only = only;
    }

    // Jobs: merge named jobs by name, append unnamed
    if let Some(overlay_jobs) = jobs {
        let mut base_jobs = merged.jobs.unwrap_or_default();
        for overlay_job in overlay_jobs {
            if let Some(ref name) = overlay_job.name {
                // Replace existing job with same name
                if let Some(pos) = base_jobs
                    .iter()
                    .position(|j| j.name.as_deref() == Some(name))
                {
                    base_jobs[pos] = overlay_job;
                } else {
                    base_jobs.push(overlay_job);
                }
            } else {
                base_jobs.push(overlay_job);
            }
        }
        merged.jobs = Some(base_jobs);
    }

    // Commands: overlay replaces entirely if set
    if commands.is_some() {
        merged.commands = commands;
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::yaml_config::{JobDef, RunCommand};
    use std::collections::HashMap;

    #[test]
    fn test_merge_configs_scalar_override() {
        let base = YamlConfig {
            min_version: Some("1.0.0".to_string()),
            colors: Some(true),
            ..Default::default()
        };
        let overlay = YamlConfig {
            min_version: Some("2.0.0".to_string()),
            ..Default::default()
        };
        let merged = merge_configs(base, overlay);
        assert_eq!(merged.min_version.as_deref(), Some("2.0.0"));
        assert_eq!(merged.colors, Some(true)); // base preserved
    }

    #[test]
    fn test_merge_hook_defs_named_jobs() {
        let base = HookDef {
            jobs: Some(vec![
                JobDef {
                    name: Some("lint".to_string()),
                    run: Some(RunCommand::Simple("eslint .".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("format".to_string()),
                    run: Some(RunCommand::Simple("prettier --check .".to_string())),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let overlay = HookDef {
            jobs: Some(vec![JobDef {
                name: Some("lint".to_string()),
                run: Some(RunCommand::Simple("cargo clippy".to_string())),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let merged = merge_hook_defs(base, overlay);
        let jobs = merged.jobs.unwrap();
        assert_eq!(jobs.len(), 2);
        // lint should be overridden
        assert_eq!(
            jobs[0]
                .run
                .as_ref()
                .and_then(|r| r.resolve_for_current_os()),
            Some("cargo clippy".to_string())
        );
        // format should be preserved
        assert_eq!(
            jobs[1]
                .run
                .as_ref()
                .and_then(|r| r.resolve_for_current_os()),
            Some("prettier --check .".to_string())
        );
    }

    #[test]
    fn test_merge_hook_defs_unnamed_appended() {
        let base = HookDef {
            jobs: Some(vec![JobDef {
                run: Some(RunCommand::Simple("echo base".to_string())),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let overlay = HookDef {
            jobs: Some(vec![JobDef {
                run: Some(RunCommand::Simple("echo overlay".to_string())),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let merged = merge_hook_defs(base, overlay);
        let jobs = merged.jobs.unwrap();
        assert_eq!(jobs.len(), 2);
    }

    // ── Tests for log config merging ─────────────────────────────────

    #[test]
    fn test_merge_log_config() {
        let base = YamlConfig {
            log: Some(crate::executor::LogConfig {
                retention: Some("7d".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let overlay = YamlConfig {
            log: Some(crate::executor::LogConfig {
                retention: Some("14d".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let merged = merge_configs(base, overlay);
        assert_eq!(merged.log.unwrap().retention, Some("14d".to_string()));
    }

    #[test]
    fn merge_configs_preserves_every_overlay_field() {
        // Regression guard for the silent-drop bug class: merging a
        // fully-populated overlay onto an empty base must reproduce the overlay
        // exactly. Before the fix, `shared` and `extends` were dropped here
        // (and `background` in `merge_hook_defs`), silently destroying user
        // config on `daft merge` / `daft file merge` and on checkout
        // propagation. The no-`..` destructure plus this assertion make a future
        // field omission fail to compile or fail this test.
        let mut hooks = HashMap::new();
        hooks.insert(
            "worktree-post-create".to_string(),
            HookDef {
                background: Some(true),
                parallel: Some(true),
                jobs: Some(vec![JobDef {
                    name: Some("example".to_string()),
                    run: Some(RunCommand::Simple("echo hi".to_string())),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        );
        let full = YamlConfig {
            min_version: Some("1.0.0".to_string()),
            colors: Some(false),
            no_tty: Some(true),
            rc: Some(".bashrc".to_string()),
            output: Some(crate::hooks::yaml_config::OutputSetting::Disabled(false)),
            extends: Some(vec!["base.yml".to_string()]),
            source_dir: Some(".daft".to_string()),
            source_dir_local: Some(".daft-local".to_string()),
            layout: Some("contained".to_string()),
            shared: Some(vec![".env".to_string()]),
            log: Some(LogConfig {
                retention: Some("7d".to_string()),
                ..Default::default()
            }),
            hooks,
        };

        let merged = merge_configs(YamlConfig::default(), full.clone());
        assert_eq!(
            merged, full,
            "merging a full overlay onto an empty base must preserve every field"
        );
    }

    #[test]
    fn merged_config_serializes_sparsely_without_null_litter() {
        // A merged/serialized config must be sparse: unset Option fields are
        // omitted, never emitted as `field: null`. Regression for the
        // visitor-config null-litter that `daft file merge` / `daft merge`
        // wrote into user daft.yml files.
        let mut hooks = HashMap::new();
        hooks.insert(
            "worktree-post-create".to_string(),
            HookDef {
                jobs: Some(vec![JobDef {
                    name: Some("example".to_string()),
                    run: Some(RunCommand::Simple("echo hi".to_string())),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        );
        let cfg = YamlConfig {
            shared: Some(vec![".env".to_string()]),
            hooks,
            ..Default::default()
        };

        let yaml = serde_yaml::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("null"),
            "serialized config must not contain null litter:\n{yaml}"
        );
        assert!(
            !yaml.contains("min_version"),
            "unset scalar fields must be omitted entirely:\n{yaml}"
        );
        assert!(yaml.contains("shared:"), "set fields must survive:\n{yaml}");
        assert!(yaml.contains("example"), "nested set fields must survive");
    }

    #[test]
    fn merge_hook_defs_preserves_background() {
        // `background` was the third silent-drop site (alongside
        // `shared`/`extends` in `merge_configs`). Merging a hook that sets only
        // `background` onto an empty base must carry it through.
        let overlay = HookDef {
            background: Some(true),
            ..Default::default()
        };
        let merged = merge_hook_defs(HookDef::default(), overlay);
        assert_eq!(merged.background, Some(true));
    }

    #[test]
    fn merge_prefers_override_for_new_fields() {
        let base = LogConfig {
            retention: Some("7d".into()),
            max_log_size: Some("10MB".into()),
            max_total_size: Some("500MB".into()),
            keep_last: Some(3),
            stale_running_after: Some("24h".into()),
            sampling_every_nth: Some(5),
        };
        let override_cfg = LogConfig {
            retention: Some("14d".into()),
            max_log_size: Some("20MB".into()),
            max_total_size: None,        // base wins for this one
            keep_last: None,             // base wins
            stale_running_after: None,   // base wins
            sampling_every_nth: Some(2), // override wins
        };
        let merged = merge_log_configs(override_cfg, base);
        assert_eq!(merged.retention.as_deref(), Some("14d"));
        assert_eq!(merged.max_log_size.as_deref(), Some("20MB"));
        assert_eq!(merged.max_total_size.as_deref(), Some("500MB"));
        assert_eq!(merged.keep_last, Some(3));
        assert_eq!(merged.stale_running_after.as_deref(), Some("24h"));
    }
}
