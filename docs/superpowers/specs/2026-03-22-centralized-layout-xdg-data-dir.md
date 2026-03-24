# Centralized Layout: Use XDG Data Directory

## Overview

Change the centralized layout template from `~/worktrees/...` to use the XDG
data directory (`~/.local/share/daft/worktrees/...`). This aligns with the XDG
Base Directory Specification: config files stay in `XDG_CONFIG_HOME`,
application data goes in `XDG_DATA_HOME`.

## Current State

```
centralized template: ~/worktrees/{{ repo }}/{{ branch | sanitize }}
resolves to:          /Users/avihu/worktrees/myrepo/main
```

## New State

```
centralized template: {{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch | sanitize }}
resolves to:          /Users/avihu/.local/share/daft/worktrees/myrepo/main
```

## Implementation

### New function: `daft_data_dir()`

Add to `src/lib.rs`, mirroring `daft_config_dir()`:

- Uses `dirs::data_dir()` + `"daft"` by default
- Add `pub const DATA_DIR_ENV: &str = "DAFT_DATA_DIR"` constant
- Respects `DAFT_DATA_DIR` env override in dev builds — used verbatim with no
  `daft/` suffix appended, matching `DAFT_CONFIG_DIR` behavior
- Error when `dirs::data_dir()` returns None:
  `"Could not determine data directory"`

### New template variable: `{{ daft_data_dir }}`

Add `daft_data_dir` as a case in `resolve_expression()` in
`src/core/layout/template.rs`. It calls `crate::daft_data_dir()` directly (not
via `TemplateContext`) since the value is global, not per-repo.

### Template change

In `src/core/layout/mod.rs`, change the centralized template from:

```
~/worktrees/{{ repo }}/{{ branch | sanitize }}
```

to:

```
{{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch | sanitize }}
```

### Bare inference

`src/core/layout/bare.rs` infers bare=true when a template starts with
`{{ repo_path }}/`. The centralized template starts with `{{ daft_data_dir }}/`,
so bare inference is unaffected (correctly infers non-bare). The existing
`test_home_path_not_bare` test (which uses `~/worktrees/...`) remains valid as a
generic home-relative test. Add a new test for `{{ daft_data_dir }}/...` to
confirm non-bare inference.

### Path resolution

`resolve_path()` in `template.rs` currently handles absolute paths, `~/` paths,
and relative paths. After rendering, `{{ daft_data_dir }}` produces an absolute
path (e.g., `/home/user/.local/share/daft/worktrees/...`), so the existing
absolute-path branch handles it with no changes.

### No migration

The centralized layout hasn't shipped (feature branch only). No migration path
needed.

## Files to Modify

- `src/lib.rs` — add `DATA_DIR_ENV` constant and `daft_data_dir()` function
- `src/core/layout/template.rs` — add `daft_data_dir` case in
  `resolve_expression()`
- `src/core/layout/mod.rs` — change centralized template string
- `src/core/layout/bare.rs` — add test for `{{ daft_data_dir }}/...` non-bare
  inference
- `docs/superpowers/specs/2026-03-20-progressive-adoption-layout-system-design.md`
  — update built-in layout table
- `tests/manual/scenarios/layout/centralized-workflow.yml` — update expected
  paths and cleanup step from `~/worktrees/` to XDG data dir path
