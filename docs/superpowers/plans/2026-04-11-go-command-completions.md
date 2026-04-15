# `daft go` Completion Overhaul — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Group `daft go` completions as worktrees → local → remote, fix the zsh
flag-leak bug, color each group distinctly, and add a fetch-on-miss spinner path
for remote-only branches.

**Architecture:** A new pure function `build_go_completions()` in
`src/commands/complete.rs` takes already-collected git ref data and produces a
`Vec<CompletionEntry>`. A thin wrapper `complete_daft_go()` collects real data
from git, calls the pure function, and optionally runs a `git fetch` with a
`/dev/tty` spinner when the prefix yields no local matches. Shell generators in
`completions/{zsh,bash,fish}.rs` emit bespoke code paths for `daft-go` that
parse tab-separated output (name, group, description) and render per-group via
`_describe -t <tag>` in zsh, ordered flat lists in bash, and awk-reshuffled
descriptions in fish.

**Tech Stack:** Rust, clap, lefthook, mise, YAML integration scenarios
(`tests/manual/scenarios/`).

**Spec:** `docs/superpowers/specs/2026-04-11-go-command-completions-design.md`

---

## File map

- **Modify** `src/commands/complete.rs` — add `CompletionEntry`,
  `CompletionGroup`, `build_go_completions()`, `complete_daft_go()`,
  `--fetch-on-miss` flag, fetch cooldown, spinner integration. Route
  `("daft-go", 1)` to `complete_daft_go()`.
- **Create** `src/completion_spinner.rs` — `/dev/tty` braille-dot spinner with a
  background thread and cancellation signal.
- **Modify** `src/lib.rs` — declare `pub mod completion_spinner;`.
- **Modify** `src/core/settings.rs` — add `go_fetch_on_miss: bool` to
  `DaftSettings`, `GO_FETCH_ON_MISS` const in `defaults` and `keys`, wire into
  `load()` / `load_global()`.
- **Modify** `src/commands/completions/zsh.rs` — bespoke `daft-go` generator
  path with `_describe -t <tag>` per group, flag gating on `-`, and
  tab-separated stdout parsing. Add zstyle coloring block to
  `DAFT_ZSH_COMPLETIONS`.
- **Modify** `src/commands/completions/bash.rs` — bespoke `daft-go` generator
  path that concatenates `wt + local + remote` in order and best-effort
  `compopt -o nosort`.
- **Modify** `src/commands/completions/fish.rs` — update the `daft-go`
  completion line to pass `--fetch-on-miss` and awk-reshuffle tab-separated
  output into fish's `name\tdescription` format.
- **Modify** `src/commands/completions/mod.rs` — add test module wiring if
  needed (most tests go inside existing generator files).
- **Create** `tests/manual/scenarios/completions/go-grouped.yml` — end-to-end
  scenario that sets up a repo with worktrees, local branches, and remote-only
  branches, and asserts exact tab-separated output from
  `daft __complete daft-go`.
- **Modify** `docs/cli/daft-go.md` — add "Completion behavior" section.
- **Create** `test-plans/go-completions.md` — manual checklist for the spinner /
  color / group-ordering path that's hard to assert in automated tests.
- **Regenerate** `man/daft-go.1` via `mise run man:gen`.

---

## Ordering notes

Task 1 is a standalone regression fix for the zsh flag-leak bug. It ships
independently — if the rest of the plan is delayed, Task 1 has already fixed the
most user-visible annoyance. Tasks 2–9 build the new data layer; Tasks 10–13
swap the shell rendering. Task 14 wires colors. Tasks 15–16 are docs and manual
test plan.

---

### Task 1: Regression test + fix for zsh flag-gating in `daft-go`

**Why first:** CLAUDE.md requires every bugfix to ship with a regression test.
This is a self-contained fix that's valuable even if the rest of the plan is
never merged.

**Files:**

- Modify: `src/commands/completions/zsh.rs` (the branch-completion block for
  commands in the `has_branches` set)
- Modify: `src/commands/completions/mod.rs` — add `#[cfg(test)] mod tests` block
  if one doesn't exist yet

- [ ] **Step 1: Write the failing unit test**

Add at the bottom of `src/commands/completions/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_daft_go_gates_flags_on_leading_dash() {
        let script = zsh::generate_zsh_completion_string("daft-go")
            .expect("generator must succeed");
        let flags_pos = script
            .find("compadd -a flags")
            .expect("generated script must contain `compadd -a flags`");
        let guard_pos = script
            .find("[[ \"$curword\" == -* ]]")
            .expect(
                "generated script must gate flag completion on a leading \
                 dash before adding flags (zsh flag-leak regression)",
            );
        assert!(
            guard_pos < flags_pos,
            "flag-gating guard must appear before `compadd -a flags`, \
             otherwise flags leak into branch completions. \
             guard_pos={guard_pos} flags_pos={flags_pos}",
        );
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```bash
cargo test -p daft --lib commands::completions::tests::zsh_daft_go_gates_flags_on_leading_dash
```

Expected: FAIL with
`generated script must gate flag completion on a leading dash before adding flags`.
Current generator does not emit that guard — it adds flags unconditionally via
`compadd -a flags`.

- [ ] **Step 3: Fix the zsh generator**

In `src/commands/completions/zsh.rs`, locate the block that reads (approximately
lines 171–186):

```rust
output.push_str("    # Flag completions (extracted from clap)\n");
output.push_str("    local -a flags\n");
output.push_str("    flags=(\n");

let cmd = get_command_for_name(command_name)
    .context(format!("Unknown command: {}", command_name))?;
let (all_flags, _, _) = extract_flags(&cmd);

for flag in all_flags {
    output.push_str(&format!("        '{}'\n", flag));
}

output.push_str("    )\n");
output.push_str("    compadd -a flags\n");
output.push_str("}\n");
```

Wrap the `compadd -a flags` call in a guard so it only runs when the user has
typed a leading dash:

```rust
output.push_str("    # Flag completions (extracted from clap)\n");
output.push_str("    if [[ \"$curword\" == -* ]]; then\n");
output.push_str("        local -a flags\n");
output.push_str("        flags=(\n");

let cmd = get_command_for_name(command_name)
    .context(format!("Unknown command: {}", command_name))?;
let (all_flags, _, _) = extract_flags(&cmd);

for flag in all_flags {
    output.push_str(&format!("            '{}'\n", flag));
}

output.push_str("        )\n");
output.push_str("        compadd -a flags\n");
output.push_str("    fi\n");
output.push_str("}\n");
```

Note: the branch-completion block earlier in the function already has its own
`if [[ $curword != -* ]]; then ... fi` guard and continues to work as-is. Moving
the flag block into an `if` guard means the function only emits flags when the
user typed a `-`, and only emits branches when they didn't — never both.

- [ ] **Step 4: Run the test and verify it passes**

```bash
cargo test -p daft --lib commands::completions::tests::zsh_daft_go_gates_flags_on_leading_dash
```

Expected: PASS.

- [ ] **Step 5: Run the full completion test suite to catch regressions**

```bash
cargo test -p daft --lib commands::completions
mise run test:manual -- --ci completions
```

Expected: all tests pass. The manual scenarios in
`tests/manual/scenarios/completions/` verify that generated scripts for every
command are parseable by the target shell — they must still pass after the guard
change.

- [ ] **Step 6: Commit**

```bash
git add src/commands/completions/zsh.rs src/commands/completions/mod.rs
git commit -m "$(cat <<'EOF'
fix(completions): gate zsh flag completions on leading dash

Flags were being added unconditionally via `compadd -a flags`, so they
leaked into branch completions for `daft go`, `daft start`, and other
commands with dynamic branch completion. Wrap the flag block in an
`if [[ $curword == -* ]]` guard so flags only appear when the user has
typed a dash.

Includes a regression test that asserts the guard is present and
appears before the `compadd -a flags` call in the generated script.
EOF
)"
```

---

### Task 2: Add `go_fetch_on_miss` setting to `DaftSettings`

**Why:** `complete_daft_go()` in Task 5 will need to read this setting. Adding
it first means the data-layer tasks can reference it directly.

**Files:**

- Modify: `src/core/settings.rs`

- [ ] **Step 1: Write the failing unit test**

Add to the existing `#[cfg(test)] mod tests` block in `src/core/settings.rs`
(the file already has a tests module — grep for `#[test]` near the end of the
file to find it):

```rust
#[test]
fn default_settings_have_go_fetch_on_miss_true() {
    let settings = DaftSettings::default();
    assert!(
        settings.go_fetch_on_miss,
        "go.fetchOnMiss must default to true — the fetch-on-miss spinner \
         path is opt-out, not opt-in"
    );
}
```

- [ ] **Step 2: Run the test and verify it fails**

