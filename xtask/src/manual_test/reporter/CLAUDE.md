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

| Slot                    | Reserved for                                                                                                                                                                                                                                                                                                                                                                        | Anti-meaning                         |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ |
| **bold green**          | Diff label: `expected:` / `unexpected:` (failure payload, accent at concentrated weight)                                                                                                                                                                                                                                                                                            | Never the pass icon                  |
| **green** (not bold)    | Pass marker: `✓` step pass, `✓` scenario footer, "passed" count > 0                                                                                                                                                                                                                                                                                                                 | Never "selected" or "in progress"    |
| **bold red**            | Failure outcome: `✗`, `❯` focal-step marker, `FAIL` word, banner label, "failed" count > 0; `actual:` diff label                                                                                                                                                                                                                                                                    | Never warnings                       |
| **yellow**              | Attention without alarm: `(slow)`, future "skipped" / "flaky"                                                                                                                                                                                                                                                                                                                       | Never errors                         |
| **cyan**                | Structural anchors. **Bold cyan** = scenario name (Level 1). **Plain cyan** = `[N/M]` step counter at the start of each step line (Level 2 boundary marker — names the step block to the eye while the bright-purple step name carries the identity); **also the live totals completion-map** (the braille field of finished scenarios — totals is the top-level structure; see §8) | Never status, never expanded command |
| **bright purple**       | Step identity: step name in the per-step opening line (bold at `-vv` for the Layer-2 anchor against Layer 3/4 content, plain at `-v`)                                                                                                                                                                                                                                               | Never assertion content              |
| **blue**                | Step action: `$ command` body (including `>` continuation lines on multi-line commands)                                                                                                                                                                                                                                                                                             | Never the pass / fail outcome        |
| **dim** / **dark grey** | Pure scaffolding: `(N checks)` / `(N failed)` suffix, scenario-header path, durations under threshold, banner rules, `step N/M` in failure block, capture-block stream label (`stdout` / `stderr`), capture-block body lines, capture-block truncation hint                                                                                                                         | Never the failure payload            |
| **default fg**          | Body content + failure payload: assertion labels, summary labels, reproduce command body, **assertion `detail` lines under a failed assertion**, failure-block location pointer (`path:line`)                                                                                                                                                                                       | Never decoration                     |

**Cyan repurposed from `daft-tui-design`.** The TUI budget reserves cyan for
focus/selection. Stdout has no focus, so cyan covers "structural anchor" — bold
cyan on the scenario name at Level 1, plain cyan on the `[N/M]` step counter at
Level 2. The counter sits to the left of the step name and acts as a
column-aligned boundary marker the eye can run down to count steps and spot
where each block begins; the step name itself carries identity in bright purple
(a separate slot, separate meaning). Treat the two cyan uses as one slot —
"structural anchor at the start of each block" — with bold marking the bigger
block.

The live region's totals completion-map (§8) is the same slot at run scale: the
totals line is the top-level structure, so its braille field is cyan. It's the
one colored gauge on screen, and the worker rows below it stay uncolored — so
cyan still reads as "the structural anchor," now for the whole run rather than a
single scenario block.

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

| Level         | Mechanism                    | What lives here                                                                                                                                                                                                                                                                                                                                                                                          |
| ------------- | ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Primary**   | bold + named color           | Scenario header (bold cyan), scenario footer on FAIL (whole `✗ name` span bold red), `FAIL` word, banner label, `1) ✗ name` in failures block, focal step name in failures block (bold default-fg)                                                                                                                                                                                                       |
| **Secondary** | default fg, no styling       | **Scenario name on a PASSING footer** (default fg, not bold), assertion labels (`✓ Exit code: …` check labels included), summary labels (`Scenarios:`/`Steps:`/`Duration:`/`Reproduce:`), numbered prefix (`1)`), "passed"/"failed"/"total" words, reproduce command body, **assertion `detail` lines under a failed assertion** (the failure payload), failure-block location pointer (`path:line`)     |
| **Tertiary**  | dim                          | `(N checks)` / `(N failed)` suffix, scenario-header path, `step N/M` inside failure block, banner rule chars, durations under threshold, capture-block stream label (`stdout` / `stderr`), capture-block body lines, capture-block truncation hint                                                                                                                                                       |
| **Accent**    | named color (may layer bold) | Count numbers (green for passed > 0, red for failed > 0), `(slow)` yellow, semantic icons (`✓` green not bold, `✗` bold red, `❯` bold red), diff labels (`expected:` bold green, `actual:` bold red), **`[N/M]` step counter** (plain cyan — structural anchor at each step boundary), **step name in per-step lines** (bright purple at `-v`, bold bright purple at `-vv`), **`$ command` body** (blue) |

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

