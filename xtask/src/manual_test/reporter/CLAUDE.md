# Reporter design language

This file constrains the _appearance_ of the YAML test runner's output: which
colors, which weights, where in the hierarchy each string sits, how the output
reads under each verbosity level. Anyone editing `reporter/pretty.rs`,
`reporter/quiet.rs`, or anything that produces user-visible bytes through the
`Reporter` trait must follow these rules.

The rules are derived from the project-level `daft-tui-design` skill (color as a
typed enum, three-level hierarchy, strip non-data ink) and adapted for stdout —
the test runner has no focus concept, no live selection, no panels, so some
palette slots are repurposed for stdout-appropriate meanings.

When the YAML runner spins off into its own crate, this file travels with
`reporter/`. The rules then become the spun-off product's design language.

The mechanics layer (how `Reporter` is wired, the parallel-buffer model, the
trait surface) lives in `mod.rs` doc comments and `tests/README.md`. This file
owns the _design_ layer only — what to emit, not how to plumb it.

---

## 1. Color budget

Color is a typed enum, not a palette. With a terminal's ~16 named colors and no
gradients, viewers learn each accent fast and get confused fast if you reuse
one. Each slot below answers exactly one question; any future "I want to make
this stand out" gets answered by looking up the right slot.

| Slot                    | Reserved for                                                                                                                                                              | Anti-meaning                         |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ |
| **bold green**          | Success outcome: `✓` step pass, `✓` scenario footer, "passed" count > 0                                                                                                   | Never "in progress" or "ready"       |
| **bold red**            | Failure outcome: `✗`, `❯` focal-step marker, `FAIL` word, banner label, "failed" count > 0                                                                                | Never warnings                       |
| **yellow**              | Attention without alarm: `(slow)`, future "skipped" / "flaky"                                                                                                             | Never errors                         |
| **cyan**                | Section heading: scenario name (top-of-block)                                                                                                                             | Never status, never expanded command |
| **dim** / **dark grey** | Scaffolding: counters (`[N/M]`, `(N checks)`), paths, durations under threshold, detail lines, citations, banner rules, `$ expanded-command`, `step N/M` in failure block | Never primary content                |
| **default fg**          | Body content: step names, assertion labels, summary labels, reproduce command body                                                                                        | Never decoration                     |

**Cyan repurposed from `daft-tui-design`.** The TUI budget reserves cyan for
focus/selection. Stdout has no focus, so cyan slides one slot over to "section
heading." That's still a primary anchor; viewers learn it within the first
scenario. One use per screen — never reuse cyan for anything else in the
runner's output.

---

## 2. Hierarchy

Three levels carry the entire visual weight system. Monospace forbids size
shifts; weight + color + position is the whole toolkit. **Pick one mechanism per
level and stop** — adding bold to a secondary item collapses the hierarchy.

| Level         | Mechanism                    | What lives here                                                                                                                                                                                                                                                     |
| ------------- | ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Primary**   | bold + named color           | Scenario header (bold cyan), scenario footer (whole line bold green/red), `FAIL` word, banner label, `1) ✗ name` in failures block, focal step name in failures block (bold default-fg)                                                                             |
| **Secondary** | default fg, no styling       | Step name in per-step lines, assertion labels, summary labels (`Scenarios:`/`Steps:`/`Duration:`/`Reproduce:`), numbered prefix (`1)`), "passed"/"failed"/"total" words, reproduce command body                                                                     |
| **Tertiary**  | dim                          | `[N/M]` step counter, `(N checks)` / `(N failed)`, source paths, detail lines under assertions, `step N/M` inside failure block, source-line citations (`at file.yml:N`), banner rule chars, durations under threshold, `$ expanded-command`, capture-block content |
| **Accent**    | named color (may layer bold) | Count numbers (green for passed > 0, red for failed > 0), `(slow)` yellow, semantic icons (✓ bold green, ✗ bold red, ❯ bold red)                                                                                                                                    |

**The decision rule.** "Should this be bold?" → look up its level in the table.
"What color?" → look up the slot in §1. If a string fits no row, it probably
doesn't belong on screen.

---

## 3. Iconography

| Glyph   | Meaning                                                                                                      | Styling    |
| ------- | ------------------------------------------------------------------------------------------------------------ | ---------- |
| `✓`     | Pass — applies at both step level and scenario footer                                                        | bold green |
| `✗`     | Fail — at every level: step assertion, scenario footer, failures-block entries, failures-block per-assertion | bold red   |
| `❯`     | Focal failing step in the failures block (one per failure entry)                                             | bold red   |
| `⎯`     | Section rule (banner only — twelve per side, fixed width)                                                    | dim        |
| `[N/M]` | Step counter — the only counter form                                                                         | dim        |
| `$`     | Expanded-command prefix (under `-v`+ verbosity)                                                              | dim        |

**Never use lowercase `x` for failure**, even at the assertion-detail level.
Every fail icon is `✗`. (Pre-styling-pass code used `x` in one site — that was a
font-fallback workaround that no longer applies.)

**`❯` is reserved.** It marks _the_ focal failing step, exactly one per failure
entry. Don't use `❯` for "in progress," "selected," or generic emphasis.

---

## 4. Pass-quiet, fail-loud

The eye should skim past green outcomes and stop hard on red ones. Build
asymmetry deliberately:

- **Pass marker = minimal.** `✓` + lowercase `ok`. No CAPS, no extra decoration.
  Step counts use plain `(N checks)` in dim.
- **Fail marker = stacked signals.** `✗` + bold + red + UPPERCASE `FAIL`. The
  `FAIL` word itself is bold red caps; the icon doubles the signal at scenario
  level.