```bash
cargo test -p daft --lib core::settings::tests::default_settings_have_go_fetch_on_miss_true
```

Expected: FAIL with `no field 'go_fetch_on_miss' on type 'DaftSettings'`.

- [ ] **Step 3: Add the constant, key, struct field, default, and loaders**

In `src/core/settings.rs`, near the existing `GO_AUTO_START` entries:

- In `mod defaults`, add:

```rust
/// Default value for go.fetchOnMiss setting.
pub const GO_FETCH_ON_MISS: bool = true;
```

- In `mod keys`, add (placed near `GO_AUTO_START`):

```rust
/// Config key for go.fetchOnMiss setting.
pub const GO_FETCH_ON_MISS: &str = "daft.go.fetchOnMiss";
```

- In `pub struct DaftSettings`, add (placed near `go_auto_start`):

```rust
/// Whether `daft go` completion should run `git fetch` when the typed
/// prefix has no local matches. Controlled by `daft.go.fetchOnMiss`.
pub go_fetch_on_miss: bool,
```

- In `impl Default for DaftSettings`, add (near `go_auto_start`):

```rust
go_fetch_on_miss: defaults::GO_FETCH_ON_MISS,
```

- In `load()` (near the `GO_AUTO_START` block), add:

```rust
if let Some(value) = git.config_get(keys::GO_FETCH_ON_MISS)? {
    settings.go_fetch_on_miss = parse_bool(&value, defaults::GO_FETCH_ON_MISS);
}
```

- In `load_global()` (near the `GO_AUTO_START` block), add:

```rust
if let Some(value) = git.config_get_global(keys::GO_FETCH_ON_MISS)? {
    settings.go_fetch_on_miss = parse_bool(&value, defaults::GO_FETCH_ON_MISS);
}
```

- [ ] **Step 4: Run the test and verify it passes**

```bash
cargo test -p daft --lib core::settings::tests::default_settings_have_go_fetch_on_miss_true
```

Expected: PASS.

- [ ] **Step 5: Run the full settings test module**

```bash
cargo test -p daft --lib core::settings
```

Expected: all existing tests still pass (the new field has a default value, so
existing tests that construct `DaftSettings::default()` are unaffected).

- [ ] **Step 6: Commit**

```bash
git add src/core/settings.rs
git commit -m "$(cat <<'EOF'
feat(settings): add go.fetchOnMiss to DaftSettings

Add `go_fetch_on_miss: bool` field (default true) and wire it through
the local and global config loaders. This setting controls whether
`daft go` completion runs `git fetch` with a spinner when the user
types a prefix that matches nothing locally.
EOF
)"
```

---

### Task 3: Define types and the pure grouping function

**Files:**

- Modify: `src/commands/complete.rs`

- [ ] **Step 1: Write failing unit tests for grouping behavior**

Add to the existing `#[cfg(test)] mod tests` block at the bottom of
`src/commands/complete.rs`:

```rust
// Tests for the new go-completion grouping function.

fn wt(name: &str, path: &str) -> (String, std::path::PathBuf) {
    (name.to_string(), std::path::PathBuf::from(path))
}

fn br(name: &str, age: &str) -> (String, String) {
    (name.to_string(), age.to_string())
}

#[test]
fn go_completions_group_order_is_worktrees_then_local_then_remote() {
    let entries = build_go_completions(
        &[wt("master", "/tmp/repo/master")],
        &[br("feat/local", "4 days ago")],
        &[br("origin/bug/xyz", "3 weeks ago")],
        None,   // no current worktree
        "origin",
        false,  // single-remote mode
        "",
    );
    let groups: Vec<CompletionGroup> = entries.iter().map(|e| e.group).collect();
    assert_eq!(
        groups,
        vec![
            CompletionGroup::Worktree,
            CompletionGroup::Local,
            CompletionGroup::Remote,
        ],
        "worktrees must come first, then local, then remote"
    );
}

#[test]
fn go_completions_sort_within_group_alphabetically() {
    let entries = build_go_completions(
        &[wt("b", "/tmp/b"), wt("a", "/tmp/a")],
        &[br("z", "1 day ago"), br("m", "2 days ago")],
        &[],
        None,
        "origin",
        false,
        "",
    );
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "m", "z"]);
}

#[test]
fn go_completions_local_shadows_remote() {
    let entries = build_go_completions(
        &[],
        &[br("feat/shared", "1 day ago")],
        &[br("origin/feat/shared", "2 days ago")],
        None,
        "origin",
        false,
        "",
    );
    assert_eq!(entries.len(), 1, "remote should be shadowed by local");
    assert_eq!(entries[0].name, "feat/shared");
    assert_eq!(entries[0].group, CompletionGroup::Local);
}

#[test]
fn go_completions_worktree_shadows_local_and_remote() {
    let entries = build_go_completions(
        &[wt("master", "/tmp/master")],
        &[br("master", "1 day ago")],
        &[br("origin/master", "2 days ago")],
        None,
        "origin",
        false,
        "",
    );
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "master");
    assert_eq!(entries[0].group, CompletionGroup::Worktree);
}

#[test]
fn go_completions_exclude_current_worktree() {
    let entries = build_go_completions(
        &[wt("master", "/tmp/master"), wt("feat/x", "/tmp/feat-x")],
        &[],
        &[],
        Some("feat/x"),
        "origin",
        false,
        "",
    );
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["master"]);
}

#[test]
fn go_completions_strip_remote_prefix_in_single_remote_mode() {
    let entries = build_go_completions(
        &[],
        &[],
        &[br("origin/bug/xyz", "3 weeks ago")],
        None,
        "origin",
        false,
        "",
    );
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "bug/xyz");
    assert_eq!(entries[0].group, CompletionGroup::Remote);
}

#[test]
fn go_completions_keep_remote_prefix_in_multi_remote_mode() {
    let entries = build_go_completions(
        &[],
        &[],
        &[
            br("origin/bug/xyz", "3 weeks ago"),
            br("fork/feat/y", "2 days ago"),
        ],
        None,
        "origin",
        true, // multi-remote mode
        "",
    );
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["fork/feat/y", "origin/bug/xyz"]);
}

#[test]
fn go_completions_filter_by_prefix() {
    let entries = build_go_completions(
        &[wt("master", "/tmp/master")],
        &[br("feat/x", "1d"), br("fix/y", "2d")],
        &[br("origin/bug/z", "3w")],
        None,
        "origin",
        false,
        "fe",
    );
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["feat/x"]);
}

#[test]
fn go_completions_drop_remote_head_symrefs() {
    let entries = build_go_completions(
        &[],
        &[],
        &[br("origin/HEAD", "just now"), br("origin/master", "1 day ago")],
        None,
        "origin",
        false,
        "",
    );
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["master"]);
}

#[test]
fn format_go_completions_emits_tab_separated_name_group_description() {
    let entries = vec![
        CompletionEntry {
            name: "master".into(),
            group: CompletionGroup::Worktree,
            description: "2 hours ago".into(),
        },
        CompletionEntry {
            name: "feat/bar".into(),
            group: CompletionGroup::Local,
            description: "4 days ago".into(),
        },
    ];
    let out = format_go_completions(&entries);
    assert_eq!(
        out,
        "master\tworktree\t2 hours ago\nfeat/bar\tlocal\t4 days ago\n"
    );
}
```

- [ ] **Step 2: Run tests and verify they fail**

```bash
cargo test -p daft --lib commands::complete::tests::go_completions
cargo test -p daft --lib commands::complete::tests::format_go_completions
```

Expected: all FAIL with `cannot find function build_go_completions` /
`cannot find type CompletionGroup`.

- [ ] **Step 3: Define types and implement the pure function**

Add to `src/commands/complete.rs` (below the existing `complete` function and
its helpers, above the existing `#[cfg(test)] mod tests` block):

