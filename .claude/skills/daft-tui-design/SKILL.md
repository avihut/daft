---
name: daft-tui-design
description: Use when designing or revising any TUI screen in daft — picking a layout, deciding what to surface, choosing a color, writing a label or error message, picking keys, designing a confirmation flow, or auditing an existing screen. Companion to daft-tui (which covers mechanics). This skill covers design decisions: hierarchy, scan order, color semantics, microcopy, keybinding doctrine, error and empty states. Synthesised from Tufte / Norman / Wathan-Schoger / Nielsen / Shneiderman plus a six-TUI reference study (gitui, lazygit, bottom, atuin, yazi, k9s).
---

# daft TUI design

Design conventions and decisions for daft's TUI surface. Read this when you're choosing what to show, where, in what colour, and with what wording. The mechanics — panic hook, Stylize, hjkl wiring, terminal lifecycle — live in [[daft-tui]]; this skill assumes those are already settled.

The principles below are the body — each is concrete enough that two contributors should make the same call given it. The supporting research lives in [`references/design-principles.md`](references/design-principles.md) (sources) and [`references/tui-references.md`](references/tui-references.md) (canonical-TUI study).

## Six principles

### 1. Strip non-data ink before adding any

Every glyph that doesn't carry information competes with the ones that do. TUI cells are scarce; decoration steals from data.

**Apply:** A worktree row reads `feat/foo  3 ahead  clean`, not `│ feat/foo │ 3↑ │ ✓ clean │`. Use single dim rules to separate sections; never wrap rows in boxes. The sync table in `src/output/tui/render.rs` already follows this — it uses column headers (`Branch  Path  Base  Changes  Remote  Age  Owner  Commit`) and no row borders. Keep it that way when adding columns.

**Don't:** add a column whose only payload is a static icon or `[INFO]` prefix. If the icon's information is "this row exists," delete it.

### 2. Build hierarchy from weight, color, and position — not size

Monospace forbids size shifts, so three levels of emphasis (primary / secondary / tertiary) carry the whole hierarchy: primary = `.bold()` + a named color or default fg; secondary = default fg; tertiary = `.dim()`. Pick one mechanism per level and stop.

**Apply:** A picker row's branch name is bold default-fg, the `3 ahead` count is plain default-fg, the last-fetched-at is dim. The selected row gets the highlight color on top — but it stays bold default-fg underneath. Wiring: see [[daft-tui]] for Stylize-over-`Style::default()`.

**Don't:** also `.bold()` the count or the timestamp because "they feel important." That flattens the three levels into two and you've lost the hierarchy.

### 3. Reserve each accent color for one meaning per screen

Color in a TUI is a typed enum, not a palette. With ~16 colours and no gradients, viewers learn each accent fast — and get confused fast if you reuse it. Publish a budget and enforce it.

**daft's color budget:**

| Color           | Reserved for                                        | Anti-meaning                                |
| --------------- | --------------------------------------------------- | ------------------------------------------- |
| `.red()`        | Errors and destructive markers (`prune`, fail)      | Anything informational. Never "selected".   |
| `.green()`      | Success, completion checkmarks, "clean" state       | "Selected" — green is outcome, not focus.   |
| `.yellow()`     | Warnings AND pending/in-progress (today: overloaded — see audit findings) | Don't add a third meaning. |
| `.cyan()`       | Focus / selection / "active tab" (the canonical TUI default; daft uses it sparingly today) | Don't substitute another color for focus. |
| `.dim()` / `.dark_gray()` | Muted secondary metadata (paths, timestamps) | Never use dim as the primary text on a row. |

A row may carry red OR yellow, never both. A row may carry green OR cyan (success vs. selected), never both. If you need to say "this errored AND is selected," the focus indication is the cyan background — leave the row text in its red/green/etc. semantic color.

### 4. Make every keystroke produce visible feedback within one frame

Silence after a keypress is a bug. Nielsen's "visibility of system status" in a TUI's budget is one redraw.

**Apply:**