| Glyph   | Meaning                                                                                                                                            | Styling                   |
| ------- | -------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------- |
| `✓`     | Pass — applies at both step level and scenario footer                                                                                              | green (not bold) — see §4 |
| `✗`     | Fail — at every level: step assertion, scenario footer, failures-block entries, failures-block per-assertion                                       | bold red                  |
| `❯`     | Focal failing step in the failures block (one per failure entry)                                                                                   | bold red                  |
| `⎯`     | Section rule (banner only — twelve per side, fixed width)                                                                                          | dim                       |
| `[N/M]` | Step counter in scrollback (per-step lines)                                                                                                        | dim                       |
| `N/M`   | `done/total` counter in the live region (no brackets — the field/bar to its left is the visual frame); see §8                                      | default fg                |
| `⟨⣿⡇…⟩` | Totals completion-map: `⟨ ⟩`-framed braille field, one dot per finished scenario cluster — live, then persisted into scrollback at end-of-run (§8) | dim frame, cyan dots      |
| `▬`/`─` | Live worker-row step bar (`▬` fill over `─` track); see §8                                                                                         | `▬` default fg, `─` dim   |
| `idle`  | A worker slot with no scenario in flight (empty `─` track, no counter/clock); see §8                                                               | dim                       |
| `◆`     | Segment separator in the live totals tail (`running ◆ failed ◆ cancelled`)                                                                         | dim                       |
| `$`     | Expanded-command prefix (under `-v`+ verbosity)                                                                                                    | dim                       |

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
| Reproduce hint                    | Imperative, copy-pasteable               | `mise run test:manual -- <token>`                 |
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
(scenario / step / step action / capture body). The indent ladder gives each
layer its own column; each step opening line pairs a plain-cyan `[N/M]` counter
(structural anchor at the step boundary, same hue family as the bold-cyan
scenario above) with a bright-purple step name (bold at `-vv` for the Layer-2
anchor against the body content below, plain at `-v` where no body content
competes); `$ command` body is blue with `>` continuation prefixes when the YAML
`run:` field is multi-line (shell prompt convention keeps the visual frame
across line wraps); capture stream label (`stdout` / `stderr`, no `--- ---`
decoration — the indent carries the framing) and capture body both stay dim —
they're context that the indent already frames, and color would compete with the
cyan/purple anchors above. A blank line separates step blocks at `-vv`.

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

## 8. Live progress region

On a TTY, the runner pins a multi-row region at the bottom of the terminal
during a parallel run: a **totals line on top, then one fixed row per worker
below it**. Completed scenarios stream their full byte buffer into scrollback
above the region in completion order; at end-of-run the filled completion-map is
persisted into scrollback and the rest of the region clears before the final
summary block prints. On non-TTY (CI logs, redirected output, `cargo run`), the
region is suppressed entirely and output reverts to the input-order drain at end
— byte-identical.

```text
  ⟨⣿⡇⣿ ⣧ … ⟩  211/581  0:05  9/10 running ◆ 2 failed
  ▬▬▬▬───────  2/6  1.2s  checkout-basic · Inspect workspace
  ▬▬░░░░░░░░░  1/3  0.4s  clone-remote · Clone repository
  ─────────────  idle
```

**The singular line is distinctive; the repeated lines are quiet.** The totals
line is one of a kind, so it carries the screen's one colored gauge. The worker
rows _repeat_ — N of them stack — so they're deliberately light: nothing that
piles into a block wall or out-shouts the real results streaming into scrollback
above. This split, not a shared "every bar looks the same" motif, is the
organizing rule of the region.