```rust
/// Which group a completion entry belongs to, used for visual separation
/// in shells that support per-item tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionGroup {
    /// Branch has a worktree — immediate navigation target.
    Worktree,
    /// Local branch without a worktree.
    Local,
    /// Remote-tracking branch not mirrored locally.
    Remote,
}

impl CompletionGroup {
    fn as_str(self) -> &'static str {
        match self {
            CompletionGroup::Worktree => "worktree",
            CompletionGroup::Local => "local",
            CompletionGroup::Remote => "remote",
        }
    }
}

/// A single completion candidate emitted by `daft __complete daft-go`.
#[derive(Debug, Clone)]
pub(crate) struct CompletionEntry {
    pub name: String,
    pub group: CompletionGroup,
    pub description: String,
}

/// Pure grouping/dedupe/filter/sort function for `daft go` completions.
///
/// Takes already-collected git data and produces a flat, ordered list of
/// completion entries: worktrees first, then local branches, then remote
/// branches. Within each group, entries are sorted alphabetically.
///
/// Dedupe rules: worktree shadows local and remote; local shadows remote.
/// Shadowing is by stripped-name comparison — a remote-only branch whose
/// stripped name collides with a local or worktree branch is dropped.
///
/// The current worktree (if any) is excluded from the worktree group —
/// `daft go` to the branch you're already on is a no-op.
///
/// In single-remote mode, the leading `<default_remote>/` prefix is
/// stripped from remote-branch names. In multi-remote mode the full
/// `<remote>/<branch>` form is preserved. HEAD symrefs (`origin/HEAD`,
/// etc.) are always dropped.
///
/// Entries whose name doesn't start with `prefix` are filtered out.
pub(crate) fn build_go_completions(
    worktrees: &[(String, std::path::PathBuf)],
    local_branches: &[(String, String)],
    remote_branches: &[(String, String)],
    current_worktree_branch: Option<&str>,
    default_remote: &str,
    multi_remote: bool,
    prefix: &str,
) -> Vec<CompletionEntry> {
    use std::collections::BTreeSet;

    // Worktree group: exclude the current worktree's branch.
    let mut wt_entries: Vec<CompletionEntry> = worktrees
        .iter()
        .filter(|(name, _)| Some(name.as_str()) != current_worktree_branch)
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, path)| CompletionEntry {
            name: name.clone(),
            group: CompletionGroup::Worktree,
            description: path.display().to_string(),
        })
        .collect();
    wt_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let wt_names: BTreeSet<&str> =
        wt_entries.iter().map(|e| e.name.as_str()).collect();

    // Local group: drop anything already in the worktree group.
    let mut local_entries: Vec<CompletionEntry> = local_branches
        .iter()
        .filter(|(name, _)| !wt_names.contains(name.as_str()))
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, age)| CompletionEntry {
            name: name.clone(),
            group: CompletionGroup::Local,
            description: age.clone(),
        })
        .collect();
    local_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let local_names: BTreeSet<String> = local_entries
        .iter()
        .map(|e| e.name.clone())
        .collect::<BTreeSet<_>>();

    // Remote group: drop HEAD symrefs, prefix-strip in single-remote mode,
    // dedupe against worktree + local by stripped name.
    let prefix_to_strip = format!("{default_remote}/");
    let mut remote_entries: Vec<CompletionEntry> = remote_branches
        .iter()
        .filter(|(name, _)| !name.ends_with("/HEAD") && name != "HEAD")
        .filter_map(|(name, age)| {
            let display = if multi_remote {
                name.clone()
            } else if let Some(stripped) = name.strip_prefix(&prefix_to_strip) {
                stripped.to_string()
            } else {
                // In single-remote mode, a remote from a non-default remote
                // is unusual — keep its full name rather than inventing a
                // shadowing rule.
                name.clone()
            };
            if wt_names.contains(display.as_str())
                || local_names.contains(&display)
            {
                return None;
            }
            if !display.starts_with(prefix) {
                return None;
            }
            Some(CompletionEntry {
                name: display,
                group: CompletionGroup::Remote,
                description: age.clone(),
            })
        })
        .collect();
    remote_entries.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = Vec::with_capacity(
        wt_entries.len() + local_entries.len() + remote_entries.len(),
    );
    out.extend(wt_entries);
    out.extend(local_entries);
    out.extend(remote_entries);
    out
}

/// Format grouped completion entries as tab-separated lines for the
/// shell completion protocol: `<name>\t<group>\t<description>`.
pub(crate) fn format_go_completions(entries: &[CompletionEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&entry.name);
        out.push('\t');
        out.push_str(entry.group.as_str());
        out.push('\t');
        out.push_str(&entry.description);
        out.push('\n');
    }
    out
}
```

- [ ] **Step 4: Run tests and verify they pass**

```bash
cargo test -p daft --lib commands::complete::tests
```

Expected: all ten new tests PASS. Existing tests
(`test_suggest_new_branch_names`, `test_suggest_new_branch_names_no_match`)
still PASS.

- [ ] **Step 5: Run clippy**

```bash
mise run clippy
```

Expected: zero warnings.

- [ ] **Step 6: Commit**

```bash
git add src/commands/complete.rs
git commit -m "$(cat <<'EOF'
feat(completions): add pure grouping function for daft-go completions

Introduce CompletionGroup, CompletionEntry, build_go_completions(), and
format_go_completions() in src/commands/complete.rs. These are the
data-layer primitives for the grouped daft go completion overhaul.

The grouping function takes already-collected ref data and applies
dedupe, prefix filtering, current-worktree exclusion, and multi-remote
naming rules. Unit-tested with fixture inputs — no git invocations
yet.

Spec: docs/superpowers/specs/2026-04-11-go-command-completions-design.md
EOF
)"
```

---

### Task 4: Collect git data and wire `complete_daft_go()` into the dispatcher

**Files:**

- Modify: `src/commands/complete.rs`

- [ ] **Step 1: Add the git collection helpers and wrapper**

Add to `src/commands/complete.rs`, near the existing
`complete_existing_branches` helper:

```rust
/// Collect `(branch, path)` pairs for every linked worktree that has a
/// branch checked out. Detached HEADs and bare repos are skipped —
/// they're not navigation targets.
fn collect_go_worktrees() -> Vec<(String, std::path::PathBuf)> {
    use crate::git::GitCommand;

    let git = GitCommand::new(true);
    let entries =
        match crate::core::worktree::prune::parse_worktree_list(&git) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

    entries
        .into_iter()
        .filter(|wt| !wt.is_bare && !wt.is_detached)
        .filter_map(|wt| wt.branch.map(|b| (b, wt.path)))
        .collect()
}

/// Collect `(branch, relative_age)` pairs for every local branch.
fn collect_go_local_branches() -> Vec<(String, String)> {
    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)%09%(committerdate:relative)",
            "refs/heads/",
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (name, age) = line.split_once('\t')?;
            Some((name.to_string(), age.to_string()))
        })
        .collect()
}

/// Collect `(branch, relative_age)` pairs for every remote-tracking
/// branch across all remotes.
fn collect_go_remote_branches() -> Vec<(String, String)> {
    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)%09%(committerdate:relative)",
            "refs/remotes/",
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (name, age) = line.split_once('\t')?;
            Some((name.to_string(), age.to_string()))
        })
        .collect()
}

/// Collect the current worktree's branch, if any — used to exclude it
/// from the completion list. Returns `None` if the caller is outside a
/// worktree or the current worktree is detached.
fn current_worktree_branch() -> Option<String> {
    use crate::git::GitCommand;

    let git = GitCommand::new(true);
    let current_path = crate::core::repo::get_current_worktree_path().ok()?;
    let entries =
        crate::core::worktree::prune::parse_worktree_list(&git).ok()?;
    entries
        .into_iter()
        .find(|wt| wt.path == current_path)
        .and_then(|wt| wt.branch)
}

/// Top-level completion helper for `daft go`. Collects real git data,
/// applies grouping rules, and returns the ordered candidate list.
/// `fetch_on_miss` is wired in Task 9 — for now it is always false.
pub(crate) fn complete_daft_go(
    prefix: &str,
    _fetch_on_miss: bool,
) -> Result<Vec<CompletionEntry>> {
    let settings =
        crate::core::settings::DaftSettings::load().unwrap_or_default();
    let default_remote = if settings.multi_remote_enabled {
        settings.multi_remote_default.clone()
    } else {
        settings.remote.clone()
    };

    let worktrees = collect_go_worktrees();
    let local = collect_go_local_branches();
    let remote = collect_go_remote_branches();
    let current_branch = current_worktree_branch();

    Ok(build_go_completions(
        &worktrees,
        &local,
        &remote,
        current_branch.as_deref(),
        &default_remote,
        settings.multi_remote_enabled,
        prefix,
    ))
}
```

- [ ] **Step 2: Route `("daft-go", 1)` to `complete_daft_go()` and emit the
      tab-separated format**

In the existing `complete()` dispatcher in `src/commands/complete.rs`, replace
the `daft-go` arm:

```rust
// daft-go: complete existing branch names
("daft-go", 1) => complete_existing_branches(word, verbose),
```

with:

```rust
// daft-go: grouped worktree/local/remote completions
("daft-go", 1) => {
    let entries = complete_daft_go(word, false)?;
    // Re-emit as individual tab-separated lines so the outer
    // `for suggestion in suggestions { println!(...) }` loop in
    // run() writes one line per entry, matching the shell-facing
    // format produced by `format_go_completions()`.
    Ok(entries
        .iter()
        .map(|e| format!("{}\t{}\t{}", e.name, e.group.as_str(), e.description))
        .collect())
}
```

