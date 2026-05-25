# Universal Design Principles That Survive in TUI

Six principles from Tufte, Norman, Wathan/Schoger, Nielsen, and Shneiderman — filtered to mechanisms that survive monospace cells, ~16 colors, and keyboard input.

### 1. Strip non-data ink before adding any

**Core idea:** Every glyph not carrying information competes with the ones that are.

**Why it transfers:** TUI chartjunk is box-drawing around tables, `[INFO]` prefixes, ASCII banners. Cells are scarce; decoration steals from data.

**Example:** A worktree row reads `feat/foo  3 ahead  clean` — not `│ feat/foo │ 3↑ │ ✓ clean │`. Use a single dim rule above the footer if grouping; never wrap rows in boxes.

**Source:** Tufte — data-ink ratio, chartjunk avoidance.

### 2. Build hierarchy from weight, color, and position — not size

**Core idea:** Three levels of emphasis (primary / secondary / tertiary) are enough; pick one mechanism per level and stop.

**Why it transfers:** Monospace forbids size, so weight and contrast carry it. Primary = `.bold()` + named color. Secondary = default fg. Tertiary = `.dim()`. Anti-pattern: multiple `.dim()` intensities the renderer can't distinguish.

**Example:** Branch name is bold default-fg, `3 ahead` is plain default-fg, last-fetched is dim. Don't also bold the count — you flatten the hierarchy.

**Source:** Wathan & Schoger — hierarchy via weight and contrast.

### 3. Reserve each accent color for one meaning per screen

**Core idea:** Color is a typed enum, not a palette.

**Why it transfers:** With ~16 colors and no gradients, viewers learn each accent fast — and get confused fast if you reuse it. Publish a budget.

**Example:** daft convention: red = recoverable error, yellow = warning/pending, green = success/clean, cyan = focus/selection, magenta = destructive-confirm. Never use red and yellow on the same row unless one is system-state and the other content. Never use green for both "synced" and "selected."

**Source:** Wathan & Schoger — decisive emphasis; Tufte — restraint with color.

### 4. Make every keystroke produce visible feedback within one frame

**Core idea:** Nielsen's "keep users informed about what is going on, through appropriate feedback within a reasonable amount of time" — in a TUI the budget is one redraw.

**Why it transfers:** A status line flipping from `Loading…` to `12 worktrees` is the same mechanism as a web spinner. Silence after a keystroke is a bug.

**Example:** When `j` moves the cursor, redraw the selected row's accent before yielding. When `Enter` triggers a slow fetch, immediately swap the picker for `Fetching origin…` — don't freeze the previous frame.

**Source:** Nielsen #1 (Visibility of System Status); Shneiderman #3 (Offer informative feedback).

### 5. Show the keybindings; don't make users remember them

**Core idea:** Norman's signifiers and Nielsen's "Recognition rather than recall" — the affordance must be visible.

**Why it transfers:** The TUI equivalent of a button label is a footer hint line. One row (`j/k move  Enter select  q quit`) costs nothing against users stuck guessing.

**Example:** Every modal-screen picker reserves the bottom line for dim keybinding hints. Inline renderers without a footer print a one-shot hint on first render: `Press Ctrl-C to cancel`.

**Source:** Norman (signifiers); Nielsen #6 (Recognition rather than recall).

### 6. Make every action reversible, or warn before it isn't

**Core idea:** Shneiderman's "Permit easy reversal of actions" and Nielsen's "emergency exit."

**Why it transfers:** `Esc` is the universal back-out in takeover screens; `Ctrl-C` in inline commands. Destructive ops need an explicit confirm step, not a single keystroke.

**Example:** `Esc` always closes a picker without committing. `daft worktree remove` prompts before deleting an unmerged branch; `--force` skips it. Never bind a destructive action to a single unmodified letter adjacent to a navigation key.

**Source:** Shneiderman #6 (Permit easy reversal); Nielsen #3 (User Control and Freedom).

## What did NOT transfer

Tufte's **small multiples** and **sparklines** survive only for numeric time-series, not sync tables or pickers. Refactoring UI's **font-size hierarchy, shadows, and saturation gradients** have no monospace analogue — `.bold()` / `.dim()` plus the color enum is the toolkit. Norman's **physical affordances** reduce to "show the key in the footer." Nielsen's **aesthetic minimalism** is redundant with Tufte once you're counting cells. Shneiderman's **"design dialogs to yield closure"** matters in form-heavy GUIs; daft's flows are one-shot commands where closure is just process exit.
