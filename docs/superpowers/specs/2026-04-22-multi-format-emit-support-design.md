# Multi-Format Emit Support

## Problem

Today daft exposes a single `--json` boolean flag on two commands (`list`,
`release-notes`). It forecloses cleanly on additional formats, and several
common shell workflows end up reaching for `jq` to translate JSON into TSV or
line-delimited records. More broadly, several other daft commands (`hooks list`,
`layout list`, `shared status`, `multi-remote status`, `hooks run` listing mode)
currently have no machine-readable output at all — any scripting against them
requires fragile parsing of the human-oriented pretty output.

This design replaces `--json` with a unified `--format` flag (plus a
`--template` escape hatch) across seven daft commands, supporting seven
declarative formats and a Tera-based custom template mode.

## Goals

- Ship a single `--format <FORMAT>` flag used consistently across seven
  commands: `list`, `release-notes`, `hooks list`, `layout list`,
  `shared status`, `multi-remote status`, and `hooks run` in listing mode.
- Support seven declarative formats: `json`, `ndjson`, `tsv`, `csv`, `yaml`,
  `toon`, `markdown`. Plus a `--template` flag (Tera syntax) for custom output.
- Model each command's data explicitly as one of four shapes (Tabular, Document,
  Matrix, Sectioned). Formats dispatch per shape; unsupported (command, format)
  combinations error at runtime with a clear message.
- Remove `--json` entirely (hard break, no deprecation period).
- Document the format surface once in a shared guide; per-command reference
  pages link to it rather than duplicating format details.

## Non-goals

- No other commands gain `--format` in this PR. `doctor`, `shortcuts list`,
  `layout show`, and all side-effect commands (`clone`, `checkout`, `sync`,
  etc.) remain unchanged. Follow-up work.