Note: we deliberately don't call `format_go_completions()` here because the
dispatcher returns `Vec<String>` (one entry per line) while
`format_go_completions()` returns a single pre-joined `String`.
`format_go_completions()` stays in the module for direct use by unit tests and
for any future caller that wants the full stdout blob.

The outer `run()` function already prints each suggestion on its own line, so
the existing `for suggestion in suggestions { println!(...); }` loop handles the
tab-separated strings correctly.

- [ ] **Step 3: Build and run the existing unit tests**

```bash
cargo build -p daft
cargo test -p daft --lib commands::complete
```

Expected: compiles cleanly, all existing tests pass. The new git collection
helpers aren't unit-tested here — they're covered by the YAML scenario in
Task 6.

- [ ] **Step 4: Manual smoke test in the daft repo itself**

Per `CLAUDE.md`: never use the daft repo as a test target for worktree
operations. But `daft __complete` is read-only and safe. Run:

```bash
cargo build
./target/debug/daft __complete daft-go "" --position 1
```

Expected output: tab-separated lines with your current worktrees, local branches
(minus any with worktrees), and remote-only branches. The current worktree's
branch should be absent from the worktree group.

- [ ] **Step 5: Commit**

```bash
git add src/commands/complete.rs
git commit -m "$(cat <<'EOF'
feat(completions): collect grouped go completions from live git data

Add `collect_go_worktrees`, `collect_go_local_branches`,
`collect_go_remote_branches`, `current_worktree_branch`, and the
`complete_daft_go` wrapper. Route the `daft-go` completion arm to the
new function and emit tab-separated `name<TAB>group<TAB>description`
lines.

Shell scripts that only read column 1 continue to work; zsh/fish
rendering using columns 2-3 lands in later tasks.
EOF
)"
```

---

### Task 5: YAML scenario exercising the grouped completion

**Files:**

- Create: `tests/manual/scenarios/completions/go-grouped.yml`

- [ ] **Step 1: Look at an existing similar scenario for schema cues**

```bash
cat tests/manual/scenarios/completions/dynamic-branch.yml
```

Note the `steps`, `run`, `cwd`, `expect.exit_code`, and `expect.output_contains`
/ `expect.output_matches` fields.

- [ ] **Step 2: Write the failing scenario**

Create `tests/manual/scenarios/completions/go-grouped.yml`:

```yaml
name: daft go completion groups worktrees, locals, and remotes
description:
  `daft __complete daft-go` emits tab-separated lines grouped as
  worktrees first, then local branches, then remote branches, with
  correct dedupe and current-worktree exclusion.

steps:
  - name: Create a bare repo with linked worktrees, local branches, and a fake remote
    run: |
      set -e
      mkdir -p $WORK_DIR/go-complete && cd $WORK_DIR/go-complete &&
      git init --bare origin.git &&
      git clone origin.git work &&
      cd work &&
      git config user.name "Test" &&
      git config user.email "test@test.com" &&
      echo "hello" > README.md &&
      git add README.md &&
      git commit -m "initial" &&
      git branch master-dup &&
      git checkout -b feat/local-only &&
      git commit --allow-empty -m "local work" &&
      git checkout -b feat/with-worktree &&
      git commit --allow-empty -m "worktree work" &&
      git checkout master &&
      git push origin master feat/local-only feat/with-worktree master-dup &&
      git checkout -b feat/remote-shadow &&
      git push origin feat/remote-shadow &&
      git checkout master &&
      git branch -D feat/remote-shadow &&
      git fetch origin &&
      git worktree add ../feat-with-worktree feat/with-worktree
    expect:
      exit_code: 0

  - name: Run completion from the master worktree (excludes master from worktree group)
    run: $DAFT_BIN __complete daft-go "" --position 1
    cwd: "$WORK_DIR/go-complete/work"
    expect:
      exit_code: 0
      # Expected tab-separated lines, in grouped order.
      # Worktree group contains feat/with-worktree (master excluded as current).
      # Local group contains feat/local-only, master-dup.
      # Remote group contains feat/remote-shadow (only that remote is not shadowed).
      output_contains:
        - "feat/with-worktree\tworktree\t"
        - "feat/local-only\tlocal\t"
        - "master-dup\tlocal\t"
        - "feat/remote-shadow\tremote\t"
      output_not_contains:
        - "master\tworktree"
        - "master\tlocal"
        - "origin/master"
        - "origin/HEAD"

  - name: Prefix filter returns only matching entries
    run: $DAFT_BIN __complete daft-go "feat/l" --position 1
    cwd: "$WORK_DIR/go-complete/work"
    expect:
      exit_code: 0
      output_contains:
        - "feat/local-only\tlocal\t"
      output_not_contains:
        - "feat/with-worktree"
        - "feat/remote-shadow"
        - "master-dup"
```

- [ ] **Step 3: Verify the YAML runner supports `output_not_contains`**

```bash
grep -rn "output_not_contains" tests/manual/ | head -5
```

Expected: at least a few hits showing the field is supported by the runner. If
not supported, the test must use `output_matches` with a negated regex instead —
adjust the step to use only `output_contains` and leave `output_not_contains`
out of this task; the positive assertions cover the grouping behavior.

- [ ] **Step 4: Run the scenario**

```bash
mise run test:manual -- --ci completions go-grouped
```

Expected: the scenario sets up the repo and asserts the grouped output. Should
PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/manual/scenarios/completions/go-grouped.yml
git commit -m "$(cat <<'EOF'
test(completions): add go-grouped integration scenario

Sets up a bare repo with worktrees, local branches, and remote-only
branches, then asserts that `daft __complete daft-go` emits the
expected tab-separated grouped output with correct dedupe and
current-worktree exclusion.
EOF
)"
```

---

### Task 6: Fetch cooldown marker

**Files:**

- Modify: `src/commands/complete.rs`

- [ ] **Step 1: Write failing unit tests**

Add to the tests module in `src/commands/complete.rs`:

```rust
use std::time::{Duration, SystemTime};

#[test]
fn fetch_cooldown_allows_fetch_when_marker_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("daft_complete_last_fetch");
    assert!(
        should_run_fetch(&marker, Duration::from_secs(30)),
        "missing marker must allow fetch"
    );
}

#[test]
fn fetch_cooldown_blocks_fetch_within_window() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("daft_complete_last_fetch");
    touch_fetch_marker(&marker).unwrap();
    assert!(
        !should_run_fetch(&marker, Duration::from_secs(30)),
        "freshly touched marker must block fetch"
    );
}

#[test]
fn fetch_cooldown_allows_fetch_after_window() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("daft_complete_last_fetch");
    touch_fetch_marker(&marker).unwrap();
    // Backdate the marker by 31 seconds.
    let old = SystemTime::now() - Duration::from_secs(31);
    filetime::set_file_mtime(&marker, filetime::FileTime::from_system_time(old))
        .unwrap();
    assert!(
        should_run_fetch(&marker, Duration::from_secs(30)),
        "marker older than cooldown must allow fetch"
    );
}
```

Also add the `filetime` dev-dependency if it isn't already present:

```bash
grep -n 'filetime' Cargo.toml
```

If absent, add under `[dev-dependencies]`:

```toml
filetime = "0.2"
tempfile = "3"
```

(`tempfile` is already used elsewhere in this crate — verify with
`grep -n 'tempfile' Cargo.toml`. If already present, leave alone.)

- [ ] **Step 2: Run tests and verify they fail**

```bash
cargo test -p daft --lib commands::complete::tests::fetch_cooldown
```

Expected: FAIL with `cannot find function should_run_fetch` /
`touch_fetch_marker`.

- [ ] **Step 3: Implement the cooldown helpers**

Add to `src/commands/complete.rs`:

```rust
/// Return `true` if the cooldown marker is missing or older than
/// `cooldown`. Used to decide whether the fetch-on-miss path should run.
fn should_run_fetch(marker: &std::path::Path, cooldown: std::time::Duration) -> bool {
    let metadata = match std::fs::metadata(marker) {
        Ok(m) => m,
        Err(_) => return true, // Missing marker -> fetch allowed
    };
    let mtime = match metadata.modified() {
        Ok(t) => t,
        Err(_) => return true,
    };
    match std::time::SystemTime::now().duration_since(mtime) {
        Ok(age) => age >= cooldown,
        Err(_) => true, // Clock skew -> be permissive
    }
}

/// Create or update the cooldown marker to reflect a just-completed fetch.
fn touch_fetch_marker(marker: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(marker)?;
    filetime::set_file_mtime(
        marker,
        filetime::FileTime::from_system_time(std::time::SystemTime::now()),
    )?;
    Ok(())
}
```

- [ ] **Step 4: Run tests and verify they pass**

```bash
cargo test -p daft --lib commands::complete::tests::fetch_cooldown
```

Expected: all three tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/commands/complete.rs Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
feat(completions): add fetch cooldown marker helpers

`should_run_fetch` and `touch_fetch_marker` implement the 30-second
cooldown that prevents the fetch-on-miss spinner path from thrashing
`git fetch` on every keystroke. The marker lives at
`<git-common-dir>/daft_complete_last_fetch`.

Unit-tested against tempfile-backed markers with backdated mtimes.
EOF
)"
```

