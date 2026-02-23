# Go Auto-Start Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.

**Goal:** Add `--start`/`-s` flag and `daft.go.autoStart` config so `daft go`
can auto-create worktrees when a branch doesn't exist, with improved error
messages including diagnosis, start suggestion, and fuzzy-matched branch
suggestions.

**Architecture:** Introduce a `CheckoutError` typed error enum in the core
checkout module so the command layer can pattern-match on `BranchNotFound` vs
other errors. The command layer (`commands/checkout.rs`) handles the fallback to
`run_create_branch()` when auto-start is enabled, and renders a three-section
error message when it's not. Fuzzy matching reuses the existing Levenshtein
implementation in `src/suggest.rs`. A new `daft.go.autoStart` config key is
added to `DaftSettings`.

**Tech Stack:** Rust, clap (CLI args), git config (settings), existing
Levenshtein distance in `src/suggest.rs`

---

### Task 1: Introduce CheckoutError typed error

**Files:**

- Modify: `src/core/worktree/checkout.rs:1-12,50-55,111-117`

**Step 1: Define CheckoutError enum**

Add to the top of `src/core/worktree/checkout.rs`, after the existing imports:

```rust
use std::fmt;

/// Errors specific to the checkout operation.
#[derive(Debug)]
pub enum CheckoutError {
    /// The requested branch was not found locally or on the remote.
    BranchNotFound {
        branch: String,
        remote: String,
        fetch_failed: bool,
    },
    /// Any other error during checkout.
    Other(anyhow::Error),
}

impl fmt::Display for CheckoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BranchNotFound {
                branch, remote, ..
            } => {
                write!(
                    f,
                    "Branch '{branch}' does not exist locally or on remote '{remote}'"
                )
            }
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CheckoutError {}

impl From<anyhow::Error> for CheckoutError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}
```

**Step 2: Track fetch failures in the execute function**

In the `execute()` function, the `fetch_branch` helper currently returns nothing
(it warns and moves on). We need to capture whether the fetch failed so we can
report it in the error. Change `fetch_branch` to return a `bool` indicating
whether the fetch succeeded, and thread that into `BranchNotFound`.

Change `fetch_branch` signature to return `bool`:

```rust
fn fetch_branch(
    git: &GitCommand,
    remote_name: &str,
    branch_name: &str,
    sink: &mut impl ProgressSink,
) -> bool {
    // ... existing body ...
    // Return false if BOTH fetches failed, true otherwise
}
```

**Step 3: Change execute() return type**

Change the `execute()` function signature from `Result<CheckoutResult>` to
`Result<CheckoutResult, CheckoutError>`.

Replace the `anyhow::bail!` at line 112 with:

```rust
if !local_exists && !remote_exists {
    return Err(CheckoutError::BranchNotFound {
        branch: params.branch_name.clone(),
        remote: params.remote_name.clone(),
        fetch_failed,
    });
}
```

All other `anyhow::bail!` and `?` operators in `execute()` will auto-convert via
the `From<anyhow::Error>` impl.

**Step 4: Run `cargo check`**

Run: `cargo check 2>&1` Expected: Compilation errors in `commands/checkout.rs`
because `run_checkout` still expects `Result<CheckoutResult>`. This is expected
and will be fixed in Task 3.

**Step 5: Commit**

```bash
git add src/core/worktree/checkout.rs
git commit -m "refactor: introduce CheckoutError typed error for checkout"
```

---

### Task 2: Add daft.go.autoStart config and --start/-s flag

**Files:**

- Modify: `src/core/settings.rs:76-111,114-155,196-230,232-248,257-314,320-377`
- Modify: `src/commands/checkout.rs:45-93`

**Step 1: Add config key and default**

In `src/core/settings.rs`:

Add to `defaults` module:

```rust
/// Default value for go.autoStart setting.
pub const GO_AUTO_START: bool = false;
```

Add to `keys` module:

```rust
/// Config key for go.autoStart setting.
pub const GO_AUTO_START: &str = "daft.go.autoStart";
```

**Step 2: Add field to DaftSettings struct**

Add to `DaftSettings`:

```rust
/// Automatically create worktree when branch not found in go command.
pub go_auto_start: bool,
```