This asymmetry is the point. Never "balance" the output by introducing `PASS`
caps or `✓ PASSED` to match `FAIL` — they should not match. The runner's loudest
moment should be a red failure, not green chrome.

The only place the asymmetry breaks is the summary stats line:
`X passed, Y failed (Z total)`. Here the words are parallel because both are
counts the reader is comparing. The colored numbers carry the loud/quiet
distinction.

---

## 5. Microcopy

| String type                       | Convention                               | Examples                                          |
| --------------------------------- | ---------------------------------------- | ------------------------------------------------- |
| Section banner label              | Title Case                               | `Failed Scenarios (N)`                            |
| Stats label (left-aligned anchor) | Title Case + trailing colon              | `Scenarios:`, `Steps:`, `Duration:`, `Reproduce:` |
| Outcome word inside a line        | lowercase                                | `ok`, `passed`, `failed`, `total`                 |
| Loud outcome marker               | UPPERCASE                                | `FAIL` (per-step only; scenario footer uses `✗`)  |
| Diff label                        | lowercase + colon                        | `expected:`, `actual:`                            |
| Capture block divider             | `--- {stream} ---` lowercase             | `--- stdout ---`, `--- stderr ---`                |
| Source citation                   | `at <file>:<line>` lowercase preposition | `at fail.yml:14`                                  |
| Reproduce hint                    | Imperative, copy-pasteable               | `mise run test:manual -- --ci <token>`            |
| Slow annotation                   | lowercase parenthetical                  | `(slow)`                                          |

**Rule of thumb.** Titles announce, labels anchor, outcomes report, errors
explain. Don't drift into mixed forms — if you're tempted to add an ellipsis or
capitalize a label, look up the right row instead.

---

## 6. Verbosity ladder

The `-q` / default / `-v` / `-vv` ladder is the contract between the user and
the reporter. Each level subtracts or adds _whole categories_ of output — not
just "more detail."

| Flag      | Per-step lines | Per-check icons on pass | Captured on pass | Captured on fail | Expanded command | Cleanup line  |
| --------- | -------------- | ----------------------- | ---------------- | ---------------- | ---------------- | ------------- |
| `-q`      | suppressed     | no                      | no               | summary only     | no               | suppressed    |
| (default) | shown          | no                      | no               | first 20 lines   | no               | **fail only** |
| `-v`      | + check icons  | yes                     | first 20 lines   | first 20 lines   | yes              | fail only     |
| `-vv`     | + check icons  | yes                     | uncapped         | uncapped         | yes              | fail only     |

**Cleanup line.** `Cleaned up test environment.` is suppressed on green at every
verbosity (Principle 1: strip non-data ink). On fail it always emits flush
against the footer — it's part of the failure block's visual context.

**`-q` failures still surface.** The summary block (failed-scenarios + the
reproduce block) is shared across reporters; `-q` doesn't hide failures, it just
suppresses the per-step chatter that precedes them.

---

## 7. Visual rhythm

Spacing rules — terse but load-bearing. Blank lines are signals; spend them on
transitions between block-level things.

- **One blank line separates scenarios.** Owned by `scenario_header` (leading
  blank), not `scenario_footer` (no trailing blank). This keeps the cleanup note
  attached to its scenario's footer.
- **One blank line separates the per-scenario stream from the summary.** Owned
  by `run_summary`.
- **One blank line above and below the failures banner.**
- **No blank lines inside a scenario block.** Step-to-step is dense by design —
  that's where the structural signal lives (counter + step name + outcome on one
  line).
- **Cleanup line is flush to its footer.** Never preceded by a blank line. The
  blank that separates scenarios lives in the _next_ scenario's
  `scenario_header`.

These rules formalize the spacing-fix contract from commit `e54fa114`.

---

## Anti-patterns

A flat list of things future PRs will be tempted to do. Each one is authorized
by an earlier section's rule it would violate.

- **Adding a new color outside the budget.** Blue, magenta, bright-anything —
  none are in §1. If a new state needs a slot, propose it as a change to the
  budget table, not as a one-off in `pretty.rs`.
- **Reusing a budget slot for a second meaning on the same screen.** Cyan is one
  meaning. Yellow is one meaning. If "the new thing also kind of feels like a
  heading," it doesn't get cyan; rethink it.
- **Bolding secondary content** (labels, counters, numbered prefixes) to "make
  them pop." Per §2, bold belongs to primary content. Bolding secondary content
  collapses the hierarchy.
- **Balancing the output with `PASS` caps or `✓ PASSED`** to mirror `FAIL` /
  `✗ FAILED`. Violates §4.
- **Coloring decoration** (rule chars, brackets, separators). Decoration is dim
  by definition; data carries the color.
- **Hardcoding ANSI escapes inline.** Always go through `term_styles`. If you
  need a combination the crate doesn't expose, add a helper to `term_styles`
  (like the existing `bold_red`, `bold_green`, `bold_cyan`) rather than inlining
  the bytes.
- **Drifting microcopy.** New label that says "in progress…" with an ellipsis
  when `Status:` would do; new outcome word that capitalizes; Title-Case where
  lowercase is the rule. Look up §5 before writing a new string.
- **Blank lines inside a scenario block** to "give the steps room to breathe."
  Per §7, intra-block density is the point — that's where the structural signal
  lives.
- **Inlining a design rule into project `CLAUDE.md`** instead of here. Design
  language is reporter-scoped; project `CLAUDE.md` only points to this file.
  Inlining rots a daft-specific file that won't spin out.
