---
title: git-worktree-merge
description: Merge branches across worktrees
---

# git worktree-merge

Merge branches across worktrees

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
| `--ff` | Allow fast-forward merges (git's default behavior) |  |
| `--no-ff` | Always create a merge commit, even when fast-forward is possible |  |
| `--ff-only` | Refuse to merge if fast-forward is not possible |  |
| `--squash` | Squash the source's changes into a single staged diff, without creating a merge commit |  |
| `--no-squash` | Explicitly disable squash (cancel a config default of `merge.squash`) |  |
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
| `-v, --verbose` | Be verbose; show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