Add to `Default` impl:

```rust
go_auto_start: defaults::GO_AUTO_START,
```

**Step 3: Load in DaftSettings::load() and load_global()**

Add to `load()` (after the `use_gitoxide` block):

```rust
if let Some(value) = git.config_get(keys::GO_AUTO_START)? {
    settings.go_auto_start = parse_bool(&value, defaults::GO_AUTO_START);
}
```

Add the same pattern to `load_global()` using `config_get_global`.

**Step 4: Add --start/-s to Args**

In `src/commands/checkout.rs`, add to the `Args` struct:

```rust
#[arg(
    short = 's',
    long = "start",
    help = "Create a new worktree if the branch does not exist"
)]
start: bool,
```

**Step 5: Add unit test for default**

In `settings.rs` tests, add to `test_default_settings`:

```rust
assert!(!settings.go_auto_start);
```

**Step 6: Run tests**

Run: `cargo test --lib settings::tests 2>&1` Expected: PASS

**Step 7: Commit**

```bash
git add src/core/settings.rs src/commands/checkout.rs
git commit -m "feat: add daft.go.autoStart config and --start/-s flag"
```

---

### Task 3: Make suggest::find_similar_commands generic for branch suggestions

**Files:**

- Modify: `src/suggest.rs:66-91`

**Step 1: Refactor find_similar_commands to accept String slices too**

The existing `find_similar_commands` takes `&[&str]`. For branch names we'll
have `Vec<String>`. Add a companion function that works with owned strings:

```rust
/// Find strings similar to `input` from a list, sorted by edit distance.
///
/// Returns at most `max` suggestions. Only includes items whose edit distance
/// is within a reasonable threshold.
pub fn find_similar<'a>(input: &str, candidates: &'a [String], max: usize) -> Vec<&'a str> {
    let mut scored: Vec<(&str, usize)> = candidates
        .iter()
        .filter_map(|candidate| {
            let dist = levenshtein_distance(input, candidate);
            if dist == 0 {
                return None;
            }
            let max_len = input.len().max(candidate.len());
            let threshold = 3.max(max_len / 3);
            if dist <= threshold {
                Some((candidate.as_str(), dist))
            } else {
                None
            }
        })
        .collect();

    scored.sort_by_key(|&(_, dist)| dist);
    scored.truncate(max);
    scored.into_iter().map(|(s, _)| s).collect()
}
```

**Step 2: Add unit tests**

```rust
#[test]
fn test_find_similar_with_branches() {
    let branches = vec![
        "feature/auth".to_string(),
        "feature/auth-fix".to_string(),
        "develop".to_string(),
        "main".to_string(),
    ];
    let suggestions = find_similar("feature/auht", &branches, 5);
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0], "feature/auth");
}

#[test]
fn test_find_similar_no_match() {
    let branches = vec!["main".to_string(), "develop".to_string()];
    let suggestions = find_similar("completely-unrelated-xyzzy", &branches, 5);
    assert!(suggestions.is_empty());
}
```

**Step 3: Run tests**

Run: `cargo test --lib suggest::tests 2>&1` Expected: PASS

**Step 4: Commit**

```bash
git add src/suggest.rs
git commit -m "refactor: add generic find_similar for branch name suggestions"
```

---

### Task 4: Add branch listing helper for fuzzy matching

**Files:**

- Modify: `src/core/worktree/checkout.rs`

**Step 1: Add a function to collect all branch names**

Add a helper function in `checkout.rs` (after the existing helpers):

```rust
/// Collect all local and remote branch names for suggestion purposes.
pub fn collect_branch_names(git: &GitCommand, remote_name: &str) -> Vec<String> {
    let mut names = Vec::new();

    // Local branches
    if let Ok(output) = git.for_each_ref("%(refname:short)", "refs/heads/") {
        for line in output.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                names.push(trimmed.to_string());
            }
        }
    }

    // Remote branches (strip remote prefix)
    let remote_refs = format!("refs/remotes/{remote_name}/");
    if let Ok(output) = git.for_each_ref("%(refname:short)", &remote_refs) {
        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.ends_with("/HEAD") {
                continue;
            }
            // Strip "origin/" prefix to get just the branch name
            if let Some(branch) = trimmed.strip_prefix(&format!("{remote_name}/")) {
                if !names.contains(&branch.to_string()) {
                    names.push(branch.to_string());
                }
            }
        }
    }

    names
}
```

