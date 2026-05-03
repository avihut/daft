---
title: git-worktree-merge
description: Merge branches across worktrees
---

# git worktree-merge

Merge branches across worktrees

::: tip
This command is also available as `daft merge`. See [daft merge](./daft-merge.md).
:::

## Description

Merges one or more source branches into a target worktree's branch.

When --into is omitted, the target is the current worktree's branch,
mirroring `git merge`. When --into <target> is supplied, the merge is
performed against that worktree's branch from wherever you are.

Multiple sources invoke git's octopus strategy, announced explicitly.

Finish commands (--abort, --continue, --quit) take an optional positional
<worktree|branch>; default to the current worktree's branch.

## Usage

```
git worktree-merge [OPTIONS] [SOURCE_OR_TARGET]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<SOURCE_OR_TARGET>` | Source branches/commits to merge (start mode), OR optional target worktree/branch for --abort / --continue / --quit (finish mode; max one positional) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--into <TARGET>` | Target worktree/branch. Defaults to the current worktree's branch |  |
| `--abort` | Abort an in-progress merge in the named worktree (defaults to CWD) |  |
| `--continue` | Continue an in-progress merge in the named worktree (defaults to CWD) |  |
| `--quit` | Quit an in-progress merge without resetting the index (defaults to CWD) |  |
| `-m <MSG>` | Commit message for the merge commit (mirrors `git merge -m`) |  |
| `-F, --file <FILE>` | Read the commit message from FILE (mirrors `git merge -F`) |  |
| `--edit` | Launch the editor to edit the merge commit message |  |
| `--no-edit` | Accept the auto-generated merge commit message without editing |  |
| `--cleanup <MODE>` | Message cleanup mode (mirrors `git merge --cleanup`) |  |
| `--merge` | Explicit merge style — always create a merge commit. This is the default; the flag exists for canceling a config-set default style |  |
| `--squash` | Squash style — collapse source's commits into one squashed commit on target |  |
| `--rebase` | Rebase style — rebase source onto target, then fast-forward (linear, preserves commits) |  |
| `--rebase-merge` | Rebase-merge style — rebase source onto target, then create a merge commit |  |
| `--commit` | Automatically create the merge commit after a successful merge |  |
| `--no-commit` | Leave the merge staged without committing |  |
| `--signoff` | Add a Signed-off-by trailer to the merge commit |  |
| `--no-signoff` | Explicitly disable signoff (cancel a config default) |  |
| `-s, --strategy <STRAT>` | Merge strategy to use (e.g. `ours`, `recursive`, `octopus`) |  |
| `-X, --strategy-option <OPT>` | Strategy-specific option (repeatable; mirrors `git merge -X`) |  |
| `-S, --gpg-sign <KEYID>` | GPG-sign the merge commit. Accepts an optional KEYID; omit to use the default key |  |
| `--no-gpg-sign` | Do not GPG-sign the merge commit (cancels `commit.gpgsign` config) |  |
| `--verify-signatures` | Verify that the tip commit of the source is signed with a valid key |  |
| `--no-verify-signatures` | Do not verify signatures on the source tip commit |  |
| `--allow-unrelated-histories` | Allow merging histories that share no common ancestor |  |
| `--stat` | Show a diffstat at the end of the merge |  |
| `-n, --no-stat` | Suppress the diffstat at the end of the merge |  |
| `--adopt-target` | When the target has no worktree and the merge is not a pure fast-forward, create an ephemeral worktree to perform the merge without prompting |  |
| `--no-adopt-target` | When the target has no worktree and the merge is not a pure fast-forward, refuse without prompting |  |
| `-y, --yes` | Auto-accept interactive prompts. Implies --adopt-target when neither --adopt-target nor --no-adopt-target is supplied. Future-proofs any new prompts we add |  |
| `-r, --remove-branch` | Remove the source worktree and delete the source branch. The local/remote behavior follows `branch.deleteRemote` (defaults to local-only) |  |
| `--keep-branch` | Explicit keep — for canceling a config-set `merge.cleanup = remove-branch` |  |
| `--set-default` | Write the resolved style/cleanup choices to `git config --local` after the merge succeeds |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-list](./git-worktree-list.md)
- [git-worktree-carry](./git-worktree-carry.md)
- [git-worktree-sync](./git-worktree-sync.md)
- [git-worktree-flow-adopt](./git-worktree-flow-adopt.md)

