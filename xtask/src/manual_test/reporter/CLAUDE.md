# Reporter design language

This file constrains the _appearance_ of the YAML test runner's output: which
colors, which weights, where in the hierarchy each string sits, how the output
reads under each verbosity level. Anyone editing `reporter/pretty.rs` or
anything that produces user-visible bytes through the `Reporter` trait must
follow these rules.

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

| Slot                    | Reserved for                                                                                                                                                                                                                                                     | Anti-meaning                         |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ |
| **bold green**          | Diff label: `expected:` / `unexpected:` (failure payload, accent at concentrated weight)                                                                                                                                                                         | Never the pass icon                  |
| **green** (not bold)    | Pass marker: `✓` step pass, `✓` scenario footer, "passed" count > 0                                                                                                                                                                                              | Never "selected" or "in progress"    |
| **bold red**            | Failure outcome: `✗`, `❯` focal-step marker, `FAIL` word, banner label, "failed" count > 0; `actual:` diff label                                                                                                                                                 | Never warnings                       |
| **yellow**              | Attention without alarm: `(slow)`, future "skipped" / "flaky"                                                                                                                                                                                                    | Never errors                         |
| **cyan**                | Section heading: scenario name (top-of-block)                                                                                                                                                                                                                    | Never status, never expanded command |
| **dim** / **dark grey** | Pure scaffolding: counters (`[N/M]`, `(N checks)`), scenario-header path, durations under threshold, banner rules, `$ expanded-command`, `step N/M` in failure block, capture-block divider (`--- stdout ---` / `--- stderr ---`), capture-block truncation hint | Never the failure payload            |
| **default fg**          | Body content + failure payload: step names, assertion labels, summary labels, reproduce command body, **assertion `detail` lines under a failed assertion**, failure-block location pointer (`path:line`)                                                        | Never decoration                     |

**Cyan repurposed from `daft-tui-design`.** The TUI budget reserves cyan for
focus/selection. Stdout has no focus, so cyan slides one slot over to "section
heading." That's still a primary anchor; viewers learn it within the first
scenario. One use per screen — never reuse cyan for anything else in the
runner's output.

**Never combine dim with color.** Most terminals implement dim as
half-brightness on top of whatever color is set — `dim + green` and `dim + red`
collapse to muddy grey-green / grey-red that's nearly invisible at a glance
against a normal background. Colors must render at full saturation. If a span
needs to be colored, it cannot also be inside a dimmed line — restructure so the
dim wrap doesn't cover it, or accept that it's scaffolding and use dim alone (no
color). The corollary: assertion `detail` lines on a failed assertion are
**not** scaffolding — they're the failure payload — and so they render at
default-fg (their diff labels at full bold green / bold red), never dim.

---

## 2. Hierarchy

Three levels carry the entire visual weight system. Monospace forbids size
shifts; weight + color + position is the whole toolkit. **Pick one mechanism per
level and stop** — adding bold to a secondary item collapses the hierarchy.

