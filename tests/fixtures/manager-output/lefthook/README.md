# lefthook piped-output fixtures

Captured **real output from every released lefthook 2.x** (v2.0.0 through
v2.1.10, all 28 releases), piped exactly the way daft sees it: git runs the
pre-push hook with stdout/stderr connected to daft's capture pipes (a non-TTY).
These files are the **grammar authority** for
`src/hooks/manager_output/lefthook.rs`; the recognizer is written against them
and its unit tests read them verbatim.

All files keep their **raw bytes** — ANSI escapes and, in old versions, CRLF
line endings (`.gitattributes` marks the tree `-text` so no eol normalization
can rewrite them). The recognizer strips ANSI and trailing whitespace/`\r`
internally for matching; forwarded lines stay raw.

## How the version matrix was built

All 28 `MacOS_arm64` release binaries were downloaded and the same 5-scenario
capture matrix (parallel pass/fail, `NO_COLOR` pass/fail, serial pass) was run
against each from an isolated scratch repo. Normalizing away scheduling order,
durations, and git's own lines, then clustering byte-identical grammars, gives
**five raw variant groups** — each represented here by one version directory:

| Directory  | Covers releases             | What changed at this boundary                                                                                                                                                                           |
| ---------- | --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `v2.0.0/`  | 2.0.0 – 2.1.2 (20 releases) | Baseline: `NO_COLOR` **ignored** (banner keeps 🥊, outcomes stay `✔️`/`🥊`), blank line between job header and its output, job output lines end in `\r` (CRLF), lines width-padded with trailing spaces |
| `v2.1.3/`  | 2.1.3 – 2.1.5               | `NO_COLOR` now respected: banner drops the emoji, outcome glyphs become `✓`/`✗`. Colored mode unchanged                                                                                                 |
| `v2.1.6/`  | 2.1.6                       | CRLF gone (plain `\n`); padding and blank-after-header still present                                                                                                                                    |
| `v2.1.7/`  | 2.1.7 – 2.1.8               | Width-padding and blank-after-header gone; banner spacing doubles (`lefthook  vX  hook:  name`); `sync hooks: ✔️(pre-push)` loses its space                                                             |
| `v2.1.10/` | 2.1.9 – 2.1.10              | Banner gains one more space before `hook:`                                                                                                                                                              |

The **structural anchors are identical across all 28 releases**: the
`┃  <job> ❯ ` header, the `  ────` separator, `summary: (done in N seconds)`,
and the `<glyph> <job> (N seconds)` outcome lines never changed. Only banner
spacing, glyph selection under `NO_COLOR`, and whitespace details drifted — so
one recognizer with `\s+`-flexible matching and rstrip covers the whole 2.x
line.

## Files

Each version directory has the 5-scenario matrix:

| Fixture             | Colour       | Result | What it exercises                                                             |
| ------------------- | ------------ | ------ | ----------------------------------------------------------------------------- |
| `parallel-pass.txt` | colour       | pass   | `parallel: true`, 4 jobs (one name with parens), all pass                     |
| `parallel-fail.txt` | colour       | fail   | one job fails; its block ends with `exit status 1`                            |
| `no-color-pass.txt` | `NO_COLOR=1` | pass   | glyph/banner behavior per era (see matrix)                                    |
| `no-color-fail.txt` | `NO_COLOR=1` | fail   | fail glyph per era                                                            |
| `serial-pass.txt`   | colour       | pass   | `parallel: false`; also carries the `sync hooks:` noise line in most captures |

`v2.0.0/` and `v2.1.10/` additionally have `with-skips-pass.txt`: jobs skipped
by `skip: true` and by a non-matching `glob:` produce thin-bar notices
(`│  <job> (skip) <reason>`, U+2502 — not the thick U+2503 block header) right
after the banner, and skipped jobs never appear in the summary. The shape is
identical at both ends of the 2.x line.

`v2.1.10/` additionally has:

