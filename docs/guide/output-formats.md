---
title: Output Formats
description: Structured output via --format and --template across daft commands
---

# Output Formats

Seven daft commands can emit machine-readable output via the shared `--format`
flag: `list`, `release-notes`, `hooks trust list`, `layout list`,
`shared status`, `multi-remote status`, and `hooks run` (when called without a
specific hook).

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

| Command               | json | ndjson | tsv | csv | yaml | toon | markdown | template |
| --------------------- | :--: | :----: | :-: | :-: | :--: | :--: | :------: | :------: |
| `list`                |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `hooks trust list`    |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `layout list`         |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `release-notes`       |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |
| `shared status`       |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `multi-remote status` |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |
| `hooks run` (listing) |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |

Requesting an unsupported combination prints a clear error listing the supported
formats for that command and exits 2.

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

## Templates

`--template` takes a [Tera](https://keats.github.io/tera/) template string. Tera
is a Jinja-inspired engine with good error messages and full control-flow.

### Context

- For `list`, `hooks trust list`, `layout list`, `shared status` — the template
  context exposes `items` as the array of rows.
- For `release-notes`, the context is the top-level document fields as
  variables.
- For `multi-remote status` and `hooks run` (listing) — the context is each
  section name as a variable binding the section's data.

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