---

### Task 7: `/dev/tty` spinner module

**Files:**

- Create: `src/completion_spinner.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the failing unit test for the frame sequence**

Create `src/completion_spinner.rs`:

```rust
//! Single-line braille-dot spinner drawn directly to `/dev/tty`.
//!
//! Used by shell-completion paths that need to run a slow operation
//! (e.g. `git fetch`) while giving the user a visible signal that
//! something is happening. The shell completion function itself is
//! synchronous from readline's perspective — the spinner is drawn by
//! this module (not the shell wrapper) by opening `/dev/tty` directly
//! and writing frames prefixed with `\r` so each frame overwrites the
//! previous one in place.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_frames_cycle_through_braille_dots() {
        let frames: Vec<&str> = (0..20).map(frame_for_tick).collect();
        assert_eq!(frames[0], "⠋");
        assert_eq!(frames[1], "⠙");
        assert_eq!(frames[9], "⠏");
        assert_eq!(frames[10], "⠋", "frame sequence must wrap at 10");
        assert_eq!(frames[19], "⠏");
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```bash
cargo test -p daft --lib completion_spinner
```

Expected: FAIL — module doesn't exist yet.

- [ ] **Step 3: Wire the module into lib.rs**

In `src/lib.rs`, add near the other `pub mod` declarations:

```rust
pub mod completion_spinner;
```

- [ ] **Step 4: Implement the spinner**

Replace the contents of `src/completion_spinner.rs` with:

```rust
//! Single-line braille-dot spinner drawn directly to `/dev/tty`.
//!
//! Used by shell-completion paths that need to run a slow operation
//! (e.g. `git fetch`) while giving the user a visible signal that
//! something is happening. The shell completion function itself is
//! synchronous from readline's perspective — the spinner is drawn by
//! this module (not the shell wrapper) by opening `/dev/tty` directly
//! and writing frames prefixed with `\r` so each frame overwrites the
//! previous one in place.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const FRAMES: [&str; 10] =
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Return the spinner frame for a given tick number.
///
/// The frame cycle wraps modulo 10 — ticks 0..10 map to FRAMES[0..10],
/// tick 10 wraps back to FRAMES[0], and so on.
pub(crate) fn frame_for_tick(tick: usize) -> &'static str {
    FRAMES[tick % FRAMES.len()]
}

/// A running spinner. Drop or call `stop()` to clear it.
pub struct Spinner {
    stop_flag: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Spinner {
    /// Start a spinner with the given label. Returns `None` if
    /// `/dev/tty` cannot be opened for writing — the caller should fall
    /// back to silent execution in that case.
    pub fn start(label: &str) -> Option<Self> {
        let mut tty = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/tty")
            .ok()?;
        let stop_flag = Arc::new(AtomicBool::new(false));
        let flag = stop_flag.clone();
        let label = label.to_string();
        let thread = thread::spawn(move || {
            let mut tick = 0usize;
            while !flag.load(Ordering::Relaxed) {
                let frame = frame_for_tick(tick);
                let _ = write!(tty, "\r{frame} {label}");
                let _ = tty.flush();
                tick += 1;
                thread::sleep(Duration::from_millis(100));
            }
            // Clear the line on exit.
            let _ = write!(tty, "\r\x1b[K");
            let _ = tty.flush();
        });
        Some(Self {
            stop_flag,
            thread: Some(thread),
        })
    }

    /// Stop the spinner and wait for the draw thread to exit.
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_frames_cycle_through_braille_dots() {
        let frames: Vec<&str> = (0..20).map(frame_for_tick).collect();
        assert_eq!(frames[0], "⠋");
        assert_eq!(frames[1], "⠙");
        assert_eq!(frames[9], "⠏");
        assert_eq!(frames[10], "⠋", "frame sequence must wrap at 10");
        assert_eq!(frames[19], "⠏");
    }
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p daft --lib completion_spinner
```

Expected: PASS.

- [ ] **Step 6: Run clippy**

```bash
mise run clippy
```

Expected: zero warnings.

- [ ] **Step 7: Commit**

```bash
git add src/completion_spinner.rs src/lib.rs
git commit -m "$(cat <<'EOF'
feat(completions): add /dev/tty braille-dot spinner

Single-line spinner module used by the daft-go fetch-on-miss path to
give the user a visible signal during git fetch. Opens /dev/tty
directly so it works from inside shell completion functions, which
are synchronous from readline's perspective. Returns None when the
tty cannot be opened (non-interactive, CI) so the caller falls back
to silent execution.

Unit-tested frame cycle; the actual tty drawing is verified by manual
test plan.
EOF
)"
```

---

### Task 8: Wire fetch-on-miss into `complete_daft_go()`

**Files:**

- Modify: `src/commands/complete.rs`

- [ ] **Step 1: Add the `--fetch-on-miss` CLI flag**

In the `Args` struct in `src/commands/complete.rs`, add:

```rust
#[arg(
    long,
    help = "When no local matches are found for the prefix, run `git fetch` \
            with a spinner and re-resolve"
)]
fetch_on_miss: bool,
```

In the `run()` function, update the call to `complete()` to pass the new flag
through. Extend the `complete()` function signature to accept it:

```rust
fn complete(
    command: &str,
    position: usize,
    word: &str,
    verbose: bool,
    fetch_on_miss: bool,
) -> Result<Vec<String>> {
```

And update the `("daft-go", 1)` arm from Task 4:

```rust
("daft-go", 1) => {
    let entries = complete_daft_go(word, fetch_on_miss)?;
    Ok(entries
        .iter()
        .map(|e| format!("{}\t{}\t{}", e.name, e.group.as_str(), e.description))
        .collect())
}
```

The parameter is named `fetch_on_miss` (no underscore prefix) in the signature
because it IS used by the `("daft-go", 1)` arm. Other arms simply don't
reference it — that's fine, Rust only warns on unused parameters if no branch
reads them, and the daft-go arm counts as a read. No warning, no underscore
needed.

- [ ] **Step 2: Implement the fetch-on-miss path in `complete_daft_go()`**

Replace the body of `complete_daft_go()` with:

```rust
pub(crate) fn complete_daft_go(
    prefix: &str,
    fetch_on_miss: bool,
) -> Result<Vec<CompletionEntry>> {
    let settings =
        crate::core::settings::DaftSettings::load().unwrap_or_default();
    let default_remote = if settings.multi_remote_enabled {
        settings.multi_remote_default.clone()
    } else {
        settings.remote.clone()
    };

    let collect = || -> Vec<CompletionEntry> {
        let worktrees = collect_go_worktrees();
        let local = collect_go_local_branches();
        let remote = collect_go_remote_branches();
        let current_branch = current_worktree_branch();
        build_go_completions(
            &worktrees,
            &local,
            &remote,
            current_branch.as_deref(),
            &default_remote,
            settings.multi_remote_enabled,
            prefix,
        )
    };

    let entries = collect();
    if !entries.is_empty()
        || !fetch_on_miss
        || !settings.go_fetch_on_miss
        || prefix.is_empty()
    {
        return Ok(entries);
    }

    // Fetch-on-miss gate: prefix non-empty, zero local matches, caller
    // opted in, setting not explicitly disabled.
    let git_common_dir = match crate::core::repo::get_git_common_dir() {
        Ok(d) => d,
        Err(_) => return Ok(entries),
    };
    let marker = git_common_dir.join("daft_complete_last_fetch");
    if !should_run_fetch(&marker, std::time::Duration::from_secs(30)) {
        return Ok(entries);
    }

    let spinner = crate::completion_spinner::Spinner::start(&format!(
        "Fetching refs from {default_remote}…"
    ));

    let fetch_result = std::process::Command::new("git")
        .args([
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-recurse-submodules",
            &default_remote,
        ])
        .output();
    // Best-effort: drop the result, just note completion.
    let _ = fetch_result;

    if let Some(s) = spinner {
        s.stop();
    }

    let _ = touch_fetch_marker(&marker);

    Ok(collect())
}
```

- [ ] **Step 3: Verify `get_git_common_dir` is re-exported where we need it**

```bash
grep -n "pub fn get_git_common_dir" src/core/repo.rs src/lib.rs
```

If missing from `src/core/repo.rs`, use the existing crate-level
`crate::get_git_common_dir()` wrapper instead.

- [ ] **Step 4: Run unit tests**

```bash
cargo test -p daft --lib commands::complete
cargo test -p daft --lib completion_spinner
```