**Step 2: Run `cargo check`**

Run: `cargo check 2>&1` Expected: PASS (function is defined but not yet called)

**Step 3: Commit**

```bash
git add src/core/worktree/checkout.rs
git commit -m "feat: add collect_branch_names helper for fuzzy matching"
```

---

### Task 5: Wire up auto-start fallback and error rendering in command layer

**Files:**

- Modify: `src/commands/checkout.rs:110-184`

This is the main task that connects everything together.

**Step 1: Update run_checkout to return CheckoutError-aware result**

Change `run_checkout` to return `Result<(), CheckoutError>` where
`CheckoutError` is re-exported or used directly. The caller (`run_with_args`)
will handle the `BranchNotFound` variant.

Actually, the cleaner approach: change `run_checkout` to propagate
`CheckoutError` up, and handle it in `run_with_args`. Since `run_with_args`
returns `Result<()>` (anyhow), we handle `CheckoutError` explicitly there.

Refactor `run_checkout` to not use `?` on `checkout::execute()` — instead match
on the result:

```rust
fn run_checkout(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    // ... existing setup code (lines 145-166) ...

    let result = {
        let mut bridge = CommandBridge::new(output, executor);
        match checkout::execute(&params, &git, &project_root, &mut bridge) {
            Ok(result) => result,
            Err(e) => return Err(e.into()),  // converts CheckoutError to anyhow
        }
    };

    // ... rest unchanged ...
}
```

Wait — we need to return `CheckoutError` from `run_checkout` so the caller can
inspect it. Better approach: return `Result<(), checkout::CheckoutError>`:

```rust
fn run_checkout(
    args: &Args,
    settings: &DaftSettings,
    output: &mut dyn Output,
) -> Result<(), checkout::CheckoutError> {
    // ... existing params setup (unchanged) ...

    let result = {
        let mut bridge = CommandBridge::new(output, executor);
        checkout::execute(&params, &git, &project_root, &mut bridge)?
    };

    render_checkout_result(&result, output);
    let exec_result = crate::exec::run_exec_commands(&args.exec, output);
    output.cd_path(&result.cd_target);
    maybe_show_shell_hint(output)?;
    exec_result?;
    Ok(())
}
```

**Step 2: Update run_with_args to handle auto-start fallback**

```rust
fn run_with_args(args: Args) -> Result<()> {
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    if args.base_branch_name.is_some() && !args.create_branch {
        anyhow::bail!("<BASE_BRANCH_NAME> can only be used with -b/--create-branch");
    }

    let settings = DaftSettings::load()?;
    let autocd = settings.autocd && !args.no_cd;
    let config = OutputConfig::with_autocd(args.quiet, args.verbose, autocd);
    let mut output = CliOutput::new(config);
    let original_dir = get_current_directory()?;

    let result = if args.create_branch {
        run_create_branch(&args, &settings, &mut output)
    } else {
        match run_checkout(&args, &settings, &mut output) {
            Ok(()) => Ok(()),
            Err(checkout::CheckoutError::BranchNotFound {
                ref branch,
                ref remote,
                fetch_failed,
            }) => {
                let auto_start = args.start || settings.go_auto_start;
                if auto_start {
                    // Restore original dir before starting fresh
                    change_directory(&original_dir).ok();
                    output.result(&format!(
                        "Branch '{branch}' not found, creating new worktree..."
                    ));
                    run_create_branch(&args, &settings, &mut output)
                } else {
                    // Restore original dir before showing error
                    change_directory(&original_dir).ok();
                    render_branch_not_found_error(
                        branch,
                        remote,
                        fetch_failed,
                        &settings,
                        &output,
                    );
                    std::process::exit(1);
                }
            }
            Err(checkout::CheckoutError::Other(e)) => Err(e),
        }
    };

    if let Err(e) = result {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}
```

**Step 3: Implement render_branch_not_found_error**

