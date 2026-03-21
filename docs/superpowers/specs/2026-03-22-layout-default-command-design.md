# `daft layout default` Command

## Overview

Add a `default` subcommand to `daft layout` for viewing and setting the global
default worktree layout. Currently the only ways to change the default are the
first-clone prompt or manually editing `config.toml`.

## CLI Surface

Three modes of operation:

### Show (no arguments)

```
$ daft layout default
sibling {{ repo }}.{{ branch | sanitize }} (default)
```

When a global config override exists:

```
$ daft layout default
contained {{ repo_path }}/{{ branch }} (global config)
```

Output format: `<name> <template> (<source>)` with the same styling and source
labels as `daft layout show` (bold name, highlighted template, dim source).

### Set

```
$ daft layout default contained
Default layout set to 'contained'.
```

Accepts any built-in name, custom layout name from config, or inline template
string. Unknown names are treated as inline templates (same behavior as
`--layout` on clone). Empty strings are rejected with an error.

Writes `defaults.layout` to `~/.config/daft/config.toml` (or
`$DAFT_CONFIG_DIR/config.toml`).

### Reset

```
$ daft layout default --reset
Default layout reset to built-in (sibling).
```

Removes the `defaults.layout` line from config.toml, reverting to the built-in
default (sibling). All other config content is preserved. If the config file or
its parent directory does not exist, this is a no-op (no error). The
confirmation message still prints.

## Implementation

### Files to modify

- `src/commands/layout.rs` -- Add `Default(DefaultArgs)` variant to
  `LayoutCommand` enum. Add `cmd_default()` function.
- `src/core/global_config.rs` -- Add `remove_default_layout()` method.
- Shell completions (`src/commands/completions/`) -- Add `default` to layout
  subcommand lists in bash, zsh, fish, fig.
- `man/daft-layout.1` -- Regenerate via `mise run man:gen`.

No changes needed to `xtask/src/main.rs` (subcommand of already-registered
`daft-layout`) or `src/commands/docs.rs` (clap auto-discovers subcommands from
the enum).

### Clap Args

```rust
#[derive(Args)]
struct DefaultArgs {
    /// Layout name or template to set as the global default
    #[arg(conflicts_with = "reset")]
    layout: Option<String>,

    /// Remove the global default, reverting to built-in (sibling)
    #[arg(long)]
    reset: bool,
}
```

Mutual exclusivity handled by clap's `conflicts_with`.

### `cmd_default` logic

1. If `--reset`: call `GlobalConfig::remove_default_layout()`, print
   confirmation.
2. If `layout` provided: validate non-empty, call
   `GlobalConfig::set_default_layout(name)`, print confirmation.
3. If neither: load config, display current default with name + template +
   source. Use `highlight_template()` for colored output. Source is "global
   config" if set in config.toml, "default" if using the built-in fallback.

### `GlobalConfig::remove_default_layout`

Read config file, remove the `layout = ...` line from the `[defaults]` section,
write back. Only remove lines that appear after `[defaults]` and before the next
`[` section header (section-aware scanning). If the config file does not exist
or has no layout line, no-op.

## Test Scenarios

YAML scenarios in `tests/manual/scenarios/layout/`:

- **default-show.yml** -- `daft layout default` shows built-in default when no
  config; shows config value when set
- **default-set.yml** -- `daft layout default contained` sets the default,
  subsequent clone uses it without `--layout` flag
- **default-reset.yml** -- `daft layout default --reset` removes the override,
  reverts to built-in
- **default-conflict.yml** -- `daft layout default contained --reset` produces
  an error (clap conflict)
