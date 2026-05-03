---
title: daft-release-notes
description: Display release notes from the changelog
---

# daft release-notes

Display release notes from the changelog

## Description

Displays release notes from daft's changelog in a scrollable interface
using the system pager (similar to how git displays man pages).

By default, shows all release notes. Use the VERSION argument to show
notes for a specific version, or use --list to see a summary of all
available versions.

The pager can be navigated using standard less commands:
  - Space/Page Down: scroll down one page
  - b/Page Up: scroll up one page
  - /pattern: search for text
  - n: find next match
  - q: quit

## Usage

```
daft release-notes [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--format <FORMAT>` | Output format. Mutually exclusive with --template |  |
| `--template <STR>` | Tera template string. Mutually exclusive with --format |  |
| `--no-headers` | Omit header row (tsv/csv only) |  |
| `-l, --list` | List all versions without full notes |  |
| `-n, --latest <N>` | Show only the latest N releases (default: all) |  |
| `--no-pager` | Disable pager, print directly to stdout |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## Structured Output

`daft release-notes` supports machine-readable output via `--format`: `json`,
`yaml`, `toon`, `markdown`, plus `--template <tera>` for custom output.

```sh
# Markdown prose, paste-ready for GitHub release
daft release-notes 1.2.0 --format markdown

# Versions as JSON for tooling
daft release-notes --format json | jq '.[0].version'
```

See the [Output Formats guide](../reference/output-formats.md) for format details
and Tera syntax.