```rust
fn render_branch_not_found_error(
    branch: &str,
    remote: &str,
    fetch_failed: bool,
    settings: &DaftSettings,
    _output: &dyn Output,
) {
    // Section 1: Diagnosis
    if fetch_failed {
        eprintln!(
            "error: Branch '{branch}' not found -- \
             could not reach remote '{remote}' to check"
        );
    } else {
        eprintln!(
            "error: Branch '{branch}' not found -- \
             it does not exist locally or on remote '{remote}'"
        );
    }

    // Section 2: Start suggestion (skip if fetch failed since start would also fail)
    if !fetch_failed {
        eprintln!();
        eprintln!(
            "  tip: Use `daft go --start {branch}` or `daft start {branch}` to create it"
        );
    }

    // Section 3: Fuzzy matches
    let git = GitCommand::new(true).with_gitoxide(settings.use_gitoxide);
    let all_branches = checkout::collect_branch_names(&git, remote);
    let suggestions = crate::suggest::find_similar(branch, &all_branches, 5);
    if !suggestions.is_empty() {
        eprintln!();
        if suggestions.len() == 1 {
            eprintln!("  Did you mean this?");
        } else {
            eprintln!("  Did you mean one of these?");
        }
        for s in &suggestions {
            eprintln!("    {s}");
        }
    }
}
```

**Step 4: Run `cargo check`**

Run: `cargo check 2>&1` Expected: PASS

**Step 5: Run unit tests**

Run: `cargo test --lib 2>&1` Expected: PASS

**Step 6: Commit**

```bash
git add src/commands/checkout.rs
git commit -m "feat: wire up auto-start fallback and three-section error in go command"
```

---

### Task 6: Add integration tests

**Files:**

- Modify: `tests/integration/test_checkout.sh`

**Step 1: Add test for improved error message**

```bash
# Test checkout nonexistent branch shows improved error
test_checkout_error_message() {
    local remote_repo=$(create_test_remote "test-repo-checkout-error-msg" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-error-msg"

    # Test error message has diagnosis section
    local output
    output=$(git-worktree-checkout nonexistent-branch-xyz 2>&1) && {
        log_error "Should have failed for nonexistent branch"
        return 1
    }

    # Check for diagnosis
    if ! echo "$output" | grep -q "not found"; then
        log_error "Error should contain 'not found'"
        echo "$output"
        return 1
    fi

    # Check for start suggestion
    if ! echo "$output" | grep -q "daft go --start"; then
        log_error "Error should suggest --start flag"
        echo "$output"
        return 1
    fi

    return 0
}
```

**Step 2: Add test for --start flag**

```bash
# Test checkout --start creates worktree when branch doesn't exist
test_checkout_start_flag() {
    local remote_repo=$(create_test_remote "test-repo-checkout-start" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-start"

    # Use --start with a new branch name
    git-worktree-checkout --start new-feature-branch || return 1

    # Verify worktree was created
    assert_directory_exists "new-feature-branch" || return 1
    assert_git_worktree "new-feature-branch" "new-feature-branch" || return 1

    return 0
}
```

**Step 3: Add test for --start with existing branch (should just go)**

```bash
# Test --start with existing branch just switches to it
test_checkout_start_existing_branch() {
    local remote_repo=$(create_test_remote "test-repo-checkout-start-existing" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-start-existing"

    # --start with an existing remote branch should just check it out
    git-worktree-checkout --start develop || return 1

    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1

    return 0
}
```

**Step 4: Add test for -s shorthand**

```bash
# Test -s shorthand works the same as --start
test_checkout_start_shorthand() {
    local remote_repo=$(create_test_remote "test-repo-checkout-start-short" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-start-short"

    git-worktree-checkout -s another-new-branch || return 1

    assert_directory_exists "another-new-branch" || return 1
    assert_git_worktree "another-new-branch" "another-new-branch" || return 1

    return 0
}
```

**Step 5: Add test for auto-start config**

```bash
# Test daft.go.autoStart config
test_checkout_auto_start_config() {
    local remote_repo=$(create_test_remote "test-repo-checkout-autostart" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-autostart"

    # Enable auto-start via local config
    cd main
    git config daft.go.autoStart true

    # Go to a nonexistent branch should auto-create
    git-worktree-checkout config-auto-branch || return 1

    cd ..
    assert_directory_exists "config-auto-branch" || return 1
    assert_git_worktree "config-auto-branch" "config-auto-branch" || return 1

    return 0
}
```