- `j` / `k` redraws the new selection before yielding. If a key handler triggers a slow operation (fetch, prune scan), swap the picker for `<verb-ing>… ` on the frame before launching the work — don't leave the previous frame frozen.
- Long-running phases (sync, prune) get the yellow spinner + phase label that `presenter.rs` already emits. Don't add a slow operation without a phase indicator.

**Don't:** assume an operation is fast enough to skip the spinner. The picker's responsiveness is graded against the worst-case input, not the median.

### 5. Show the keybindings; don't make users remember them

Norman's signifiers and Nielsen's "recognition over recall": the affordance must be visible. In a TUI, that's a hint line. Five of six reference TUIs (gitui, lazygit, bottom, yazi, k9s) render a bottom bar of currently-valid keys; atuin alone skips it.

**daft's stance — open convention** (see Open conventions): daft today shows neither a persistent hint bar nor a `?` overlay in shared_picker. New pickers should add a dim bottom-line hint (`j/k move  h/l tab  Enter select  Esc cancel`) until the global help model is settled. One line is cheaper than every contributor improvising.

Wiring for the hjkl + arrow match arms: see [[daft-tui]].

### 6. Make every action reversible, or warn before it isn't

Shneiderman's "permit easy reversal" and Nielsen's "user control and freedom." `Esc` is the universal back-out for takeover screens; `Ctrl-C` for inline commands. Destructive ops need an explicit confirm step, not a single keystroke.

**Apply:**

- `Esc` always cancels a picker. `Cancelled` is a first-class variant on every `*Decision` enum (see `RemoveDecision::Cancelled` in `shared_picker/remove_modal.rs`).
- Destructive operations go through a modal overlay — the convention used by `remove_modal.rs`, `show_confirm_dialog`, and the `"Cancel sync?"` / `"Overwrite local changes?"` flows in `shared_picker/mod.rs`. Match this pattern for new destructive ops; don't invent inline-chord confirmations.
- Never bind a destructive action to a single unmodified letter adjacent to a navigation key.

## Open conventions

These are choices daft has either made or left open. The questions came from divergences across the six reference TUIs; the answers here are grounded in daft's current code.

### Layout: hybrid by purpose

daft renders TUIs in **two modes**, by purpose:

- **Inline viewport** for sync / prune progress — `src/output/tui/mod.rs`, `driver.rs`, `render.rs`. The table stays in scrollback so the user can scroll back to a completed run.
- **Alternate-screen takeover** for pickers — `shared_picker/`. Full-screen modal interactions that disappear on exit.

Decision rule: if the output should remain visible after the command exits, use inline. If it's a modal interaction the user dismisses, use alternate screen. Don't add a third paradigm (persistent multi-panel à la lazygit) without a strong reason — daft's flows are transactional, not session-oriented.

### Tab dispatch: directional, not numeric

`shared_picker/input.rs:48-49` dispatches tab walks with `h` / `l` (and `Left` / `Right`). The `Tab` / `BackTab` keys are bound to *panel focus toggle*, not tab cycling.

- Don't introduce numeric tab keys (`1`, `2`, `3`) — that's gitui's convention, not daft's.
- Don't repurpose `Tab` for tab walking. It's reserved for inter-panel focus.
- When a picker grows to ≥4 tabs, revisit this convention — `h` / `l` becomes unwieldy past three.

### Confirmation: modal overlay, never inline chord

daft's destructive confirmations are modal overlays (see `shared_picker/remove_modal.rs`, `show_confirm_dialog`). Five of six reference TUIs use modal confirms; bottom and k9s use inline chord (`dd`) for kill operations.

- All new destructive ops use the modal pattern. Don't add a `dd`-style inline chord.
- The modal's default focus is the safe option (Cancel / Materialize), not the destructive one. `remove_modal.rs` already does this.

### Microcopy: title-case titles, sentence-case prompts, terse-past success

Decoded from the existing TUI strings:

