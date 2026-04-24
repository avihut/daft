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
git worktree-merge [OPTIONS] [SOURCE]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<SOURCE>` | Source branches/commits to merge. Two or more invoke octopus | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-v, --verbose` | Be verbose; show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

