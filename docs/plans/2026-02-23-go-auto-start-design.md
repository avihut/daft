# Go Auto-Start Design

## Summary

Add the ability for `daft go` to automatically create a new worktree and branch
when the target branch doesn't exist, via `--start`/`-s` flag or
`daft.go.autoStart` config. Also improve the error message when a branch isn't
found.

## Typed Error for Checkout

The core `checkout::execute()` currently uses `anyhow::bail!` when a branch
isn't found. Introduce a `CheckoutError` enum with structured data:

```rust
pub enum CheckoutError {
    BranchNotFound {
        branch: String,
        remote: String,
        fetch_failed: bool,
    },
    Other(anyhow::Error),
}
```

`execute()` returns `Result<CheckoutResult, CheckoutError>` instead of
`Result<CheckoutResult>`. The command layer pattern-matches to decide the next
action.

## Three-Section Error Message

When `go` fails with `BranchNotFound` and auto-start is not enabled:

```
Error: Branch 'foo' not found -- it does not exist locally or on remote 'origin'

  tip: Use `daft go --start foo` or `daft start foo` to create it

  Did you mean?
    - feature/foo-bar
    - fix/foo-baz
```

### Section 1: Diagnosis

Adapts based on context:

- "does not exist locally or on remote 'origin'" (normal case)
- "could not reach remote 'origin' to check" (if fetch failed)

### Section 2: Start Suggestion

Only shown if starting a new branch is viable (fetch didn't fail for a reason
that would also prevent `start`).

### Section 3: Fuzzy Matches

Compare user input against all local + remote branches using string similarity
(`strsim` crate, Jaro-Winkler distance). Only shown if matches above a ~0.7
threshold are found, capped at 5 suggestions.

## --start / -s Flag

Add `--start` / `-s` to the `Args` struct in `checkout.rs`. When present, if
`go` fails with `BranchNotFound`, fall back to `run_create_branch()` instead of
showing the error. Print an info line first:

```
Branch 'foo' not found, creating new worktree...
```

## daft.go.autoStart Config

Add `daft.go.autoStart` (default: `false`) to `DaftSettings`. When enabled, `go`
behaves as if `--start` was passed.

## Precedence and Flow

In `run_with_args()`:

1. Call `run_checkout()`
2. If `BranchNotFound` and (`args.start` || `settings.go_auto_start`):
   - Print info line
   - Call `run_create_branch()` with the same branch name
3. If `BranchNotFound` without auto-start:
   - Show the three-section error message
4. Other errors or success: handle normally

No `--no-start` flag needed. The info line protects against accidental creation
from typos.