| String type             | Convention                                | Examples from daft                                              |
| ----------------------- | ----------------------------------------- | --------------------------------------------------------------- |
| Modal / screen title    | Title Case                                | `"Shared File Manager"`, `"Confirm deletion"`, `"Partial submit"` |
| Confirmation question   | Sentence-case verb-first, ending `?`      | `"Cancel sync?"`, `"Overwrite local changes?"`, `"Continue?"`   |
| Empty state             | Sentence-case statement                   | `"No matching files found"`, `"No shared files remaining"`      |
| Success (transient)     | Past-tense terse                          | `"Collected {path} from {wt}"`, `"Removed {path}"`              |
| Error                   | `"Failed to <verb> <thing>: {detail}"`    | `"Failed to create shared storage: {e}"`, `"Collect failed: {e}"` |
| Inline instructional    | Lowercase parenthetical                   | `"Search: (type to filter)"`                                    |
| Column header           | Title Case noun                           | `"Branch"`, `"Path"`, `"Changes"`, `"Remote"`                   |

Use these forms. The rule of thumb: titles announce, prompts ask, success states report, errors explain. Don't drift into mixed forms ("Searching..." with the ellipsis as a status when "Search:" is the prompt).

### Theming: hardcoded slots

daft has no user theming infrastructure today and isn't planning any. All color choices live in code via the budget above. This is atuin's stance, not bottom's — and it's the right one for daft's surface area. If user theming ever lands, the budget above becomes the slot names.

## What did NOT transfer from the graphical-design canon

These are tempting borrowings to suppress:

- **Tufte's sparklines and small multiples** survive only for numeric time-series. daft has none — the sync table is categorical, not quantitative. Don't reach for them.
- **Refactoring UI's font-size hierarchy, shadows, saturation gradients** have no monospace analogue. `.bold()` + `.dim()` + the color enum are the entire emphasis toolkit. Don't simulate sizing with banners or ASCII art.
- **Norman's physical affordances** reduce to "show the key in the footer." Don't write text that pretends to be a button.
- **Aesthetic minimalism as a separate principle** is redundant with Tufte's data-ink ratio once you're counting cells.
- **"Design dialogs to yield closure"** matters in form-heavy GUIs. daft's flows are one-shot commands; closure is just process exit.

## Reference TUIs — what to study, where

When a design question doesn't resolve from the principles, study these source files in the linked repos. Each is the most direct answer to a question daft also has.

| Question                              | App      | Where to look                                                                |
| ------------------------------------- | -------- | ---------------------------------------------------------------------------- |
| Color-promoting context-relevant keys | lazygit  | `pkg/gui/options_map.go`, `renderContextOptionsMap`                          |
| Semantic color slot naming            | gitui    | `src/ui/style.rs`                                                            |
| Per-binding `desc` field (descriptions next to bindings) | yazi | `yazi-config/preset/keymap-*.toml`                                       |
| Currently-valid-only hint bar         | gitui    | `src/cmdbar.rs`                                                              |
| Compactness ladder (auto-density)     | atuin    | `crates/atuin/src/command/client/search/interactive.rs::Compactness`         |
| Single-highlight-color reuse          | bottom   | `src/options/config/style/themes/default.rs::HIGHLIGHT_COLOUR`               |
| Status colors as named slots          | k9s      | `internal/config/styles.go`, `skins/*.yaml`                                  |

Full per-app extraction with stack, layout paradigm, and microcopy notes lives in [`references/tui-references.md`](references/tui-references.md).

## Anti-patterns

- **Boxing rows.** Don't draw borders around table rows or list items. Use dim separators between sections at most. (Violates Principle 1.)
- **Color as decoration.** Don't add green or cyan to a label "to make it pop." Every accent must answer "what state is this signalling?" If the answer is "none," it stays default fg. (Violates Principle 3.)
- **Yellow for two things on one screen.** Yellow currently signals both "in-progress phase" and "warning" in daft — when both can appear on the same screen (e.g., a warning during a running sync), one of them must move. (Violates Principle 3.)
- **Inline-chord destructive ops.** No `dd`-style two-key destructive confirms. Use the modal pattern.
- **Footer-less picker.** Adding a new picker variant without at least a one-line dim keybinding hint, until the global help model is settled. (Violates Principle 5.)
- **Re-documenting mechanics.** If you find yourself writing about *how* a panic hook works or *how* Stylize composes, you're in the wrong skill — that's [[daft-tui]].
