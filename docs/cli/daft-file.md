---
title: daft-file
description: Merge a source daft.yml into a target daft.yml
---

# daft file

Merge a source daft.yml into a target daft.yml

## Description

Merge SOURCE into TARGET using the same recursive YAML merge that daft uses
at load time: source wins on conflicts, new hook sections are added wholesale.

When TARGET is omitted, daft.yml in the current directory is used.

By default the source file is deleted after a successful merge.
When TARGET is untracked (visitor file) you are prompted for confirmation
unless --yes / --force is passed.

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

