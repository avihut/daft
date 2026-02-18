# Doctor --fix and --dry-run Improvements

## Problem

`daft doctor` correctly detects issues but has gaps in fixing them:

1. Shortcut symlink warnings have no fix closure, so `--fix` silently skips
   them.
2. `--dry-run` shows the same warning text as regular output instead of concrete
   planned actions.
3. `--dry-run` cannot detect precondition failures upfront (e.g., directory not
   writable, conflicting file exists).

## Design

### New types (doctor/mod.rs)

Add `FixAction` to represent a single planned fix operation with pre-flight
validation:

```rust
pub struct FixAction {
    pub description: String,
    pub would_succeed: bool,
    pub failure_reason: Option<String>,
}

type DryRunFn = Box<dyn Fn() -> Vec<FixAction>>;
```

Add `dry_run_fix: Option<DryRunFn>` field to `CheckResult` with a
`.with_dry_run_fix()` builder method.

### Shortcut fix closures (doctor/installation.rs)

`check_shortcut_symlinks()` attaches `.with_fix()` and `.with_dry_run_fix()` to
each partially-installed style result. The fix creates missing symlinks using
the existing `create_symlink()` function. The dry-run checks each missing
shortcut for: directory writable, no conflicting non-daft file.

### Dry-run functions for all fixable checks

| Check                | Dry-run validates                                   |
| -------------------- | --------------------------------------------------- |
| Command symlinks     | install dir writable, no conflicting non-daft files |
| Shortcut symlinks    | install dir writable, no conflicting non-daft files |
| Worktree consistency | git available, in a git repo                        |
| Fetch refspec        | origin remote exists                                |
| Remote HEAD          | origin remote exists                                |
| Hooks executable     | files exist, metadata writable                      |
| Deprecated names     | target name doesn't already exist                   |

### Updated preview_fixes() (commands/doctor.rs)

Calls dry-run functions when available and displays per-action results with
success/failure indicators. Falls back to suggestion text when no dry-run
function is provided.

Example output:

```
Would fix 3 issue(s):

  [!] Command symlinks -- 9/11 installed, 2 missing
      + Create symlink git-worktree-flow-adopt -> daft in /usr/local/bin
      + Create symlink git-worktree-flow-eject -> daft in /usr/local/bin

  [!] Shortcuts: git style -- 6/8 installed, 2 missing
      + Create symlink gwtco -> daft in /usr/local/bin
      x Create symlink gwtcb -> daft in /usr/local/bin
        /usr/local/bin/gwtcb exists and is not a daft symlink
```

### Not auto-fixable (suggestion-only)

- Binary in PATH
- Git installation
- Man pages
- Shell integration
- Shell wrappers
- Trust level
