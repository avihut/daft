# `daft layout default` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `daft layout default [<name>] [--reset]` subcommand for viewing
and setting the global default worktree layout.

**Architecture:** New `Default` variant in the existing `LayoutCommand` enum in
`src/commands/layout.rs`. Uses existing `GlobalConfig::set_default_layout()` for
set, adds `remove_default_layout()` for reset. Show mode reuses
`highlight_template()` from the same module.

**Tech Stack:** Rust, clap (Args/Subcommand), existing GlobalConfig and Layout
types.

**Spec:** `docs/superpowers/specs/2026-03-22-layout-default-command-design.md`

---

### Task 1: Write tests (TDD)

**Files:**

- Create: `tests/manual/scenarios/layout/default-show.yml`
- Create: `tests/manual/scenarios/layout/default-set.yml`
- Create: `tests/manual/scenarios/layout/default-reset.yml`
- Create: `tests/manual/scenarios/layout/default-conflict.yml`

- [ ] **Step 1: Write default-show test**

```yaml
name: Layout default shows current default
description: daft layout default shows the global default layout with source.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Show default with no config (built-in)
    run: NO_COLOR=1 daft layout default 2>&1
    expect:
      exit_code: 0
      output_contains:
        - "sibling"
        - "default"

  - name: Set global default to contained
    run: |
      mkdir -p $DAFT_CONFIG_DIR
      cat > $DAFT_CONFIG_DIR/config.toml << 'TOML'
      [defaults]
      layout = "contained"
      TOML
    expect:
      exit_code: 0

  - name: Show default with config override
    run: NO_COLOR=1 daft layout default 2>&1
    expect:
      exit_code: 0
      output_contains:
        - "contained"
        - "global config"
```

- [ ] **Step 2: Write default-set test**

```yaml
name: Layout default set changes global default
description:
  daft layout default <name> sets the default and subsequent clones use it.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Set default to contained
    run: daft layout default contained 2>&1
    expect:
      exit_code: 0
      output_contains:
        - "contained"

  - name: Clone uses the new default
    run: git-worktree-clone $REMOTE_TEST_REPO 2>&1
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/main"

  - name: Verify clone used contained layout
    run: NO_COLOR=1 daft layout show 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "contained"
```

- [ ] **Step 3: Write default-reset test**

```yaml
name: Layout default reset reverts to built-in
description: daft layout default --reset removes the config override.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Set default to contained
    run: daft layout default contained 2>&1
    expect:
      exit_code: 0

  - name: Reset to built-in
    run: daft layout default --reset 2>&1
    expect:
      exit_code: 0
      output_contains:
        - "reset"
        - "sibling"

  - name: Verify default is sibling again
    run: NO_COLOR=1 daft layout default 2>&1
    expect:
      exit_code: 0
      output_contains:
        - "sibling"
        - "default"
```

- [ ] **Step 4: Write default-conflict test**

```yaml
name: Layout default rejects --reset with layout name
description: --reset and layout name are mutually exclusive (clap conflict).

steps:
  - name: Conflict produces error
    run: daft layout default contained --reset 2>&1
    expect:
      exit_code: 2
      output_contains:
        - "cannot be used with"
```

- [ ] **Step 5: Run tests to verify they fail**

```bash
mise run test:manual -- --ci layout:default-show layout:default-set layout:default-reset layout:default-conflict
```