Expected: all existing tests still PASS. No new unit tests in this task — the
fetch-on-miss behavior is a composition of already-tested primitives
(`should_run_fetch`, `touch_fetch_marker`, the spinner, and `complete_daft_go`).
The manual test plan (Task 16) verifies the end-to-end user-visible behavior.

- [ ] **Step 5: Manual smoke test**

```bash
cargo build
# Inside a repo where you know there are no local branches matching "zzzz":
./target/debug/daft __complete daft-go "zzzz" --position 1 --fetch-on-miss
```

Expected: the spinner line appears on the terminal for the duration of
`git fetch`, then disappears. Output is empty if the fetch did not produce any
matching refs. Running again within 30s should NOT show the spinner (cooldown).

- [ ] **Step 6: Commit**

```bash
git add src/commands/complete.rs
git commit -m "$(cat <<'EOF'
feat(completions): fetch-on-miss with spinner for daft-go

When `daft __complete daft-go` is called with `--fetch-on-miss` and
the user's prefix has no local matches, run `git fetch` with a
braille-dot spinner drawn to /dev/tty, then re-resolve. Gated by the
`daft.go.fetchOnMiss` setting (default on) and a 30-second cooldown
marker at `<git-common-dir>/daft_complete_last_fetch` to keep the
path from thrashing when the user types past a known-good prefix.
EOF
)"
```

---

### Task 9: Rewrite zsh generator for `daft-go` with grouped `_describe`

**Files:**

- Modify: `src/commands/completions/zsh.rs`

- [ ] **Step 1: Write failing unit tests for the new zsh shape**

Add to the tests module in `src/commands/completions/mod.rs`:

```rust
#[test]
fn zsh_daft_go_emits_describe_per_group() {
    let script = zsh::generate_zsh_completion_string("daft-go")
        .expect("generator must succeed");
    assert!(
        script.contains("_describe -t worktree"),
        "daft-go zsh completion must call _describe with the worktree tag"
    );
    assert!(
        script.contains("_describe -t local"),
        "daft-go zsh completion must call _describe with the local tag"
    );
    assert!(
        script.contains("_describe -t remote"),
        "daft-go zsh completion must call _describe with the remote tag"
    );
}

#[test]
fn zsh_daft_go_passes_fetch_on_miss_flag() {
    let script = zsh::generate_zsh_completion_string("daft-go")
        .expect("generator must succeed");
    assert!(
        script.contains("--fetch-on-miss"),
        "daft-go zsh completion must pass --fetch-on-miss to daft __complete"
    );
}
```

- [ ] **Step 2: Run and verify the new tests fail**

```bash
cargo test -p daft --lib commands::completions::tests::zsh_daft_go
```

Expected: the two new tests FAIL; the existing
`zsh_daft_go_gates_flags_on_leading_dash` test still PASSES (it just checks for
the guard).

- [ ] **Step 3: Add a bespoke `daft-go` code path in the zsh generator**

In `src/commands/completions/zsh.rs`, inside `generate_zsh_completion_string()`,
special-case `daft-go` before the generic branch-completion code. The cleanest
shape is to early-return a hand-written function body for `daft-go`:

```rust
pub(super) fn generate_zsh_completion_string(command_name: &str) -> Result<String> {
    if command_name == "daft-go" {
        return Ok(generate_zsh_daft_go_completion());
    }
    // ...existing generic body unchanged...
}
```

Add the new function at the bottom of the file:

```rust
fn generate_zsh_daft_go_completion() -> String {
    let cmd = crate::commands::checkout::GoArgs::command();
    let (all_flags, _, _) = extract_flags(&cmd);
    let flags_block: String = all_flags
        .iter()
        .map(|f| format!("            '{f}'\n"))
        .collect();

    format!(
        r#"#compdef daft-go

__daft_go_impl() {{
    local curword="${{words[$CURRENT]}}"
    local cword=$((CURRENT - 1))

    # Flag completions — only when the user has typed a leading dash.
    if [[ "$curword" == -* ]]; then
        local -a flags
        flags=(
{flags_block}        )
        compadd -a flags
        return
    fi

    # Collect grouped candidates from daft __complete (tab-separated).
    local -a raw wt_items local_items remote_items
    raw=(${{(f)"$(daft __complete daft-go "$curword" --position "$cword" --fetch-on-miss 2>/dev/null)"}})

    local line name rest group desc
    for line in "${{raw[@]}}"; do
        name="${{line%%$'\t'*}}"
        rest="${{line#*$'\t'}}"
        group="${{rest%%$'\t'*}}"
        desc="${{rest#*$'\t'}}"
        case "$group" in
            worktree) wt_items+=("$name:$desc") ;;
            local)    local_items+=("$name:$desc") ;;
            remote)   remote_items+=("$name:$desc") ;;
        esac
    done

    # Three _describe calls with empty string suppress group headers
    # but keep ordering: worktrees, then locals, then remotes.
    _describe -t worktree '' wt_items
    _describe -t local    '' local_items
    _describe -t remote   '' remote_items
}}

_daft_go() {{
    __daft_go_impl
}}

compdef _daft_go daft-go
"#
    )
}
```

Note: the `compdef` at the bottom is for direct invocation of `daft-go`.
Shortcut aliases are handled by the existing umbrella wiring in
`DAFT_ZSH_COMPLETIONS`, which is updated in Task 13.

- [ ] **Step 4: Run the new tests**

```bash
cargo test -p daft --lib commands::completions::tests::zsh_daft_go
```

Expected: all three zsh_daft_go tests PASS, including the existing
`zsh_daft_go_gates_flags_on_leading_dash` regression test (the guard remains
present in the new hand-written body).

- [ ] **Step 5: Run the full completion scenario suite**

```bash
mise run test:manual -- --ci completions
```

Expected: all scenarios pass. The `all-commands` scenario that generates zsh
completions for every command must still succeed — verify
`daft completions zsh --command=daft-go` returns exit 0.

- [ ] **Step 6: Commit**

```bash
git add src/commands/completions/zsh.rs src/commands/completions/mod.rs
git commit -m "$(cat <<'EOF'
feat(completions): grouped zsh completion for daft-go

Special-case daft-go in the zsh generator with a bespoke function
that parses tab-separated output from `daft __complete` and renders
three _describe calls (worktree, local, remote) with empty group
descriptions. This preserves visible ordering without showing
headers.

The flag-gating regression test from Task 1 continues to pass — the
new hand-written body still enforces the `$curword == -*` guard.
`--fetch-on-miss` is passed through to `daft __complete` so the
spinner path triggers when the user types a prefix with no local
match.
EOF
)"
```

---

### Task 10: Update bash generator for `daft-go` with ordered output

**Files:**

- Modify: `src/commands/completions/bash.rs`

- [ ] **Step 1: Write the failing unit test**

Add to the tests module in `src/commands/completions/mod.rs`:

```rust
#[test]
fn bash_daft_go_uses_nosort_and_fetch_on_miss() {
    let script = bash::generate_bash_completion_string("daft-go")
        .expect("generator must succeed");
    assert!(
        script.contains("--fetch-on-miss"),
        "daft-go bash completion must pass --fetch-on-miss to daft __complete"
    );
    assert!(
        script.contains("compopt -o nosort"),
        "daft-go bash completion must attempt compopt -o nosort to \
         preserve group ordering"
    );
    assert!(
        script.contains("cut -f1"),
        "daft-go bash completion must strip tab-separated group/desc \
         columns with cut -f1"
    );
}
```

- [ ] **Step 2: Run and verify it fails**

```bash
cargo test -p daft --lib commands::completions::tests::bash_daft_go_uses_nosort_and_fetch_on_miss
```

Expected: FAIL.

- [ ] **Step 3: Special-case `daft-go` in the bash generator**

In `src/commands/completions/bash.rs`, near the top of
`generate_bash_completion_string()`:

```rust
pub(super) fn generate_bash_completion_string(command_name: &str) -> Result<String> {
    if command_name == "daft-go" {
        return Ok(generate_bash_daft_go_completion());
    }
    // ...existing generic body unchanged...
}
```

Add at the bottom of the file:

```rust
fn generate_bash_daft_go_completion() -> String {
    let cmd = crate::commands::checkout::GoArgs::command();
    let (all_flags, _, _) = extract_flags(&cmd);
    let flags_joined = all_flags.join(" ");

    format!(
        r#"_daft_go() {{
    local cur prev words cword
    _init_completion || return

    if [[ "$cur" == -* ]]; then
        local flags="{flags_joined}"
        COMPREPLY=( $(compgen -W "$flags" -- "$cur") )
        return 0
    fi

    local raw
    raw=$(daft __complete daft-go "$cur" --position "$cword" --fetch-on-miss 2>/dev/null | cut -f1)
    if [[ -n "$raw" ]]; then
        COMPREPLY=( $(compgen -W "$raw" -- "$cur") )
        compopt -o nosort 2>/dev/null || true
        return 0
    fi
}}
complete -F _daft_go daft-go
"#
    )
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p daft --lib commands::completions::tests::bash_daft_go
```