| Fixture                   | What it exercises                                                                                                                                                                                                                   |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `no-summary.txt`          | banner + blocks, **no** summary — a manager killed mid-run (synthesised by truncating `parallel-pass.txt` at the separator); unresolved jobs must be reconciled by the phase verdict                                                |
| `remote-rejected.txt`     | hook **passes** (every job `✔️`) but the push is rejected non-fast-forward; git's `! [rejected]`/`error:`/`hint:` lines trail on stderr — no job may be flipped to failed (#752's trap)                                             |
| `follow-pass.txt`         | `follow: true`: headers print at job **start** and output lines interleave arbitrarily after them — grouping lines to the most-recent header matches the raw stream's visual truth; per-job resolution still comes from the summary |
| `output-summary-pass.txt` | `output: [summary]`: banner and blocks suppressed — stream starts at `summary:`, the recognizer must **decline** (passthrough is today's behavior; there is no live structure to show)                                              |

Decline fixtures (streams that are **not** lefthook) live in `../decline/`:

| Fixture           | What it exercises                                                                                                                                                                              |
| ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `raw-script.txt`  | a plain hand-written `.git/hooks/pre-push` — no banner, decline on the first content line                                                                                                      |
| `decline-box.txt` | a custom hook that prints its _own_ `╭──╮` box **and** an emoji, but not the lefthook banner text — the recognizer may hold through the box frame but must decline on the non-banner text line |

## Grammar (union across 2.x, piped)

ANSI-stripped and rstripped (spaces + `\r`), in emission order:

```
╭────────╮                                ← banner top    (corners U+256D/U+256E)
│ [🥊 ]lefthook  vX.Y.Z   hook:  <hook> │  ← banner text   (spacing varies by era: \s+)
╰────────╯                                ← banner bottom (corners U+2570/U+256F)
sync hooks: ✔️[ ](pre-push)                ← optional noise, when lefthook re-syncs hooks
│  <job name> (skip) <reason>              ← skip notices (thin bar U+2502); absent from summary

┃  <job name> ❯                            ← job block header (U+2503 … U+276F)
[blank]                                    ←   ≤2.1.6 emits a blank line here
<job stdout+stderr, verbatim>              ←   lefthook re-emits both on ITS stdout
exit status <N>                            ←   only when the job failed
[blank]                                    ← block ends
… one block per job …

  ────────────                             ← summary separator (leading spaces + U+2500 run)
summary: (done in <T> seconds)             ← OPENS the summary section
<glyph> <job name> (<D> seconds)           ← one outcome line per job
```

Then **git's own** push-result lines follow on **stderr** (`To <remote>`,
` ! [rejected]`, `error: failed to push …`, `hint: …`).

### Streams and timing

- lefthook writes its **entire** grammar to **stdout** (it captures each job's
  stdout _and_ stderr and re-emits them); git's push result is on **stderr** and
  is emitted only after the hook exits — the two never interleave inside
  lefthook's sequence.
- In default (buffered) mode each `┃` block is **flushed atomically when that
  job completes** (measured: block arrival tracks job duration; the summary +
  outcome burst arrives at the end). Job _appearance_ is live; pass/fail
  _verdicts_ all arrive in the final burst.
- There is **no upfront job count** — a running census can only say "N so far".
- With `follow: true`, headers print at job start and output interleaves.

### Glyph matrix

|                    | pass               | fail             |
| ------------------ | ------------------ | ---------------- |
| colour (all 2.x)   | `✔️` U+2714 U+FE0F | `🥊` U+1F94A     |
| `NO_COLOR`, ≤2.1.2 | `✔️` (unchanged)   | `🥊` (unchanged) |
| `NO_COLOR`, ≥2.1.3 | `✓` U+2713         | `✗` U+2717       |

The colour-mode fail glyph is the **same 🥊 as the banner emoji** — outcome
matching must be gated to the summary section to avoid colliding with it.

## How these were captured

From an **isolated scratch repo** (never this repo; per CLAUDE.md safe-testing
rules), git identity via env vars only. For each of the 28 release binaries:

```bash
scratch=$(mktemp -d)
git init -q --bare "$scratch/remote.git"
git init -q "$scratch/work" && cd "$scratch/work"
git config --local user.name Test && git config --local user.email test@test.com
git remote add origin "$scratch/remote.git"
cat > lefthook.yml <<'YML'
pre-push:
  parallel: true
  jobs:
    - name: fmt
      run: "sleep 0.1; echo 'fmt output line'; true"
    - name: clippy
      run: "sleep 0.15; echo 'clippy output line'; true"
    - name: unit tests (related)
      run: "sleep 0.2; echo 'running unit tests'; echo 'test result ok'; true"
    - name: typecheck
      run: "sleep 0.12; echo 'typecheck ok'; true"
YML
"$LEFTHOOK_BIN" install -f
GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=test@test.com \
GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=test@test.com \
  git commit -qam cap
git push -f origin master > parallel-pass.txt 2>&1   # colour is forced even when piped
```

Fail / serial / `NO_COLOR` / `follow` / `output:` variants swap `lefthook.yml`
(or the hook script) as reflected in each file. To extend coverage to a future
release: download its binary, re-run the matrix, and diff the ANSI-stripped +
rstripped normalization against `v2.1.10/` — if it clusters with an existing
group no new fixtures are needed; a new shape gets a new version directory and a
row in the matrix above.