**Step 6: Add test for fuzzy suggestions**

```bash
# Test fuzzy branch suggestions in error message
test_checkout_fuzzy_suggestions() {
    local remote_repo=$(create_test_remote "test-repo-checkout-fuzzy" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-fuzzy"

    # Try a branch name close to "develop"
    local output
    output=$(git-worktree-checkout develp 2>&1) && {
        log_error "Should have failed for typo branch"
        return 1
    }

    # Check for fuzzy suggestion
    if ! echo "$output" | grep -q "Did you mean"; then
        log_error "Error should contain fuzzy suggestions"
        echo "$output"
        return 1
    fi

    if ! echo "$output" | grep -q "develop"; then
        log_error "Error should suggest 'develop'"
        echo "$output"
        return 1
    fi

    return 0
}
```

**Step 7: Register tests in run_checkout_tests**

Add to `run_checkout_tests()`:

```bash
# Auto-start and error message tests
run_test "checkout_error_message" "test_checkout_error_message"
run_test "checkout_start_flag" "test_checkout_start_flag"
run_test "checkout_start_existing_branch" "test_checkout_start_existing_branch"
run_test "checkout_start_shorthand" "test_checkout_start_shorthand"
run_test "checkout_auto_start_config" "test_checkout_auto_start_config"
run_test "checkout_fuzzy_suggestions" "test_checkout_fuzzy_suggestions"
```

**Step 8: Run integration tests**

Run: `mise run test:integration 2>&1` Expected: PASS (all existing + new tests)

**Step 9: Commit**

```bash
git add tests/integration/test_checkout.sh
git commit -m "test: add integration tests for go auto-start and error messages"
```

---

### Task 7: Update help text, man pages, and docs

**Files:**

- Modify: `src/commands/checkout.rs:20-44` (long_about)
- Modify: `src/commands/docs.rs` (if go command description needs update)
- Run: `mise run man:gen`

**Step 1: Update long_about in checkout.rs**

Add a paragraph about `--start`/`-s` and `daft.go.autoStart` to the long_about
string.

**Step 2: Regenerate man pages**

Run: `mise run man:gen`

**Step 3: Update docs site**

Update `docs/cli/daft-go.md` (or equivalent) to document:

- The `--start`/`-s` flag
- The `daft.go.autoStart` config option
- The improved error message with fuzzy suggestions

Update `docs/guide/` configuration page if one exists, to include
`daft.go.autoStart`.

**Step 4: Verify man pages**

Run: `mise run man:verify 2>&1` Expected: PASS

**Step 5: Commit**

```bash
git add src/commands/checkout.rs src/commands/docs.rs man/ docs/
git commit -m "docs: update help text, man pages, and docs for go auto-start"
```

---

### Task 8: Update settings module doc comment and SKILL.md

**Files:**

- Modify: `src/core/settings.rs:1-48` (module doc comment table)
- Modify: `SKILL.md` (if it exists)

**Step 1: Add daft.go.autoStart to settings doc comment table**

Add a row to the table in the module doc:

```
//! | `daft.go.autoStart` | `false` | Auto-create worktree when branch not found in go |
```

**Step 2: Update SKILL.md if it exists**

If `SKILL.md` exists, add documentation about the `--start`/`-s` flag and the
`daft.go.autoStart` config.

**Step 3: Run all checks**

Run: `mise run fmt && mise run clippy && mise run test:unit 2>&1` Expected: PASS

**Step 4: Commit**

```bash
git add src/core/settings.rs SKILL.md
git commit -m "docs: update settings doc comment and SKILL.md for go auto-start"
```

---

### Task 9: Final verification

**Step 1: Run full CI simulation**

Run: `mise run ci 2>&1` Expected: PASS

**Step 2: Manual smoke test**

Test the following scenarios manually in a temp directory:

1. `daft go nonexistent` — should show three-section error
2. `daft go --start new-branch` — should create worktree
3. `daft go -s new-branch-2` — should create worktree
4. `git config daft.go.autoStart true && daft go auto-branch` — should create
5. `daft go develp` — should show "Did you mean? develop"