Expected: PASS.

- [ ] **Step 5: Run the manual completion scenarios**

```bash
mise run test:manual -- --ci completions
```

Expected: all scenarios pass.

- [ ] **Step 6: Commit**

```bash
git add src/commands/completions/bash.rs src/commands/completions/mod.rs
git commit -m "$(cat <<'EOF'
feat(completions): grouped bash completion for daft-go

Special-case daft-go in the bash generator with a hand-written
function that strips tab-separated group/description columns with
`cut -f1` and preserves the ordering emitted by `daft __complete`
with best-effort `compopt -o nosort`. Flags are gated on leading
dash, and `--fetch-on-miss` is passed through for the spinner path.

Bash has no per-item description or color support — the visible
benefit over the old behavior is group ordering (worktrees first)
and no flag leaks.
EOF
)"
```

---

### Task 11: Update fish generator for `daft-go`

**Files:**

- Modify: `src/commands/completions/fish.rs`

- [ ] **Step 1: Write the failing unit test**

Add to the tests module in `src/commands/completions/mod.rs`:

```rust
#[test]
fn fish_daft_go_passes_fetch_on_miss_and_awk_reshuffles() {
    let script = fish::generate_fish_completion_string("daft-go")
        .expect("generator must succeed");
    assert!(
        script.contains("--fetch-on-miss"),
        "daft-go fish completion must pass --fetch-on-miss"
    );
    assert!(
        script.contains("awk"),
        "daft-go fish completion must reshuffle tab-separated columns via awk"
    );
}
```

Also check `DAFT_FISH_COMPLETIONS` since the existing daft-go completion line
lives in the umbrella string constant, not the per-command output:

```rust
#[test]
fn fish_daft_umbrella_passes_fetch_on_miss_for_go() {
    let fish_completions = fish::generate_daft_fish_completions();
    assert!(
        fish_completions.contains("daft __complete daft-go")
            && fish_completions.contains("--fetch-on-miss"),
        "daft go subcommand in umbrella fish completions must pass \
         --fetch-on-miss"
    );
}
```

- [ ] **Step 2: Run and verify the tests fail**

```bash
cargo test -p daft --lib commands::completions::tests::fish_daft_go
cargo test -p daft --lib commands::completions::tests::fish_daft_umbrella
```

Expected: both FAIL.

- [ ] **Step 3: Update the fish generator**

In `src/commands/completions/fish.rs`, find the per-command generator and
special-case `daft-go` similarly to zsh/bash — return a bespoke function body
that passes `--fetch-on-miss` and awk-reshuffles:

The per-command fish generator is relatively small. Replace the dynamic
branch-completion line it would otherwise emit with (this is the final fish text
— show this exact output when running
`daft completions fish --command=daft-go`):

```fish
complete -c daft-go -f -a "(daft __complete daft-go (commandline -ct) --position 1 --fetch-on-miss 2>/dev/null | awk -F'\t' '{printf \"%s\t%s · %s\n\", $1, $3, $2}')"
```

In Rust source, write this via `r#"..."#` raw strings (no special escaping) or
via `"..."` with `\\t` and `\\n` for the literal backslash escape sequences that
fish / awk see:

```rust
fn generate_fish_daft_go_line() -> &'static str {
    // Raw string avoids double-escaping. Fish sees literal
    // backslash-t and backslash-n inside the awk invocation, which
    // awk then interprets as tab and newline.
    r#"complete -c daft-go -f -a "(daft __complete daft-go (commandline -ct) --position 1 --fetch-on-miss 2>/dev/null | awk -F'\t' '{printf \"%s\t%s · %s\n\", $1, $3, $2}')""#
}
```

After implementing, verify with
`./target/debug/daft completions fish --command=daft-go` and confirm the output
matches the expected fish text above character-for-character.

Also update `DAFT_FISH_COMPLETIONS` in the same file — replace the existing
line:

```fish
complete -c daft -n '__fish_seen_subcommand_from go' -f -a "(daft __complete daft-go '' 2>/dev/null)"
```

with:

```fish
complete -c daft -n '__fish_seen_subcommand_from go' -f -a "(daft __complete daft-go (commandline -ct) --position 1 --fetch-on-miss 2>/dev/null | awk -F'\t' '{printf \"%s\t%s · %s\n\", $1, $3, $2}')"
```

- [ ] **Step 4: Run tests and verify they pass**

```bash
cargo test -p daft --lib commands::completions::tests::fish_daft_go
cargo test -p daft --lib commands::completions::tests::fish_daft_umbrella
```

Expected: PASS.

- [ ] **Step 5: Manual verification of the generated fish script**

```bash
cargo build
./target/debug/daft completions fish --command=daft-go
./target/debug/daft shell-init fish | grep -A1 "daft __complete daft-go"
```

Inspect the emitted lines — the `awk` invocation should be correctly escaped for
fish (single tab field separator, three-column reshuffle).

- [ ] **Step 6: Commit**

```bash
git add src/commands/completions/fish.rs src/commands/completions/mod.rs
git commit -m "$(cat <<'EOF'
feat(completions): grouped fish completion for daft-go

Pass `--fetch-on-miss` to `daft __complete daft-go` and reshuffle the
tab-separated output into fish's `name\tdescription` format where
the description is `<age> · <group>`. Fish colors descriptions
distinctly from the main completion, which gives implicit visual
grouping without needing an explicit color config.
EOF
)"
```

---

### Task 12: Emit zstyle coloring block for `daft-go`

**Files:**

- Modify: `src/commands/completions/zsh.rs`

- [ ] **Step 1: Write the failing unit test**

Add to `src/commands/completions/mod.rs`:

```rust
#[test]
fn zsh_daft_go_emits_zstyle_colors_per_group() {
    let script = zsh::generate_zsh_completion_string("daft-go")
        .expect("generator must succeed");
    assert!(
        script.contains(":*:daft-go:*:worktree"),
        "zsh daft-go must emit a zstyle line for the worktree group"
    );
    assert!(
        script.contains(":*:daft-go:*:local"),
        "zsh daft-go must emit a zstyle line for the local group"
    );
    assert!(
        script.contains(":*:daft-go:*:remote"),
        "zsh daft-go must emit a zstyle line for the remote group"
    );
}
```

- [ ] **Step 2: Run and verify it fails**

```bash
cargo test -p daft --lib commands::completions::tests::zsh_daft_go_emits_zstyle_colors_per_group
```

Expected: FAIL.

- [ ] **Step 3: Prepend the zstyle block to
      `generate_zsh_daft_go_completion()`**

Edit `generate_zsh_daft_go_completion()` in `src/commands/completions/zsh.rs` so
the returned string begins with a zstyle block before the `#compdef` line.
Replace the `format!` header:

```rust
format!(
    r#"#compdef daft-go
"#
```

with:

```rust
format!(
    r#"#compdef daft-go

# Per-group colors for `daft go` completions. Scoped to daft-go so the
# user's global completion colors are untouched. `zstyle -d` to disable.
zstyle ':completion:*:*:daft-go:*:worktree' list-colors '=(#b)(*)=0=1;32'
zstyle ':completion:*:*:daft-go:*:local'    list-colors '=(#b)(*)=0=1;34'
zstyle ':completion:*:*:daft-go:*:remote'   list-colors '=(#b)(*)=0=2;37'
"#
```

- [ ] **Step 4: Run the test**

```bash
cargo test -p daft --lib commands::completions::tests::zsh_daft_go_emits_zstyle_colors_per_group
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/commands/completions/zsh.rs src/commands/completions/mod.rs
git commit -m "$(cat <<'EOF'
feat(completions): color daft-go groups in zsh via zstyle

Emit a scoped zstyle block alongside the daft-go zsh completion so
worktree entries show in bright green, local in bright blue, and
remote in dim gray. Scoped to `:*:daft-go:*:<group>` so user-level
completion colors are unaffected. Users who dislike the defaults
can `zstyle -d` the entries.
EOF
)"
```

---

### Task 13: Wire `go` verb alias to the grouped completion in `DAFT_ZSH_COMPLETIONS`

**Why:** The zsh umbrella function in `DAFT_ZSH_COMPLETIONS` currently delegates
`daft go` to `__daft_go_impl`. That's already the right path name — Task 9 kept
the `__daft_go_impl` name intentionally. Verify there's no stale delegation to
adjust, and add a test that pins the contract.

**Files:**

- Modify: `src/commands/completions/mod.rs` (test only, unless a real bug is
  found)