| Level         | Mechanism                    | What lives here                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| ------------- | ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Primary**   | bold + named color           | Scenario header (bold cyan), scenario footer on FAIL (whole `✗ name` span bold red), `FAIL` word, banner label, `1) ✗ name` in failures block, focal step name in failures block (bold default-fg), **step opening name at `-vv`** (bold default-fg — see §6)                                                                                                                                                                                                                                                                |
| **Secondary** | default fg, no styling       | Step name in per-step lines at `-v` (plain), **scenario name on a PASSING footer** (default fg, not bold), assertion labels (`✓ Exit code: …` check labels included), summary labels (`Scenarios:`/`Steps:`/`Duration:`/`Reproduce:`), numbered prefix (`1)`), "passed"/"failed"/"total" words, reproduce command body, **assertion `detail` lines under a failed assertion** (the failure payload), failure-block location pointer (`path:line`), **capture-block body lines** (when emitted at `-v`+, they're the payload) |
| **Tertiary**  | dim                          | `[N/M]` step counter, `(N checks)` / `(N failed)`, scenario-header path, `step N/M` inside failure block, banner rule chars, durations under threshold, `$ expanded-command`, **capture-block divider** (`--- stdout ---` / `--- stderr ---` — orientation only), capture-block truncation hint                                                                                                                                                                                                                              |
| **Accent**    | named color (may layer bold) | Count numbers (green for passed > 0, red for failed > 0), `(slow)` yellow, semantic icons (`✓` green not bold, `✗` bold red, `❯` bold red), diff labels (`expected:` bold green, `actual:` bold red)                                                                                                                                                                                                                                                                                                                         |

**The decision rule.** "Should this be bold?" → look up its level in the table.
"What color?" → look up the slot in §1. If a string fits no row, it probably
doesn't belong on screen.

**Hierarchy is contextual to the visible data layers.** An element styled
correctly at one verbosity tier can dissolve into noise at the next when a new
layer with similar styling appears alongside it. Re-coloring and re-indenting
existing elements is the right move, not the wrong one — indentation is a
hierarchy mechanism as much as weight and color. Verbosity-specific rules live
in §6 callouts; never bury them in `pretty.rs` helpers without a comment
pointing at the layer interaction that motivated them.

---

## 3. Iconography

| Glyph   | Meaning                                                                                                      | Styling                   |
| ------- | ------------------------------------------------------------------------------------------------------------ | ------------------------- |
| `✓`     | Pass — applies at both step level and scenario footer                                                        | green (not bold) — see §4 |
| `✗`     | Fail — at every level: step assertion, scenario footer, failures-block entries, failures-block per-assertion | bold red                  |
| `❯`     | Focal failing step in the failures block (one per failure entry)                                             | bold red                  |
| `⎯`     | Section rule (banner only — twelve per side, fixed width)                                                    | dim                       |
| `[N/M]` | Step counter — the only counter form                                                                         | dim                       |
| `$`     | Expanded-command prefix (under `-v`+ verbosity)                                                              | dim                       |

**Never use lowercase `x` for failure**, even at the assertion-detail level.
Every fail icon is `✗`. (Pre-styling-pass code used `x` in one site — that was a
font-fallback workaround that no longer applies.)

**`❯` is reserved.** It marks _the_ focal failing step, exactly one per failure
entry. Don't use `❯` for "in progress," "selected," or generic emphasis.

---

## 4. Pass-quiet, fail-loud

The eye should skim past green outcomes and stop hard on red ones. Build
asymmetry deliberately:

- **Pass marker = minimal.** `✓` in plain green (NOT bold), lowercase `ok`, no
  CAPS, no extra decoration. The scenario name on a passing footer is default
  fg, not bold. Step counts use plain `(N checks)` in dim. A wall of bold green
  at default-tier verbosity (one footer per scenario × hundreds of scenarios)
  collapses into chrome — the eye stops being able to skim past it. Plain green
  on a small `✓` glyph is the entire pass signal.
- **Fail marker = stacked signals.** `✗` + bold + red + UPPERCASE `FAIL`. The
  `FAIL` word itself is bold red caps; the icon doubles the signal at scenario
  level, and the scenario name on a failing footer goes bold red (the whole
  icon+name span) so a single red line jumps off a wall of quiet pass lines.

This asymmetry is the point. Never "balance" the output by introducing `PASS`
caps or `✓ PASSED` to match `FAIL` — they should not match. The runner's loudest
moment should be a red failure, not green chrome.

The only place the asymmetry breaks is the summary stats line:
`X passed, Y failed (Z total)`. Here the words are parallel because both are
counts the reader is comparing. The colored numbers carry the loud/quiet
distinction.

The same asymmetry lands at the **verbosity-flag level** in §6: the default
ladder emits one footer line per scenario (pass-quiet) and reserves the expanded
per-step / capture detail for `-v` and above; `-q` goes one further and emits
nothing inline for passing scenarios. Failures stay loud at every level — the
fail footer + the end-of-run summary block always surface.

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
just "more detail." The default sits where `cargo test`, vitest, and pytest sit:
one line per leaf unit (here, one footer per scenario). Old-default detail
(header, path, per-step lines, inline capture on fail) lives at `-v`; "firehose
everything, uncapped" lives at `-vv`. `-q` collapses passing scenarios entirely.

| Flag      | Pass footer | Fail footer | Cleanup line | Scenario header + path | Per-step lines | Check icons on pass | Inline capture on fail | Inline capture on pass | Expanded `$ command` |
| --------- | ----------- | ----------- | ------------ | ---------------------- | -------------- | ------------------- | ---------------------- | ---------------------- | -------------------- |
| `-q`      | no          | yes         | fail only    | no                     | no             | no                  | no (summary only)      | no                     | no                   |
| (default) | yes         | yes         | fail only    | no                     | no             | no                  | no (summary only)      | no                     | no                   |
| `-v`      | yes         | yes         | fail only    | yes                    | yes            | no                  | first 20 lines         | no                     | no                   |
| `-vv`     | yes         | yes         | fail only    | yes                    | yes            | yes                 | uncapped               | uncapped               | yes                  |

**Cleanup line.** `Cleaned up test environment.` is suppressed on pass at every
verbosity (Principle 1: strip non-data ink). On fail it emits flush against the
footer at every verbosity — it's part of the failure block's visual context, and
the rule is identical across `-q` / default / `-v` / `-vv`.

**Failures always surface.** The summary block (failed-scenarios + the reproduce
block) emits whenever there are failures, at every verbosity. `-q` and default
don't hide failures, they defer the per-step detail from "inline as it happens"
to "in the end-of-run summary block." A default-quiet run that hits a failure
still gives the reader the focal step, the failed assertions, the diff labels at
full saturation, the terminal-clickable `path:line`, and the capture — all in
the failures block.

**`-v` is the diagnostic tier.** Anyone who wants live "which step in which
scenario" feedback as the run progresses (a sequential debug run, a flake hunt)
reaches for `-v`. Anyone who wants full capture without truncation reaches for
`-vv`.

**`-vv` re-styles existing elements to keep four data layers on screen at once**
(scenario / step / step action / capture body). Specifically at `-vv`: each
layer gets its own indent column so spatial separation can carry the hierarchy
that weight + color alone can't sustain at this density; the step opening name
bolds so each step block has a Level-2 anchor; `$ command` and capture-stream
labels promote from dim to default fg because they sit adjacent to capture body
and would otherwise vanish; the `--- {stream} ---` decoration drops in favor of
plain `stdout` / `stderr` labels (the indent carries the framing now); a blank
line separates step blocks. At `-v` only the step-indent and the on-fail
capture-body indent apply — Layer 3 / pass Layer 4 don't appear, so the rest of
the ladder doesn't get exercised.

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
- **Coloring a span inside a dimmed line** ("FG-only reset so outer dim
  survives"). Don't. Most terminals render dim as half-brightness on top of the
  color and you get muddy grey-green / grey-red that's invisible at a glance. If
  a span needs color, it cannot be inside a dim wrap — restructure the
  surrounding context to drop the dim, or accept the span as scaffolding and use
  dim alone. The original `term_styles::inline_green` / `inline_red` /
  `FG_DEFAULT` helpers existed to support this anti-pattern and were removed.
- **Drifting microcopy.** New label that says "in progress…" with an ellipsis
  when `Status:` would do; new outcome word that capitalizes; Title-Case where
  lowercase is the rule. Look up §5 before writing a new string.
- **Blank lines inside a scenario block** to "give the steps room to breathe."
  Per §7, intra-block density is the point — that's where the structural signal
  lives.
- **Inlining a design rule into project `CLAUDE.md`** instead of here. Design
  language is reporter-scoped; project `CLAUDE.md` only points to this file.
  Inlining rots a daft-specific file that won't spin out.