- No unification of structured emit with the existing human-friendly pretty
  printers (`list`'s ANSI/interactive table, etc.). Those serve a different
  audience and stay untouched; `--format`/`--template` routes through a separate
  code path.
- No configurable CSV/TSV dialect. RFC 4180 CSV, standard tab-separated TSV, the
  only knob is `--no-headers`.
- No output-format config file or env var. Flag-only.
- No benchmarks. Worst-case payloads (`list` of ~200 worktrees) are well inside
  what every serializer in scope handles in tens of milliseconds.
- No deprecation shim for `--json`. Pre-1.0 project, small user base; users
  upgrade and change their scripts.

## CLI Surface

Three shared flags mounted on each of the seven commands:

```
--format <FORMAT>    Output format. Command-specific supported set.
--template <STR>     Tera template string. Mutually exclusive with --format.
--no-headers         Omit header row (tsv/csv only; warns otherwise).
```

`FORMAT` is one of: `json`, `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`.
No aliases, no `json-compact` (users who want compact JSON pipe through
`jq -c`).

### Per-command supported-format matrix

Derived from each command's shape (Section: Per-command shape mapping). Listed
here because it is the user-facing contract.

| Command               | Shape     | json | ndjson | tsv | csv | yaml | toon | markdown | template |
| --------------------- | --------- | :--: | :----: | :-: | :-: | :--: | :--: | :------: | :------: |
| `list`                | Tabular   |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `hooks list`          | Tabular   |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `layout list`         | Tabular   |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `release-notes`       | Document  |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |
| `shared status`       | Matrix    |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `multi-remote status` | Sectioned |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |
| `hooks run` (list)    | Sectioned |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |

Rationale for `—` cells:

- **Document + ndjson/tsv/csv:** release-notes is one document. NDJSON of one
  record is just compact JSON. TSV/CSV of prose has no meaning.
- **Sectioned + ndjson/tsv/csv:** sections are heterogeneous. Streaming as
  NDJSON or flattening to a single table silently misrepresents the data.

### Unsupported combo behavior

Runtime error, exit code 2, nothing written to stdout:

```
error: 'release-notes' does not support --format tsv
  supported formats: json, yaml, toon, markdown
```

`--help` for each command's `--format` lists that command's supported set
explicitly in its `long_help`. No global enum presenting formats the command
cannot produce.

## Architecture

All new code lives under `src/output/emit/`, sibling to the existing
`src/output/` modules. Structured emit is an output concern; keeping it
colocated with `cli.rs`, `tui.rs`, `buffering.rs` reinforces that.

```
src/output/emit/
  mod.rs            public API: emit(), EmitArgs, Format, EmitPayload, builders
  format.rs         Format enum (clap ValueEnum), Display
  args.rs           EmitArgs clap args for #[command(flatten)]
  payload.rs        EmitPayload enum + Table, Section, Matrix builders
  dispatch.rs       (shape × format) → fn; unsupported-combo error
  formats/
    json.rs         tabular, document, matrix, sectioned
    ndjson.rs       tabular, matrix
    tsv.rs          tabular, matrix (long-form)
    csv.rs          tabular, matrix (long-form)
    yaml.rs         all shapes
    toon.rs         all shapes
    markdown.rs     all shapes (table vs prose by shape)
    template.rs     all shapes, Tera
```

### Core types

```rust
// format.rs
#[derive(Copy, Clone, Debug, clap::ValueEnum)]
pub enum Format { Json, Ndjson, Tsv, Csv, Yaml, Toon, Markdown }

// args.rs
#[derive(clap::Args, Debug)]
pub struct EmitArgs {
    #[arg(long, value_enum, conflicts_with = "template")]
    pub format: Option<Format>,

    #[arg(long, conflicts_with = "format")]
    pub template: Option<String>,

    #[arg(long)]
    pub no_headers: bool,
}

// payload.rs
pub enum EmitPayload {
    Tabular(Table),
    Document(serde_json::Value),
    Matrix(Matrix),
    Sectioned(Vec<Section>),
}

pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

pub struct Matrix {
    row_key: String,      // e.g. "path"
    col_key: String,      // e.g. "worktree"
    cell_key: String,     // e.g. "state"
    rows: Vec<String>,
    cols: Vec<String>,
    cells: HashMap<(String, String), Cell>,
}

pub struct Section {
    name: String,
    payload: Box<EmitPayload>,
}

pub enum Cell { Str(String), Int(i64), Bool(bool), Null }
```

### Entry point

```rust
pub fn emit<W: Write>(
    payload: EmitPayload,
    args: &EmitArgs,
    writer: &mut W,
) -> Result<()>
```

`dispatch.rs` matches on `(payload.shape(), args.format_or_template())` and
either calls the matching `formats::<fmt>::<shape>(...)` function or returns
`EmitError::UnsupportedCombo { shape, format, supported }`. The supported matrix
is derived from the shape (not declared per command) — this guarantees the
documented support matrix and the runtime behavior cannot drift.

### Per-command integration

Minimal boilerplate. Example:

```rust
// commands/list.rs
#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    emit: EmitArgs,
    // ... existing list args
}

pub fn run(args: Args) -> Result<()> {
    let infos = collect_worktree_info(/* ... */)?;

    if args.emit.is_structured() {
        let payload = EmitPayload::Tabular(build_table(&infos, /* ... */));
        return emit::emit(payload, &args.emit, &mut io::stdout());
    }

    print_human_table(&infos, /* ... */)  // existing colored/interactive renderer
}
```

### Separation from human-friendly printers

When neither `--format` nor `--template` is present, commands use their existing
rendering (colors, sorts, per-column width, TUI-ish tables). When either is
present, emit routes to `emit::emit`. We are **not** merging, for example,
`list.rs::print_table` with `formats::markdown.rs::tabular` — they serve
different audiences (ANSI/interactive vs. plain-text/pipeable), and conflating
them would either degrade the human UX or bloat the markdown emitter.

### Separation of concerns

- `payload.rs` knows nothing about formats — pure data builders.
- `formats/*.rs` know nothing about commands — pure shape-to-string.
- `commands/*.rs` know nothing about serialization — build a payload, call emit.

### New dependencies

To add (verify each at implementation time):

- `serde_yaml` — YAML serialization.
- `csv` — RFC 4180 CSV with proper quoting.
- `tera` — template engine.
- `toon` — TOON serialization (confirm crate name/availability).

## Per-Command Shape Mapping

### `list`, `hooks list`, `layout list` — Tabular

- **`list`:** columns match the current `--json` field order, honoring
  `--columns`. One row per worktree. Size summary row handled per-format
  (below).
- **`hooks list`:** columns `repo_path`, `trust_level`, `remote_fingerprint`,
  `timestamp` (ISO-8601 UTC).
- **`layout list`:** columns `name`, `template`, `is_default`, `is_selected`.

### `release-notes` — Document

- **json:** preserve existing shape exactly.
- **yaml/toon:** `serde_yaml` / `toon` serialization of the same document.
- **markdown:** rendered prose (paste-ready for GitHub) — not a table. Requires
  release-notes to expose a markdown renderer; it likely has one internally (to
  verify at implementation).
- **template:** Tera context is the JSON document as a dict.

### `shared status` — Matrix

- **Rows:** shared file paths. **Columns:** worktree names. **Cells:** state
  enum (`linked`, `materialized`, `missing`, `broken`, `conflict`).
- **json/yaml/toon:** nested
  `{ "paths": { "<path>": { "<worktree>": "<state>" } } }`.
- **tsv/csv:** long-form — three columns `path`, `worktree`, `state`, one row
  per `(path, worktree)` pair. Wide-form would change column count with the
  worktree set and break awk scripts.
- **markdown:** wide-form pivot table (paths as rows, worktrees as columns).
  Humans reading markdown benefit from the pivot; column-instability is fine for
  human-only output.
- **ndjson:** one object per `(path, worktree)` pair, long-form.

### `multi-remote status`, `hooks run` (list) — Sectioned

- **`multi-remote status`:** two sections, both Tabular:
  - `remotes` — columns `name`, `url`, `is_default`.
  - `worktrees` — columns `branch`, `remote`, `path`.
- **`hooks run` listing mode:** one section per hook type (`post-clone`,
  `worktree-pre-create`, etc.). Each section Tabular with columns `job_name`,
  `description`, `tags` (comma-joined).
- **json/yaml/toon:** `{ "<section>": [...], "<section>": [...] }`.
- **markdown:** H2 per section, table per section.
- **template:** Tera context is `{ <section_name>: [...] }` or equivalent
  sectioned structure.

### `list` size-summary handling

The existing JSON output wraps entries in
`{"worktrees": [...], "total_size_bytes": N, "total_size": "..."}` when the size
column is active. New rule:

- **json/yaml/toon:** same wrapper as today.
- **ndjson:** one object per worktree, then a final
  `{"summary": {"total_size_bytes": N, "total_size": "..."}}` line. Documented;
  the `"summary"` key lets line processors discriminate.
- **tsv/csv:** trailing row with a `TOTAL` sentinel in the path cell and size
  filled in. Matches the convention used by `du`, `wc -l`, etc.
- **markdown:** last row uses a bold `**TOTAL**` in the path cell.

## Migration & Breaking Change

`--json` is removed from `list` and `release-notes`. No deprecation period.

### Code changes

- `src/commands/list.rs`: remove `json: bool`, remove the `if args.json` branch
  and the existing `print_json` fn; route via
  `emit::emit(EmitPayload::Tabular(…))`.
- `src/commands/release_notes.rs`: remove `json: bool`; route via
  `emit::emit(EmitPayload::Document(…))`.
- All seven commands: add `#[command(flatten)] emit: EmitArgs` and the
  `is_structured → emit` branch.
- Update `long_about` on `list` (current text references `--json`).

### Documentation

- **New shared page:** `docs/guide/output-formats.md`. Canonical reference for
  `--format`, `--template`, and `--no-headers`. Contains:
  - One-paragraph description and an example output for each of the seven
    declarative formats.
  - Tera syntax primer with three-to-four worked examples.
  - `--no-headers` semantics.
  - A "supported per command" matrix (mirror of the one in this spec).
  - Error-message examples for unsupported combos and bad templates.
- **Per-command reference pages** (`docs/cli/*.md` for the seven commands): add
  a short "Structured Output" section per page. Lists the supported formats for
  that command, shows one representative example (e.g.
  `daft list --format tsv | cut -f1,3`), and links to the shared guide for
  format semantics and template details. Intentionally thin — the shared guide
  is the source of truth.
- **`docs/guide/*`:** update any existing references to `--json`.
- **`SKILL.md`:** update per CLAUDE.md's explicit rule for feature changes that
  affect agent interaction.
- **Changelog / release notes** entry: one paragraph covering the breaking
  change and the migration (see below).

### Man pages

Regenerate via `mise run man:gen` and commit.

### Shell completions

Update the hardcoded string constants in
`src/commands/completions/{bash,zsh, fish,fig}.rs`: remove `--json` entries, add
`--format` with its value list. The per-flag clap autogeneration for the seven
commands handles the rest.

### Tests

- YAML scenarios in `tests/manual/scenarios/` that exercise `--json` → rewrite
  to `--format json`.
- Add new scenarios covering each command × format combination we support, plus
  the failure modes (Section: Testing strategy).
- Any Rust unit tests touching `args.json = true` → convert to the new emit
  path.

### Commit & release

- Single squash-merge PR, as per CLAUDE.md.
- Conventional Commit: `feat!: multi-format emit support with --format flag`.
- Commit body includes
  `BREAKING CHANGE: --json removed; use --format json instead` so release-plz
  tooling picks it up.
- Pre-1.0 project → breaking change bumps minor. The Release PR that release-plz
  opens will need a manual `Cargo.toml` bump from `0.N.x` → `0.(N+1).0`, per
  CLAUDE.md's rule about minor/major bumps.

### User-facing migration blurb

For the release notes:

> `--json` has been removed. Use `--format json` instead. Seven new formats are
> available: `json`, `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`. Add
> `--template '<tera>'` for custom output.

## Error Handling

### Parse-time (clap)

- `--format <invalid>` → clap `ValueEnum` rejection, exit 2.
- `--format X --template Y` together → clap `conflicts_with` error, exit 2.

### Emit-dispatch (runtime, before any stdout output)

- Unsupported (command, format) combo → stderr, exit 2, no stdout:
  ```
  error: 'release-notes' does not support --format tsv
    supported formats: json, yaml, toon, markdown
  ```
- `--no-headers` with a format that has no headers → stderr warning, proceed:
  ```
  warning: --no-headers has no effect with --format json (only tsv/csv)
  ```

### Template errors (runtime, after data collection, before stdout)

- Invalid Tera syntax → exit 2, stderr shows Tera's built-in line/column
  pointer.
- Template references a missing field → Tera runtime error with field name,
  exit 2.
- Empty input to a template that iterates → empty output, exit 0.

### Data-encoding edge cases

- **TSV:** tab characters and newlines inside a cell value are replaced with a
  single space. Documented as "TSV cells have whitespace normalized; use
  `--format csv` or `--format json` to preserve raw content." Lossy but the only
  convention that keeps awk pipelines working.
- **CSV:** handled by the `csv` crate per RFC 4180 — fields with
  commas/quotes/newlines are double-quoted, embedded quotes doubled. Not lossy.

### Serialization errors (TOON/YAML/etc.)

Propagated with the crate's error message. Stderr, exit 1. Unlikely in practice
for data that has already validated as a serde structure.

### Broken pipe

`stdout` returning `EPIPE` (user pipes to `head`, quits `less`) → exit 0
silently. `daft list --format json | head` must not error. Reuse daft's existing
broken-pipe handler if one exists; add one if not.

### Exit-code summary

| Condition                                         | Code |
| ------------------------------------------------- | ---- |
| Success                                           | 0    |
| Broken pipe                                       | 0    |
| Clap parse error, unsupported combo, bad template | 2    |
| Serialization or internal error                   | 1    |

### Non-behaviors

- No validation that `--template` output "looks like" a valid format. Users can
  produce anything; that is the point of templates.
- No partial output on failure for document/tabular/matrix/sectioned — either
  the full output emits or nothing does. NDJSON is the one exception: rows are
  written as built, so a mid-stream failure leaves partial output. That is
  consistent with every NDJSON producer in the ecosystem.

## Testing Strategy

### Unit tests (Rust, `cargo test`)

Each format file owns a `#[cfg(test)] mod tests` that asserts byte-exact output
against a fixed payload. Fixtures live in a shared `emit::test_fixtures` module:

- `fixture_tabular()` — three worktrees with known values, covering nulls,
  special characters, unicode.
- `fixture_document()` — release-notes doc with several sections.
- `fixture_matrix()` — two paths × three worktrees, with one `missing` cell.
- `fixture_sectioned()` — two sections of tabular data.

Byte-exact output assertions (using `insta` snapshot tests if daft already uses
it; plain string comparison otherwise — decide at implementation time) catch
silent drift, which is exactly the kind of break structured-output consumers
will notice weeks later.

Non-format-specific unit tests:

- `payload::Table::new + row(…)` builder invariants (column-count consistency,
  type coercion).
- `dispatch::emit` unsupported-combo error message — exact string, since it is
  user-facing.
- TSV whitespace normalization (input with tabs+newlines → single spaces).
- CSV RFC 4180 edge cases: comma, quote, newline inside a field.
- Broken-pipe handling: write to a closed pipe, assert exit 0 and no panic.
- Clap integration: `--format json --template X` → parse error.

### Integration tests (YAML scenarios)

One scenario file per command under `tests/manual/scenarios/`, covering:

1. Baseline: `--format json` parses as valid JSON (scenario pipes through
   `jq -e .`).
2. Per format the command supports: exit 0, non-empty stdout, cheap
   format-validity check (e.g. tsv → line count matches row count + header).
3. Unsupported combo: `release-notes --format tsv` → exit 2, stderr contains
   "does not support".
4. Removed flag: `--json` → exit 2 (clap "unknown argument"). Guards the
   breaking change.
5. Template: `--template '{{ some.field }}'` → exit 0, stdout matches expected
   substring.
6. Template error: malformed template → exit 2, stderr contains "template".

Approximate scenario count: ~60 (7 commands × ~8 checks, minus skips for
unsupported combinations).

### Regression test discipline

Per CLAUDE.md, every edge case fixed during implementation gets a scenario.
Particularly: cell values with tabs/newlines, unicode in commit subjects, very
long values, empty datasets.

### What is not tested

- Third-party crate correctness (serde, csv, tera, toon crates).
- Interactive TUI pathways — those are human UX and do not route through emit.

### CI gates

All pre-existing (via `mise run ci`): `fmt:check`, `clippy` (zero warnings),
`test:unit`, `test:integration`, `man:verify`. All must pass before merge.

### Bench / perf

Not required for this PR. Worst-case payloads are well within what the chosen
serializers handle in tens of milliseconds. Benchmark later only if someone
reports slowness.

## Open Follow-ups (explicitly out of scope)

- `--format` on `doctor`, `shortcuts list`, `layout show`, and any other command
  that produces structured-looking output.
- An output-format config file / env var for default format.
- A fuzzy/fzf-style `daft list` picker driven by structured output.
- Benchmark suite for emit throughput.
