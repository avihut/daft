//! Pure merge semantics for YAML hooks configuration.
//!
//! Field-level merge of two `YamlConfig`s (and their nested `HookDef` /
//! `LogConfig`), with the overlay taking precedence over the base. These
//! functions are pure — no filesystem, no git, no I/O — so the loader, the
//! `daft file merge` command, and cross-worktree visitor propagation can all
//! share one definition of "what merging two configs means".

use crate::hooks::yaml_config::{HookDef, JobDef, LogConfig, YamlConfig};

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
        relations,
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
    if relations.is_some() {
        merged.relations = relations;
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

// ── Three-way merge ─────────────────────────────────────────────────────────

/// Result of a three-way merge: the merged config plus key-level reports.
///
/// `conflicts` lists key paths where ours and theirs both changed away from
/// the base in different ways. For those keys `merged` retains OURS purely so
/// the struct is total — **callers must not write `merged` blindly while
/// `conflicts` is non-empty**; resolving (prompt, abort, explicit source-wins)
/// is the caller's job.
#[derive(Debug, Default)]
pub struct Merge3Outcome {
    pub merged: YamlConfig,
    /// Key paths both sides changed differently, e.g.
    /// `hooks.worktree-post-create.jobs.setup`.
    pub conflicts: Vec<String>,
    /// Key paths where theirs' change was adopted (ours was unchanged from
    /// the base) — the material for user-facing "adopted N key(s)" messages.
    pub took_from_theirs: Vec<String>,
}

#[derive(Default)]
struct Tally {
    conflicts: Vec<String>,
    took: Vec<String>,
}

/// The four-way rule applied at every granularity:
/// agreement → ours; only-theirs-changed → theirs (recorded); only-ours-changed
/// → ours; both-changed-differently → conflict (recorded), keep ours.
fn pick3<T: PartialEq + Clone>(key: &str, base: &T, ours: &T, theirs: &T, tally: &mut Tally) -> T {
    if ours == theirs {
        return ours.clone();
    }
    if theirs == base {
        return ours.clone();
    }
    if ours == base {
        tally.took.push(key.to_string());
        return theirs.clone();
    }
    tally.conflicts.push(key.to_string());
    ours.clone()
}

/// Three-way merge of two configs against their common base (the seed
/// recorded when the source worktree's copy was written).
///
/// Unlike the two-way [`merge_configs`] — which must guess and lets the
/// overlay win every disagreement — the base disambiguates *who changed
/// what*: a stale copy (theirs == base) contributes nothing, a one-sided
/// change flows through, and a two-sided change is reported as a conflict
/// instead of being silently resolved.
pub fn merge3(base: &YamlConfig, ours: &YamlConfig, theirs: &YamlConfig) -> Merge3Outcome {
    let mut tally = Tally::default();

    // Destructure all three WITHOUT a `..` rest pattern (same compile guard
    // as `merge_configs`): a new `YamlConfig` field fails to compile here
    // until it is explicitly handled.
    let YamlConfig {
        min_version: b_min_version,
        colors: b_colors,
        no_tty: b_no_tty,
        rc: b_rc,
        output: b_output,
        extends: b_extends,
        source_dir: b_source_dir,
        source_dir_local: b_source_dir_local,
        layout: b_layout,
        shared: b_shared,
        log: b_log,
        relations: b_relations,
        hooks: b_hooks,
    } = base;
    let YamlConfig {
        min_version: o_min_version,
        colors: o_colors,
        no_tty: o_no_tty,
        rc: o_rc,
        output: o_output,
        extends: o_extends,
        source_dir: o_source_dir,
        source_dir_local: o_source_dir_local,
        layout: o_layout,
        shared: o_shared,
        log: o_log,
        relations: o_relations,
        hooks: o_hooks,
    } = ours;
    let YamlConfig {
        min_version: t_min_version,
        colors: t_colors,
        no_tty: t_no_tty,
        rc: t_rc,
        output: t_output,
        extends: t_extends,
        source_dir: t_source_dir,
        source_dir_local: t_source_dir_local,
        layout: t_layout,
        shared: t_shared,
        log: t_log,
        relations: t_relations,
        hooks: t_hooks,
    } = theirs;

    let merged = YamlConfig {
        min_version: pick3(
            "min_version",
            b_min_version,
            o_min_version,
            t_min_version,
            &mut tally,
        ),
        colors: pick3("colors", b_colors, o_colors, t_colors, &mut tally),
        no_tty: pick3("no_tty", b_no_tty, o_no_tty, t_no_tty, &mut tally),
        rc: pick3("rc", b_rc, o_rc, t_rc, &mut tally),
        output: pick3("output", b_output, o_output, t_output, &mut tally),
        extends: pick3("extends", b_extends, o_extends, t_extends, &mut tally),
        source_dir: pick3(
            "source_dir",
            b_source_dir,
            o_source_dir,
            t_source_dir,
            &mut tally,
        ),
        source_dir_local: pick3(
            "source_dir_local",
            b_source_dir_local,
            o_source_dir_local,
            t_source_dir_local,
            &mut tally,
        ),
        layout: pick3("layout", b_layout, o_layout, t_layout, &mut tally),
        shared: pick3("shared", b_shared, o_shared, t_shared, &mut tally),
        log: merge3_log(b_log, o_log, t_log, &mut tally),
        relations: pick3(
            "relations",
            b_relations,
            o_relations,
            t_relations,
            &mut tally,
        ),
        hooks: merge3_hooks(b_hooks, o_hooks, t_hooks, &mut tally),
    };

    Merge3Outcome {
        merged,
        conflicts: tally.conflicts,
        took_from_theirs: tally.took,
    }
}

/// Per-field three-way merge of the `log:` section. `log: {}` and a missing
/// section are semantically identical (`#[serde(default)]`), so all sides
/// are normalized through `unwrap_or_default()` and an all-default result
/// collapses back to `None` (no `log: {}` litter in serialized output).
fn merge3_log(
    base: &Option<LogConfig>,
    ours: &Option<LogConfig>,
    theirs: &Option<LogConfig>,
    tally: &mut Tally,
) -> Option<LogConfig> {
    if base.is_none() && ours.is_none() && theirs.is_none() {
        return None;
    }
    let b = base.clone().unwrap_or_default();
    let o = ours.clone().unwrap_or_default();
    let t = theirs.clone().unwrap_or_default();

    // No-`..` destructure: a new LogConfig field must be handled here.
    let LogConfig {
        retention: b_retention,
        max_log_size: b_max_log_size,
        max_total_size: b_max_total_size,
        keep_last: b_keep_last,
        stale_running_after: b_stale_running_after,
        sampling_every_nth: b_sampling_every_nth,
    } = b;

    let merged = LogConfig {
        retention: pick3(
            "log.retention",
            &b_retention,
            &o.retention,
            &t.retention,
            tally,
        ),
        max_log_size: pick3(
            "log.max_log_size",
            &b_max_log_size,
            &o.max_log_size,
            &t.max_log_size,
            tally,
        ),
        max_total_size: pick3(
            "log.max_total_size",
            &b_max_total_size,
            &o.max_total_size,
            &t.max_total_size,
            tally,
        ),
        keep_last: pick3(
            "log.keep_last",
            &b_keep_last,
            &o.keep_last,
            &t.keep_last,
            tally,
        ),
        stale_running_after: pick3(
            "log.stale_running_after",
            &b_stale_running_after,
            &o.stale_running_after,
            &t.stale_running_after,
            tally,
        ),
        sampling_every_nth: pick3(
            "log.sampling_every_nth",
            &b_sampling_every_nth,
            &o.sampling_every_nth,
            &t.sampling_every_nth,
            tally,
        ),
    };

    if merged == LogConfig::default() {
        None
    } else {
        Some(merged)
    }
}

fn merge3_hooks(
    base: &std::collections::HashMap<String, HookDef>,
    ours: &std::collections::HashMap<String, HookDef>,
    theirs: &std::collections::HashMap<String, HookDef>,
    tally: &mut Tally,
) -> std::collections::HashMap<String, HookDef> {
    use std::collections::BTreeSet;

    let mut merged = std::collections::HashMap::new();
    // Sorted key union for deterministic conflict/took ordering.
    let keys: BTreeSet<&String> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();

    for name in keys {
        let key = format!("hooks.{name}");
        let b = base.get(name);
        let o = ours.get(name);
        let t = theirs.get(name);

        let resolved: Option<HookDef> = if o == t || t == b {
            // Agreement, or theirs unchanged from base: ours stands.
            o.cloned()
        } else {
            match (o, t) {
                // Both present and theirs changed: recurse so conflicts and
                // adoptions are reported at field/job granularity (the
                // announce messages depend on it), even when ours is
                // untouched.
                (Some(o), Some(t)) => Some(merge3_hook_defs(
                    &key,
                    &b.cloned().unwrap_or_default(),
                    o,
                    t,
                    tally,
                )),
                // Theirs added a whole hook (base and ours lack it).
                (None, Some(t)) if o == b => {
                    tally.took.push(key.clone());
                    Some(t.clone())
                }
                // Ours deleted it, theirs modified it: conflict; ours'
                // deletion is kept.
                (None, Some(_)) => {
                    tally.conflicts.push(key.clone());
                    None
                }
                // Theirs deleted a hook ours left pristine: adopt the
                // deletion.
                (Some(_), None) if o == b => {
                    tally.took.push(key.clone());
                    None
                }
                // Ours modified it, theirs deleted it: conflict; keep ours.
                (Some(o), None) => {
                    tally.conflicts.push(key.clone());
                    Some(o.clone())
                }
                // `o == t` above covers (None, None).
                (None, None) => None,
            }
        };

        if let Some(def) = resolved {
            merged.insert(name.clone(), def);
        }
    }

    merged
}

fn merge3_hook_defs(
    prefix: &str,
    base: &HookDef,
    ours: &HookDef,
    theirs: &HookDef,
    tally: &mut Tally,
) -> HookDef {
    // No-`..` destructure (compile guard, see `merge_hook_defs`).
    let HookDef {
        background: b_background,
        parallel: b_parallel,
        piped: b_piped,
        follow: b_follow,
        exclude_tags: b_exclude_tags,
        exclude: b_exclude,
        skip: b_skip,
        only: b_only,
        jobs: b_jobs,
        commands: b_commands,
    } = base;

    HookDef {
        background: pick3(
            &format!("{prefix}.background"),
            b_background,
            &ours.background,
            &theirs.background,
            tally,
        ),
        parallel: pick3(
            &format!("{prefix}.parallel"),
            b_parallel,
            &ours.parallel,
            &theirs.parallel,
            tally,
        ),
        piped: pick3(
            &format!("{prefix}.piped"),
            b_piped,
            &ours.piped,
            &theirs.piped,
            tally,
        ),
        follow: pick3(
            &format!("{prefix}.follow"),
            b_follow,
            &ours.follow,
            &theirs.follow,
            tally,
        ),
        exclude_tags: pick3(
            &format!("{prefix}.exclude_tags"),
            b_exclude_tags,
            &ours.exclude_tags,
            &theirs.exclude_tags,
            tally,
        ),
        exclude: pick3(
            &format!("{prefix}.exclude"),
            b_exclude,
            &ours.exclude,
            &theirs.exclude,
            tally,
        ),
        skip: pick3(
            &format!("{prefix}.skip"),
            b_skip,
            &ours.skip,
            &theirs.skip,
            tally,
        ),
        only: pick3(
            &format!("{prefix}.only"),
            b_only,
            &ours.only,
            &theirs.only,
            tally,
        ),
        jobs: merge3_jobs(prefix, b_jobs, &ours.jobs, &theirs.jobs, tally),
        // Legacy `commands` maps merge at whole-map granularity — per-command
        // three-way for a deprecated shape is not worth the surface.
        commands: pick3(
            &format!("{prefix}.commands"),
            b_commands,
            &ours.commands,
            &theirs.commands,
            tally,
        ),
    }
}

/// Three-way job-list merge.
///
/// **Named jobs** resolve per name at whole-`JobDef` granularity — a job is
/// the user's unit of intent; field-level merging inside one job invites
/// nonsensical hybrids. Add/modify/delete all flow through the same
/// four-way rule (absence = `None`), so delete-vs-modify of the same named
/// job is a conflict.
///
/// **Unnamed jobs** have no identity to diff, so the rule is positional and
/// conservative: ours' unnamed jobs are kept as-is, and an unnamed job from
/// theirs is appended only when it is new relative to both the base and
/// ours. Deletions of unnamed jobs never propagate and never conflict.
fn merge3_jobs(
    prefix: &str,
    base: &Option<Vec<JobDef>>,
    ours: &Option<Vec<JobDef>>,
    theirs: &Option<Vec<JobDef>>,
    tally: &mut Tally,
) -> Option<Vec<JobDef>> {
    let b_jobs = base.as_deref().unwrap_or_default();
    let o_jobs = ours.as_deref().unwrap_or_default();
    let t_jobs = theirs.as_deref().unwrap_or_default();

    let named = |jobs: &[JobDef], name: &str| -> Option<JobDef> {
        jobs.iter()
            .find(|j| j.name.as_deref() == Some(name))
            .cloned()
    };

    let mut merged: Vec<JobDef> = Vec::new();

    // Pass 1: ours' jobs in their original order.
    for job in o_jobs {
        match job.name.as_deref() {
            Some(name) => {
                let key = format!("{prefix}.jobs.{name}");
                let b = named(b_jobs, name);
                let o = Some(job.clone());
                let t = named(t_jobs, name);
                if let Some(resolved) = pick3(&key, &b, &o, &t, tally) {
                    merged.push(resolved);
                }
            }
            None => merged.push(job.clone()),
        }
    }

    // Pass 2: named jobs present in theirs but absent from ours.
    for job in t_jobs {
        let Some(name) = job.name.as_deref() else {
            continue;
        };
        if named(o_jobs, name).is_some() {
            continue;
        }
        let key = format!("{prefix}.jobs.{name}");
        let b = named(b_jobs, name);
        let t = Some(job.clone());
        // ours deleted it (or never had it): resolve None vs theirs.
        if let Some(resolved) = pick3(&key, &b, &None, &t, tally) {
            merged.push(resolved);
        }
    }

    // Pass 3: unnamed jobs from theirs that are new relative to base and ours.
    for job in t_jobs {
        if job.name.is_some() {
            continue;
        }
        let in_base = b_jobs.iter().any(|j| j == job);
        let in_ours = o_jobs.iter().any(|j| j == job);
        if !in_base && !in_ours {
            tally.took.push(format!("{prefix}.jobs[unnamed]"));
            merged.push(job.clone());
        }
    }

    if merged.is_empty() && ours.is_none() && theirs.is_none() {
        None
    } else if merged.is_empty() {
        // All jobs resolved away (e.g. theirs deleted everything ours had
        // left pristine). Preserve "had a jobs key" shape only when a side
        // still has one.
        if ours.is_some() || theirs.is_some() {
            Some(Vec::new())
        } else {
            None
        }
    } else {
        Some(merged)
    }
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
            relations: Some(vec![crate::catalog::relations::RelationEntry {
                url: "git@example.com:org/client.git".to_string(),
                name: Some("client".to_string()),
                kind: Some("consumer".to_string()),
            }]),
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

    // ── merge3 ───────────────────────────────────────────────────────

    fn cfg_with_job(hook: &str, name: &str, run: &str) -> YamlConfig {
        let mut hooks = HashMap::new();
        hooks.insert(
            hook.to_string(),
            HookDef {
                jobs: Some(vec![JobDef {
                    name: Some(name.to_string()),
                    run: Some(RunCommand::Simple(run.to_string())),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        );
        YamlConfig {
            hooks,
            ..Default::default()
        }
    }

    fn job_run(cfg: &YamlConfig, hook: &str, name: &str) -> Option<String> {
        cfg.hooks.get(hook)?.jobs.as_ref()?.iter().find_map(|j| {
            (j.name.as_deref() == Some(name))
                .then(|| j.run.as_ref()?.resolve_for_current_os())
                .flatten()
        })
    }

    #[test]
    fn merge3_stale_theirs_contributes_nothing() {
        // THE regression scenario from issue #628: base == theirs == A (the
        // worktree's pristine seeded copy), ours == B (the evolved target).
        // The merge must be exactly B — no conflicts, nothing adopted.
        let base = cfg_with_job("worktree-post-create", "setup", "echo setup-v1");
        let theirs = base.clone();
        let ours = cfg_with_job("worktree-post-create", "setup", "echo setup-v2");

        let out = merge3(&base, &ours, &theirs);
        assert_eq!(out.merged, ours);
        assert!(out.conflicts.is_empty(), "{:?}", out.conflicts);
        assert!(
            out.took_from_theirs.is_empty(),
            "{:?}",
            out.took_from_theirs
        );
    }

    #[test]
    fn merge3_scalar_matrix() {
        let base = YamlConfig {
            shared: Some(vec![".env".into()]),
            ..Default::default()
        };

        // Ours-only change.
        let ours = YamlConfig {
            shared: Some(vec![".env".into(), ".envrc".into()]),
            ..Default::default()
        };
        let out = merge3(&base, &ours, &base.clone());
        assert_eq!(out.merged.shared, ours.shared);
        assert!(out.conflicts.is_empty() && out.took_from_theirs.is_empty());

        // Theirs-only change.
        let theirs = YamlConfig {
            shared: Some(vec![".env".into(), ".tool-versions".into()]),
            ..Default::default()
        };
        let out = merge3(&base, &base.clone(), &theirs);
        assert_eq!(out.merged.shared, theirs.shared);
        assert_eq!(out.took_from_theirs, vec!["shared".to_string()]);

        // Same change on both sides: agreement, not a conflict.
        let out = merge3(&base, &theirs.clone(), &theirs);
        assert_eq!(out.merged.shared, theirs.shared);
        assert!(out.conflicts.is_empty() && out.took_from_theirs.is_empty());

        // Different changes on both sides: conflict, ours retained.
        let out = merge3(&base, &ours, &theirs);
        assert_eq!(out.conflicts, vec!["shared".to_string()]);
        assert_eq!(out.merged.shared, ours.shared, "conflicted key keeps ours");
    }

    #[test]
    fn merge3_named_job_conflict_has_exact_key() {
        let base = cfg_with_job("worktree-post-create", "setup", "echo setup-v1");
        let ours = cfg_with_job("worktree-post-create", "setup", "echo setup-v2");
        let theirs = cfg_with_job("worktree-post-create", "setup", "echo setup-feat");

        let out = merge3(&base, &ours, &theirs);
        assert_eq!(
            out.conflicts,
            vec!["hooks.worktree-post-create.jobs.setup".to_string()]
        );
        assert_eq!(
            job_run(&out.merged, "worktree-post-create", "setup").as_deref(),
            Some("echo setup-v2"),
            "conflicted job keeps ours"
        );
    }

    #[test]
    fn merge3_adopts_added_named_job_and_keeps_ours_changes() {
        // Theirs added a job; ours independently changed another. Both flow
        // through, nothing conflicts.
        let base = cfg_with_job("worktree-post-create", "setup", "echo setup-v1");
        let ours = cfg_with_job("worktree-post-create", "setup", "echo setup-v2");
        let mut theirs = base.clone();
        theirs
            .hooks
            .get_mut("worktree-post-create")
            .unwrap()
            .jobs
            .as_mut()
            .unwrap()
            .push(JobDef {
                name: Some("extra".to_string()),
                run: Some(RunCommand::Simple("echo extra-job".to_string())),
                ..Default::default()
            });

        let out = merge3(&base, &ours, &theirs);
        assert!(out.conflicts.is_empty(), "{:?}", out.conflicts);
        assert_eq!(
            out.took_from_theirs,
            vec!["hooks.worktree-post-create.jobs.extra".to_string()]
        );
        assert_eq!(
            job_run(&out.merged, "worktree-post-create", "setup").as_deref(),
            Some("echo setup-v2")
        );
        assert_eq!(
            job_run(&out.merged, "worktree-post-create", "extra").as_deref(),
            Some("echo extra-job")
        );
    }

    #[test]
    fn merge3_adopts_theirs_deletion_of_pristine_job() {
        // Theirs deleted a named job ours never touched: deletion flows.
        let mut base = cfg_with_job("worktree-post-create", "setup", "echo setup-v1");
        base.hooks
            .get_mut("worktree-post-create")
            .unwrap()
            .jobs
            .as_mut()
            .unwrap()
            .push(JobDef {
                name: Some("old".to_string()),
                run: Some(RunCommand::Simple("echo old".to_string())),
                ..Default::default()
            });
        let ours = base.clone();
        let theirs = cfg_with_job("worktree-post-create", "setup", "echo setup-v1");

        let out = merge3(&base, &ours, &theirs);
        assert!(out.conflicts.is_empty());
        assert!(
            job_run(&out.merged, "worktree-post-create", "old").is_none(),
            "deletion from theirs must apply to a pristine job"
        );
        assert_eq!(
            out.took_from_theirs,
            vec!["hooks.worktree-post-create.jobs.old".to_string()]
        );
    }

    #[test]
    fn merge3_delete_vs_modify_is_a_conflict() {
        // Ours deleted the whole hook; theirs modified it: conflict, ours'
        // deletion is retained.
        let base = cfg_with_job("post-clone", "fetch", "echo v1");
        let ours = YamlConfig::default();
        let theirs = cfg_with_job("post-clone", "fetch", "echo v2");

        let out = merge3(&base, &ours, &theirs);
        assert_eq!(out.conflicts, vec!["hooks.post-clone".to_string()]);
        assert!(out.merged.hooks.is_empty(), "ours' deletion is kept");
    }

    #[test]
    fn merge3_unnamed_jobs_append_only_when_new() {
        let mk = |runs: &[&str]| -> Option<Vec<JobDef>> {
            Some(
                runs.iter()
                    .map(|r| JobDef {
                        run: Some(RunCommand::Simple(r.to_string())),
                        ..Default::default()
                    })
                    .collect(),
            )
        };
        let mut tally = Tally::default();
        let merged = merge3_jobs(
            "hooks.h",
            &mk(&["echo a"]),
            &mk(&["echo a"]),
            &mk(&["echo a", "echo new"]),
            &mut tally,
        )
        .unwrap();
        assert_eq!(merged.len(), 2, "new unnamed job from theirs appended");
        assert_eq!(tally.took, vec!["hooks.h.jobs[unnamed]".to_string()]);

        // An unnamed job already in the base is not re-appended.
        let mut tally = Tally::default();
        let merged = merge3_jobs(
            "hooks.h",
            &mk(&["echo a", "echo old"]),
            &mk(&["echo a"]),
            &mk(&["echo a", "echo old"]),
            &mut tally,
        )
        .unwrap();
        assert_eq!(
            merged.len(),
            1,
            "base unnamed job dropped by ours stays dropped"
        );
        assert!(tally.took.is_empty());
    }

    #[test]
    fn merge3_log_per_field_and_normalization() {
        let base = YamlConfig {
            log: Some(LogConfig {
                retention: Some("7d".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let ours = YamlConfig {
            log: Some(LogConfig {
                retention: Some("7d".into()),
                keep_last: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        };
        let theirs = YamlConfig {
            log: Some(LogConfig {
                retention: Some("14d".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let out = merge3(&base, &ours, &theirs);
        let log = out.merged.log.unwrap();
        assert_eq!(
            log.retention.as_deref(),
            Some("14d"),
            "theirs' field change"
        );
        assert_eq!(log.keep_last, Some(3), "ours' field change");
        assert!(out.conflicts.is_empty());

        // All three None → stays None; `log: {}` ≡ None normalizes away.
        let out = merge3(
            &YamlConfig::default(),
            &YamlConfig::default(),
            &YamlConfig {
                log: Some(LogConfig::default()),
                ..Default::default()
            },
        );
        assert!(out.merged.log.is_none(), "log: {{}} must normalize to None");
    }

    #[test]
    fn merge3_full_overlay_preserved_against_empty_base() {
        // Mirror of merge_configs_preserves_every_overlay_field: with an
        // empty base and pristine ours, a fully-populated theirs must come
        // through intact (every field is a theirs-only change).
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
            relations: Some(vec![crate::catalog::relations::RelationEntry {
                url: "git@example.com:org/client.git".to_string(),
                name: Some("client".to_string()),
                kind: Some("consumer".to_string()),
            }]),
            hooks,
        };

        let out = merge3(&YamlConfig::default(), &YamlConfig::default(), &full);
        assert_eq!(
            out.merged, full,
            "a fully-populated theirs against an empty base/ours must be preserved"
        );
        assert!(out.conflicts.is_empty());
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
