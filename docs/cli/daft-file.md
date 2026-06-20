---
title: daft-file
description: Merge a source daft.yml into a target daft.yml
---

# daft file

Merge a source daft.yml into a target daft.yml

## Description

Merge SOURCE into TARGET. When the source is a worktree-root daft file with
seed provenance (daft recorded what it wrote there), the merge is THREE-WAY
against that seed: only keys the source genuinely refined move into the
target, a key-level preview is printed first, and the target is backed up to
<git-common-dir>/.daft/backups/file-merge/ before writing. Keys changed on
both sides are conflicts: pick a side at the interactive prompt, pass -y to
take the source's values, or the command aborts non-zero listing the keys.

Without provenance the legacy two-way merge applies: source wins on
conflicts, new hook sections are added wholesale, and when TARGET is
untracked (visitor file) you are prompted for confirmation unless
--yes / --force is passed.

When TARGET is omitted, daft.yml in the current directory is used.

By default the source file is deleted after a successful merge
(--keep-source retains it and re-seeds it as consolidated).

## Usage

```
daft file [OPTIONS] <FIRST> [SECOND]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<FIRST>` | Target file to merge INTO, or source file when TARGET is omitted | Yes |
| `<SECOND>` | Source file to merge FROM (optional; when omitted, FIRST is the source and the target defaults to daft.yml in the current directory) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--keep-source` | Keep the source file after merging (do not delete it) |  |
| `-y, --yes` | Skip confirmation prompt when the target is untracked |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