- [ ] **Step 1: Write a test that locks in the alias wiring**

```rust
#[test]
fn zsh_umbrella_delegates_go_to_daft_go_impl() {
    let combined = format!(
        "{}\n{}",
        zsh::generate_zsh_completion_string("daft-go").unwrap(),
        zsh::DAFT_ZSH_COMPLETIONS,
    );
    // The umbrella must delegate `daft go` to the grouped implementation.
    assert!(
        combined.contains("__daft_go_impl"),
        "zsh umbrella must call __daft_go_impl for the `go` verb alias"
    );
}
```

- [ ] **Step 2: Run and verify the test result**

```bash
cargo test -p daft --lib commands::completions::tests::zsh_umbrella_delegates_go_to_daft_go_impl
```

If the test passes unchanged: the umbrella wiring already matches. Commit and
move on.

If the test fails: inspect `DAFT_ZSH_COMPLETIONS` in `zsh.rs` and update the
`go)` case inside the verb-alias `case` block to call `__daft_go_impl` (this is
the pre-existing shape — the call site already exists, so expect the test to
pass).

- [ ] **Step 3: Commit**

```bash
git add src/commands/completions/mod.rs
git commit -m "$(cat <<'EOF'
test(completions): pin zsh umbrella delegation for daft go

Lock in that `daft go` delegates to __daft_go_impl, which is the
new grouped completion function introduced in the daft-go overhaul.
EOF
)"
```

---

### Task 14: Docs — `daft go` CLI reference and config reference

**Files:**

- Modify: `docs/cli/daft-go.md`
- Modify: `docs/guide/configuration.md` (if the config reference page lists daft
  settings — verify first)

- [ ] **Step 1: Check the current state of `docs/cli/daft-go.md`**

```bash
cat docs/cli/daft-go.md
```

Note the existing structure — frontmatter, synopsis, options table, etc.

- [ ] **Step 2: Add a "Completion behavior" section**

Append to `docs/cli/daft-go.md` (before any trailing "See also" block):

````markdown
## Completion behavior

`daft go <TAB>` offers candidates grouped by type:

1. **Worktrees** — branches that already have a linked worktree. These are the
   primary navigation targets and are listed first. The branch you are currently
   sitting in is excluded.
2. **Local branches** — branches in `refs/heads/` that don't have a worktree
   yet. Selecting one of these will check it out into a new worktree.
3. **Remote branches** — branches in `refs/remotes/` that don't already exist
   locally. In single-remote mode the `<remote>/` prefix is stripped for
   readability; in multi-remote mode the full `<remote>/<branch>` form is
   preserved.

In zsh and fish, each candidate is annotated with the relative time of its last
commit (e.g. "3 days ago"). In zsh the three groups are colored distinctly —
worktrees in bright green, local branches in bright blue, remote branches in dim
gray. Bash shows a flat list in the same group order but without colors or
descriptions.

### Fetch-on-miss

If you type a prefix that doesn't match any local or already-fetched remote ref,
daft will run `git fetch` once (from the configured default remote) and
re-resolve, showing a spinner while the fetch runs. This lets you tab-complete
to a remote branch that exists upstream but hasn't been pulled yet.

The fetch path is gated by a 30-second cooldown per repository, so rapid
keystrokes won't trigger repeated fetches. To disable the feature entirely:

```sh
git config daft.go.fetchOnMiss false
```
````

````

- [ ] **Step 3: Regenerate the man page**

```bash
mise run man:gen
````

Expected: `man/daft-go.1` is updated. Verify with:

```bash
git diff man/daft-go.1
```

- [ ] **Step 4: Run docs lint**

```bash
mise run fmt
```

Expected: prettier formats the markdown files without errors.

- [ ] **Step 5: Commit**

```bash
git add docs/cli/daft-go.md man/daft-go.1
git commit -m "$(cat <<'EOF'
docs(cli): document daft go completion grouping and fetch-on-miss

Add a "Completion behavior" section to the daft-go reference page
covering the three-group layout, per-shell rendering, and the
fetch-on-miss spinner gated by daft.go.fetchOnMiss.
EOF
)"
```

---

### Task 15: Manual test plan for the spinner and visual rendering

**Files:**

- Create: `test-plans/go-completions.md`

- [ ] **Step 1: Inspect an existing test plan for format**

```bash
ls test-plans/
head -30 test-plans/*.md | head -40
```

Note the frontmatter and markdown-checklist shape.

- [ ] **Step 2: Write the plan**

Create `test-plans/go-completions.md`:

```markdown
---
branch: fix/go-completions
---

# daft go Completion Overhaul

## Setup

- [ ] Install the current build via `mise run dev`.
- [ ] In a test repo with ≥ 3 worktrees, ≥ 2 local-only branches, and ≥ 5
      remote-only branches, open a new zsh shell and a new bash shell.

## Group ordering

- [ ] `daft go <TAB>` in zsh lists worktrees first, then local branches, then
      remote branches, in that order.
- [ ] Same in bash (flat list but preserving the order).
- [ ] Same in fish.
- [ ] The current worktree's branch does NOT appear in the worktree group.

## Descriptions and colors

- [ ] zsh: each entry shows a relative age ("3 days ago", etc.) in the
      description column.
- [ ] zsh: worktree entries are bright green, local are bright blue, remote are
      dim gray.
- [ ] fish: each entry shows `<age> · <group>` in the description.
- [ ] bash: no descriptions, but no flags leaked into the branch list.

## Flag gating

- [ ] `daft go -<TAB>` in zsh shows ONLY flags, no branches.
- [ ] `daft go -<TAB>` in bash shows ONLY flags, no branches.
- [ ] `daft go <TAB>` (no dash) shows ONLY branches, no flags.

## Fetch-on-miss + spinner

- [ ] Find a remote-only branch that's NOT in your local `refs/remotes/` (ask
      someone to push a branch, or delete your local remote ref with
      `git update-ref -d refs/remotes/origin/<branch>`).
- [ ] Type `daft go <prefix-of-that-branch><TAB>`. Expected: a braille-dot
      spinner with "Fetching refs from origin…" appears on the terminal for the
      duration of the fetch, then clears.
- [ ] After the fetch completes, the completion list now includes the remote
      branch.
- [ ] Immediately type the same completion again. Expected: no spinner this time
      (cooldown).
- [ ] Wait 30+ seconds and retry. Expected: spinner reappears.
- [ ] `git config daft.go.fetchOnMiss false` — expected: spinner never appears,
      regardless of cooldown.
- [ ] Reset with `git config --unset daft.go.fetchOnMiss`.

## Multi-remote mode

- [ ] Enable multi-remote via `daft multi-remote enable`.
- [ ] `daft go <TAB>` — remote-only entries now show `<remote>/<branch>`
      verbatim instead of stripped form.
- [ ] Disable multi-remote again via `daft multi-remote disable`.

## Non-interactive invocation

- [ ] `daft __complete daft-go "" --position 1 | head` inside a repository emits
      tab-separated lines with three columns each.
- [ ] The same command with `--fetch-on-miss` and a non-matching prefix does NOT
      draw a spinner (no /dev/tty when stdout is piped) and still emits any
      matching output.
```

- [ ] **Step 3: Commit**

```bash
git add test-plans/go-completions.md
git commit -m "$(cat <<'EOF'
test(plan): add manual test plan for daft go completion overhaul

Covers group ordering, descriptions, colors, flag gating, the
fetch-on-miss spinner + cooldown, multi-remote naming, and
non-interactive invocation. Tied to branch fix/go-completions via
frontmatter for the sandbox `test-plan` command to pick up.
EOF
)"
```

---

### Task 16: Full CI simulation and ready-for-review

**Files:** none

- [ ] **Step 1: Run the full CI locally**

```bash
mise run ci
```

Expected: clippy, fmt check, unit tests, integration tests, man-page verify all
pass. The CLAUDE.md pre-commit requirements (`mise run fmt`, `mise run clippy`,
`mise run test:unit`) are a subset of `mise run ci`.

If anything fails, fix the root cause in a new commit — do not skip lefthook or
amend earlier commits in the chain.

- [ ] **Step 2: Run the full manual test plan**

Open the manual test plan from Task 15:

```bash
# Inside the daft sandbox if available, or manually:
cat test-plans/go-completions.md
```

Walk through each checkbox. If any manual check fails, fix the root cause and
add a new commit. Update the plan if the expected behavior changed.

- [ ] **Step 3: Push the branch (no auto-PR)**

```bash
git push -u origin fix/go-completions
```

Per CLAUDE.md: PRs target master and are always squash-merged. The user will
open the PR manually via `gh pr create` or the GitHub UI, with title
`feat(completions): overhaul daft go completion grouping` and the spec + plan
referenced in the body.
