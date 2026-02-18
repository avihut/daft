# Structural Push Hook Bypass & Graceful Failure Handling

## Context

Daft pushes to remotes in three commands as a side-effect of worktree
management:

- `checkout-branch`: pushes new branch to set upstream tracking
- `branch-delete`: pushes `--delete` to remove remote branch
- `branch move --push`: pushes existing branch to new remote

These are structural operations — the user's intent is worktree management, not
"push my code." Git's pre-push hooks (which often run test suites) fire on all
of these, causing friction and unexpected slowdowns.

Additionally, `checkout-branch` treats push failure as fatal, erroring out while
leaving the already-created worktree behind with no upstream.

## Design

### Principle

Daft's push operations are structural. Pre-push hooks should be skipped, and
push failures should never destroy local work.

### Change 1: Skip pre-push hooks with `--no-verify`

Add `--no-verify` to all push operations in the shared `GitCommand` methods:

- `push_set_upstream` in `src/git.rs`
- `push_delete` in `src/git.rs`
- Raw `push --delete` in `src/commands/branch.rs` (`delete_remote_branch`)

Applied at the method level since all daft pushes are structural.

### Change 2: Non-fatal push failure in `checkout-branch`

Change `src/commands/checkout_branch.rs` so push failure produces a warning with
a recovery command instead of returning an error:

```
Could not push 'branch' to 'origin': <error>.
The worktree is ready locally. Push manually with: git push -u origin branch
```

This aligns with `branch move` and `branch-delete`, which already treat push
failures as non-fatal.

### What stays the same

- `branch-delete` push_delete failure handling (already non-fatal)
- `branch move --push` failure handling (already non-fatal)
- `daft.checkout.push = false` (still skips push entirely)
- Daft's own lifecycle hooks (post-create, etc.) are unaffected

## Files changed

| File                              | Change                                                 |
| --------------------------------- | ------------------------------------------------------ |
| `src/git.rs`                      | `--no-verify` on `push_set_upstream` and `push_delete` |
| `src/commands/branch.rs`          | `--no-verify` on raw `push --delete`                   |
| `src/commands/checkout_branch.rs` | Push failure becomes warning, not fatal                |
| Integration tests                 | Update assertions for push failure behavior            |
| Documentation                     | Note that daft skips pre-push hooks                    |
