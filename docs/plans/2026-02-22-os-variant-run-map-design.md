# OS-Variant Run Map

Replace per-OS job duplication with polymorphic `run`, `skip`, and `only` fields
that accept OS-keyed maps. Jobs with no matching OS variant are silently
excluded from execution and output.

## Problem

OS targeting via the top-level `os` field requires duplicate jobs per platform
(e.g., `install-mise-macos`, `install-mise-linux`). This inflates output with
skip messages for irrelevant platforms and forces awkward `needs` dependencies
that reference all OS variants.

## Solution

### Polymorphic `run` field

Three forms, resolved at execution time:

```yaml
# 1. String (all platforms, unchanged)
- name: mise-install
  run: mise install

# 2. List (all platforms, unchanged)
- name: setup
  run:
    - mise install
    - lefthook install

# 3. OS-keyed map (new)
- name: install-mise
  run:
    macos: brew install mise
    linux: curl https://mise.run | sh
```

OS map values can themselves be strings or lists.

Valid OS keys: `macos`, `linux`, `windows`.

When the current OS has no matching key, the job is a **platform skip** --
completely invisible in output.

### Polymorphic `skip` and `only` fields

Same pattern -- can be a universal value or an OS-keyed map:

```yaml
# Universal (unchanged)
skip:
  - run: "command -v mise"
    desc: mise is already installed

# OS-keyed
skip:
  macos:
    - run: "brew list mise"
      desc: mise is already installed
  linux:
    - run: "command -v mise"
      desc: mise is already installed
```

When `skip`/`only` is an OS map and the current OS has no key, no conditions
apply (the job runs unconditionally for that OS, assuming `run` matched).

### Removed fields

- `os` on `JobDef` -- removed immediately (feature is <24h old, no external
  users)

### Kept fields

- `arch` on `JobDef` -- unchanged, orthogonal filter on the whole job

## Execution order

1. **Resolve run variant**: if `run` is an OS map, look up current OS. No match
   -> silent platform skip (no output).
2. **Check arch constraint**: if set and doesn't match, skip with message
   (existing behavior).
3. **Resolve skip/only variant**: if OS map, look up current OS to get the
   applicable rules.
4. **Evaluate skip/only conditions**: only runs after platform is resolved.
5. **Execute the resolved command**.

## Dependency semantics

- Platform-skipped jobs count as **satisfied** for `needs` -- dependents
  proceed.
- Condition-skipped jobs (via `skip`) also count as satisfied (existing
  behavior).
- Only **failed** jobs block dependents.

## Output behavior

- Platform-skipped (no OS variant): completely invisible.
- Condition-skipped (via `skip`/`only`): shown as `(skip) reason` (unchanged).
- Arch-skipped: shown as `(skip) not on <arch>` (unchanged).
- Running: shown normally (unchanged).

## Data model changes

### `RunCommand` (new, replaces `Option<String>`)

```rust
enum RunCommand {
    Simple(String),
    List(Vec<String>),
    Platform(HashMap<TargetOs, PlatformRunCommand>),
}

enum PlatformRunCommand {
    Simple(String),
    List(Vec<String>),
}
```

`JobDef.run` changes from `Option<String>` to `Option<RunCommand>`.

### `SkipCondition` (extended)

Add a `Platform` variant:

```rust
enum SkipCondition {
    Bool(bool),
    EnvVar(String),
    Rules(Vec<SkipRule>),
    Platform(HashMap<TargetOs, Vec<SkipRule>>),
}
```

### `OnlyCondition` (extended)

Same pattern:

```rust
enum OnlyCondition {
    Bool(bool),
    EnvVar(String),
    Rules(Vec<OnlyRule>),
    Platform(HashMap<TargetOs, Vec<OnlyRule>>),
}
```

### `PlatformConstraint<T>` and `TargetOs`/`TargetArch`

Kept as-is. `TargetOs` gains `Hash` derive for use as HashMap key.

### `os` field removed from `JobDef`

The `os: Option<PlatformConstraint<TargetOs>>` field is deleted.

## Example: project daft.yml after migration

```yaml
hooks:
  post-clone:
    jobs:
      - name: install-brew
        description: Install Homebrew package manager
        run:
          macos: |
            /bin/bash -c "$(curl -fsSL
            https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
        skip:
          - run: "command -v brew"
            desc: Brew is already installed

      - name: install-mise
        description: Install mise
        run:
          macos: brew install mise
          linux: curl https://mise.run | sh
        needs: [install-brew]
        skip:
          - run: "command -v mise"
            desc: mise is already installed

      - name: mise-install
        description: Install tools from mise.toml
        run: mise install
        needs: [install-mise]

      - name: install-lefthook-in-repo
        description: Set up lefthook git hooks
        run: lefthook install
        needs: [mise-install]
        skip:
          - run: "lefthook check-install"
            desc: Lefthook hooks are already installed
```

### Output on macOS (all cached)

```
  install-brew (skip) Brew is already installed
  install-mise (skip) mise is already installed
  mise-install > ...
  install-lefthook-in-repo (skip) Lefthook hooks are already installed
```

### Output on Linux (fresh)

```
  install-mise > ...
  mise-install > ...
  install-lefthook-in-repo > ...
```

`install-brew` is invisible on Linux (no `linux` key in its `run` map).

## Validation rules

- If `run` is an OS map, each key must be a valid `TargetOs`.
- If `skip`/`only` is an OS map, keys must be valid `TargetOs` values.
- A job must have either `run` or `script` (unchanged).
- `script` is NOT made polymorphic (rare usage, not worth the complexity).

## Migration from `os` field

The `os` field is removed. To migrate:

```yaml
# Before:
- name: install-brew
  os: macos
  run: brew install mise

# After:
- name: install-brew
  run:
    macos: brew install mise
```