**The worker rows are a fixed pool, one per worker — they never come and go.**
Sized once at run start (worker count, capped at the scenario count) and held
for the whole run, so the region's height is constant and no row ever shifts
under the reader's eye. An earlier design added a row when a worker picked up a
scenario and removed it on completion; the row count churned as workers ramped
up and wound down, and a run was impossible to track because nothing held still.
A slot is _claimed_ when its worker starts a scenario and _released_ to a quiet
`idle` placeholder — dim empty `─` track (left edge still aligned with the busy
rows), dim `idle` label, no counter or clock (a ticking timer on a worker that
isn't running would read as activity where there is none) — when the scenario
finishes. A free slot always exists to claim because the worker pool has exactly
that many threads.

**Totals line — a spatial completion-map** (not a percentage gauge, not a
time-series). A braille **field** where each dot owns a fixed cluster of
scenarios _by index_ and lights once that cluster finishes. Because the
scheduler hands work out non-linearly (workers chew the beginning, middle, and
end at once), the field fills _scattered_ — a developing-photo view of which
parts of the scenario set are done, which a left-to-right fill bar can't show.

- **Field** — `FIELD_WIDTH` braille chars × 8 dots; each dot ≈
  `total / (FIELD_WIDTH·8)` scenarios by index (small runs drop to one scenario
  per dot). Dots fill bottom-left → top-right within a cell. The four braille
  rows are _resolution_, not a y-axis, so the field stays **one line** — no
  vertical real estate spent. Dots **cyan**, `⟨ ⟩` frame **dim**.
- Then `done/total` scenarios (default fg) · run `elapsed` (dim) · `R/A running`
  (R in-flight, A = worker-pool size) · `M failed` · `C cancelled` ·
  `(cancelling)`. Segment separators are **dim** `◆`.

**The completion-map persists past the run.** When the region tears down, the
finished field is printed into scrollback (a frozen `⟨…⟩  done/total scenarios`
line) directly above the summary block, rather than vanishing with the rest of
the live region — it's the one view of _which_ parts of the set ran, and
discarding it the instant the run ends throws away the artifact the reader just
watched develop. The frozen line drops the live-only segments (elapsed,
running/failed/cancelled): the summary block right below already owns the
precise duration and pass/fail tally, so repeating them would only duplicate.
After a cancel, `done < total`, so the partially-lit map reads as how far the
run got.

**Worker row — a light step bar + a flowing tail.**

- **Bar** — `progress_chars("▬─")`: a medium-rect `▬` fill (default fg) over a
  thin `─` track (dim). Heavier than a hairline rule, never the full-cell block,
  so stacked rows never read as a wall.
- Then `done/total` steps (default fg) · per-scenario `elapsed` (dim) · the
  **tail**: the scenario name (default fg — the scannable identity) with the
  current step flowing right after it behind a dim `·` (**no fixed step
  column**, so the step never feels detached). The separator and step name are
  **dim** — the step changes every step, so keeping it quiet stops it from
  flicker-grabbing the eye and reserves color for the totals line. `(slow)` is
  yellow once the scenario crosses 5 s.

**No new color slot.** Cyan = structural anchor (§1) → the totals completion-map
(totals is the top-level structure), making it the one colored gauge on screen;
the live counters stay **default fg** rather than scrollback's cyan `[N/M]` so a
second cyan slot doesn't dilute the anchor. Everything on the worker rows reuses
default fg (bar fill, scenario name, counter) and dim (track, elapsed,
separator, step name). `M failed` is bold red once `> 0` (live fail-loud) and
**hidden at `0`** — a ticking `0 failed` on every green run is chrome. **Cancel
mode is a derivative of this same line**: `C cancelled` and `(cancelling)` ride
the totals tail in the yellow slot (attention-without-alarm), and the worker
rows keep their layout as they wind down.

**Narrow-display safety is a correctness property, not cosmetics.** Each line's
variable tail is a single `{wide_msg}`, which indicatif **truncates** (never
wraps) to the terminal width — the fixed field/bar + counter + time prefix is
the only non-truncating content. A wrapped row would desync indicatif's
line-count accounting and strand a ghost row in scrollback (the hazard the whole
`indicatif_sink.rs` trailer / `state_lock` / remove-then-println machinery
exists to prevent), so keeping every row exactly one line tall at any width is
load-bearing. `{wide_msg}` truncation is ANSI-aware, so the inlined dim / yellow
/ red segments survive a cut without bleeding color past it. On the worker row
the step name truncates first, then the scenario name; the field, counter, and
time always remain.

**Visibility ladder**: the region is orthogonal to verbosity — all four tiers
(`-q` / default / `-v` / `-vv`) get it on TTY, because it tracks live state, not
output volume. `-q` benefits the most: its scrollback is silent on green, so the
region is the entire heartbeat. Failures still surface in scrollback at `-q`
with the fail footer + cleanup line.

**Interactive and `--setup-only` skip the region.** Both bail to `run_serial`
before the parallel scheduler runs and have semantics (TTY ownership for
interactive, stdout work-dir capture for setup-only) incompatible with a pinned
live area.

**Implementation discipline.** ANSI codes are inlined into the hand-built
strings at indicatif's `{prefix}` / `{msg}` boundary — the completion-map (cyan
dots, dim frame), the persisted end-of-run map line, the dim `idle` label, the
`◆` separators, the dim ` · step` tail, and the conditional `failed > 0` /
`(slow)` / cancel segments (the template DSL has no conditional hook for those,
and a custom-rendered field can't be a styled built-in key). Inlined SGR
therefore **bypasses `NO_COLOR`** — bar _templates_ honor it, but bytes inside
`{prefix}`/`{msg}` are passed through verbatim. Inlining is acceptable **inside
the progress module only**. Everywhere else in the reporter, go through
`term_styles` — see Anti-patterns.

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