Expected: All fail (subcommand doesn't exist yet).

- [ ] **Step 6: Commit test files**

```bash
git add tests/manual/scenarios/layout/default-show.yml \
        tests/manual/scenarios/layout/default-set.yml \
        tests/manual/scenarios/layout/default-reset.yml \
        tests/manual/scenarios/layout/default-conflict.yml
git commit -m "test(layout): add failing tests for layout default command"
```

---

### Task 2: Add `remove_default_layout` to GlobalConfig

**Files:**

- Modify: `src/core/global_config.rs`

- [ ] **Step 1: Add `remove_default_layout` method**

Add after `set_default_layout`:

```rust
/// Remove the default layout from the config file.
///
/// Reverts to the built-in default (sibling). Only removes lines under
/// `[defaults]` section (section-aware). No-op if config file doesn't
/// exist or has no layout line.
pub fn remove_default_layout() -> Result<()> {
    let path = Self::default_path()?;

    if !path.exists() {
        return Ok(());
    }

    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read config from {}", path.display()))?;

    let mut result = String::new();
    let mut in_defaults = false;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_defaults = trimmed == "[defaults]";
        }
        if in_defaults
            && (trimmed.starts_with("layout = ") || trimmed.starts_with("layout="))
        {
            continue; // Skip this line
        }
        result.push_str(line);
        result.push('\n');
    }

    std::fs::write(&path, result)
        .with_context(|| format!("Failed to write config to {}", path.display()))
}
```

- [ ] **Step 2: Build and verify**

```bash
cargo build
```

- [ ] **Step 3: Commit**

```bash
git add src/core/global_config.rs
git commit -m "feat(config): add remove_default_layout for layout reset"
```

---

### Task 3: Add `Default` subcommand to layout

**Files:**

- Modify: `src/commands/layout.rs`

- [ ] **Step 1: Add DefaultArgs and enum variant**

Add to `LayoutCommand` enum:

```rust
/// View or set the global default layout
Default(DefaultArgs),
```

Add struct:

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

- [ ] **Step 2: Add routing in `run()`**

Update the match in `run()`:

```rust
Some(LayoutCommand::Default(default_args)) => cmd_default(&default_args, &mut output),
```

- [ ] **Step 3: Implement `cmd_default`**

```rust
fn cmd_default(args: &DefaultArgs, output: &mut dyn Output) -> Result<()> {
    if args.reset {
        GlobalConfig::remove_default_layout()?;
        output.result("Default layout reset to built-in (sibling).");
        return Ok(());
    }

    if let Some(ref layout_name) = args.layout {
        if layout_name.is_empty() {
            anyhow::bail!("Layout name cannot be empty.");
        }
        GlobalConfig::set_default_layout(layout_name)?;
        output.result(&format!("Default layout set to '{layout_name}'."));
        return Ok(());
    }

    // Show current default
    let global_config = GlobalConfig::load().unwrap_or_default();
    let use_color = styles::colors_enabled();

    let (layout, source) = match global_config.defaults.layout {
        Some(_) => (global_config.default_layout().unwrap(), "global config"),
        None => (DEFAULT_LAYOUT.to_layout(), "default"),
    };
    let (name, template) = (layout.name, layout.template);

    let template_display = if use_color {
        highlight_template(&template)
    } else {
        template
    };

    output.info(&format!(
        "{} {} {}",
        bold(&name),
        template_display,
        dim(&format!("({source})"))
    ));

    Ok(())
}
```

- [ ] **Step 4: Build and run tests**

```bash
cargo build
mise run test:manual -- --ci layout:default-show layout:default-set layout:default-reset layout:default-conflict
```

Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add src/commands/layout.rs
git commit -m "feat(layout): add daft layout default command"
```

---

### Task 4: Update shell completions

**Files:**

- Modify: `src/commands/completions/bash.rs:206`
- Modify: `src/commands/completions/zsh.rs:281`
- Modify: `src/commands/completions/fish.rs:233`

- [ ] **Step 1: Update bash completions**

In `bash.rs` line 206, change:

```bash
COMPREPLY=( $(compgen -W "list show transform" -- "$cur") )
```

to:

```bash
COMPREPLY=( $(compgen -W "default list show transform" -- "$cur") )
```

- [ ] **Step 2: Update zsh completions**

In `zsh.rs` line 281, change:

```zsh
compadd list show transform
```

to:

```zsh
compadd default list show transform
```

- [ ] **Step 3: Update fish completions**

In `fish.rs` line 233, change:

```fish
complete -c daft -n '__fish_seen_subcommand_from layout; and not __fish_seen_subcommand_from list show transform' -f -a 'list show transform'
```

to:

```fish
complete -c daft -n '__fish_seen_subcommand_from layout; and not __fish_seen_subcommand_from default list show transform' -f -a 'default list show transform'
```

- [ ] **Step 4: Update fig completions**

Note: Layout currently has no Fig subcommand completions (pre-existing gap).
Skip for now — out of scope for this task.

- [ ] **Step 5: Run clippy and fmt**

```bash
mise run fmt && mise run clippy
```

- [ ] **Step 6: Commit**

```bash
git add src/commands/completions/
git commit -m "feat(layout): add completions for layout default"
```

---

### Task 5: Update long_about help text

**Files:**

- Modify: `src/commands/layout.rs:29-46` (the `long_about` string)

- [ ] **Step 1: Add default to the help text**

Add after the `transform` line:

```
Use `daft layout default` to view or change the global default layout.
```

- [ ] **Step 2: Regenerate man page**

```bash
mise run man:gen
```

- [ ] **Step 3: Final full test run**

```bash
mise run fmt && mise run clippy && mise run test:unit
mise run test:manual -- --ci layout:default-show layout:default-set layout:default-reset layout:default-conflict
```

- [ ] **Step 4: Commit**

```bash
git add src/commands/layout.rs man/daft-layout.1
git commit -m "docs(layout): add layout default to help text and man page"
```
