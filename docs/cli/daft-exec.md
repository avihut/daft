---
title: daft exec
description: Run a command across one or more worktrees
---

# daft exec

Run a command across one or more worktrees without changing your current directory.

::: tip
This is the short form of [`git worktree-exec`](./git-worktree-exec.md). The two
are equivalent; this page mirrors the reference for convenience.
:::

See [`git worktree-exec`](./git-worktree-exec.md) for the full CLI reference.

## Quick examples

```bash
daft exec --all -- cargo test
daft exec feat/auth 'feat/ui-*' -- pnpm lint
daft exec --all -x 'mise install' -x 'pnpm build' -x 'pnpm test'
daft exec feat/auth -- claude
```

## See Also

- [git worktree-exec](./git-worktree-exec.md) for full options reference
- [Running commands across worktrees](/worktrees/running-commands) for the narrative guide
