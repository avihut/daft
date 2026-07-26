---
title: Output Formats
description: Structured output via --format and --template across daft commands
---

# Output Formats

Eleven daft commands can emit machine-readable output via the shared `--format`
flag: `list`, `merge`, `release-notes`, `hooks jobs`, `hooks trust list`,
`hooks run` (when called without a specific hook), `layout list`, `repo list`,
`repo info`, `shared status`, and `multi-remote status`.

## Flags

- `--format <FORMAT>` — pick one of: `json`, `ndjson`, `tsv`, `csv`, `yaml`,
  `toon`, `markdown`. Mutually exclusive with `--template`.
- `--template <STR>` — render output with a
  [Tera](https://keats.github.io/tera/) template. Mutually exclusive with
  `--format`.
- `--no-headers` — omit the header row in `tsv` / `csv` output. Ignored (with a
  warning) for other formats.

## Per-command support

Not every format applies to every command. The supported sets:

| Command                 | json | ndjson | tsv | csv | yaml | toon | markdown | template |
| ----------------------- | :--: | :----: | :-: | :-: | :--: | :--: | :------: | :------: |
| `list`                  |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `hooks jobs`            |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `hooks trust list`      |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `layout list`           |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `repo list`             |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `repo list --worktrees` |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |
| `shared status`         |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `merge`                 |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |
| `release-notes`         |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |
| `repo info`             |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |
| `multi-remote status`   |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |
| `hooks run` (listing)   |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |

The split is not per-command taste: it follows the payload's shape. Commands
whose output is a list of uniform rows support every format; those whose output
is a document (nested, or several differently-shaped lists) support only the
four that can represent nesting. There are no rows to put in a `csv` for a
payload that is not row-shaped.

Requesting an unsupported combination prints a clear error naming the formats
that command does support, and exits 1. (Exit 2 is clap's parse-error code —
what you get from an unknown format name, or from `--format` on a subcommand
that takes none.)

## Formats

### json

Pretty-printed JSON, two-space indent. Safe to pipe into `jq`. Use `jq -c .` for
single-line output.

### ndjson

One JSON object per line. Streams naturally into line processors like `jq`,
`fq`, `mlr`, or `awk`. Tabular commands emit one row per line; matrix commands
emit one object per populated cell in long form.

### tsv

Tab-separated rows with a header row unless `--no-headers` is set. Cell values
containing tabs or newlines are replaced with a single space before emission —
TSV has no standard escaping, and preserving those bytes would break awk
pipelines. If you need raw content, use `csv` or `json`.

### csv

RFC 4180 CSV. Fields with commas, quotes, or newlines are double-quoted; quotes
inside a field are escaped by doubling.

### yaml

YAML 1.2. Preserves nested structure; good for configs and human reading.

### toon

[TOON](https://github.com/toon-format/spec) — token-efficient structured data,
designed for piping into LLM context. Roughly 30-50% fewer tokens than
equivalent JSON.

### markdown

For tabular commands, a GitHub-flavored markdown table. For `release-notes`, the
rendered prose notes (ready to paste into a GitHub release). For
`shared status`, a wide-form pivot table for quick visual reading.

## The merge verdict

`daft merge --format <fmt>` writes one document describing how the merge ended.
It is available in start mode only — the finish modes (`--abort`, `--continue`,
`--quit`) reject the flag at parse time rather than accepting it and emitting
nothing.

Sections:

| Section     | Contents                                                                        |
| ----------- | ------------------------------------------------------------------------------- |
| `verdict`   | One row: the outcome and what was certified.                                    |
| `sources`   | One row per source branch, with the SHA the gate pinned.                        |
| `conflicts` | One row per conflicted path. Present only when the merge conflicted.            |
| `jobs`      | The gate's per-job rows, identical in shape to `daft hooks jobs --format json`. |

`status` is the field to branch on:

| Status           | Meaning                                                         | Exit |
| ---------------- | --------------------------------------------------------------- | ---- |
| `landed`         | The merge landed.                                               | 0    |
| `up-to-date`     | The target already contained every source; nothing ran.         | 0    |
| `squash-staged`  | `--squash --no-commit`: staged on the target, not committed.    | 0    |
| `refused`        | Gate **policy** stopped the merge; the repository is untouched. | 1    |
| `gate-failed`    | The gate ran and a check came back red.                         | 1    |
| `conflicted`     | git left the target worktree conflicted.                        | 1    |
| `commit-aborted` | The squash-commit step was aborted.                             | 1    |
| `failed`         | Something else broke mid-flight.                                | 1    |

The three unhappy outcomes are distinct because they call for different
responses: `refused` means rebase the track and retry, `gate-failed` means fix
the code (the failing jobs are in the `jobs` section), and `failed` means a
human should look. When `status` is `refused`, the `refusal` field carries a
stable token naming the policy — `not-fast-forward`, `source-worktree-dirty`,
`source-advanced-during-gate`, and so on.

A merge that landed but whose cleanup was refused reports `status: landed` with
`cleanup: refused` and the reason in `message`, and still exits 1: both facts
are true, and the exit code keeps behaving as it did before the flag existed.

`pre_merge_invocation` / `post_merge_invocation` are the log-store ids for the
hook runs. They are the join key into `daft hooks jobs` — useful when you want
row-oriented job data (`--format tsv`), or the captured output:

```sh
daft merge track --format json | jq -r '.verdict[0].pre_merge_invocation'
daft hooks jobs --last --hook pre-merge --format tsv
```

The ids are present whenever the gate ran, including when it failed. If ids are
present but no `jobs` section is, the log store could not be read — the merge
result itself is still authoritative.

## Templates

`--template` takes a [Tera](https://keats.github.io/tera/) template string. Tera
is a Jinja-inspired engine with good error messages and full control-flow.

### Context

- For `list`, `hooks jobs`, `hooks trust list`, `layout list`, `repo list`,
  `shared status` — the template context exposes `items` as the array of rows.
- For `release-notes` and `repo info`, the context is the top-level document
  fields as variables.
- For `merge`, `multi-remote status`, and `hooks run` (listing) — the context is
  each section name as a variable binding the section's data.

### Examples

Print one branch per line from `daft list`:

```sh
daft list --template '{% for r in items %}{{ r.name }}
{% endfor %}'
```

Custom summary:

```sh
daft list --template '{{ items | length }} worktrees'
```

Release titles only:

```sh
daft release-notes --template '{% for r in releases %}{{ r.version }}
{% endfor %}'
```

Syntax errors in your template print a line-and-column pointer to stderr and
exit 2.

## Errors

```
error: 'release-notes' does not support --format tsv
  supported formats: json, yaml, toon, markdown

error: invalid value 'bogus' for '--format <FORMAT>'
  [possible values: json, ndjson, tsv, csv, yaml, toon, markdown]

error: the argument '--format <FORMAT>' cannot be used with '--template <STR>'
```
