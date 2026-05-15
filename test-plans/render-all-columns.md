---
branch: daft-494/fix/render-all-columns
---

# Render All Columns — Manual Smoke

Fix #494: TUI no longer drops columns by terminal width. Verify across three
widths (narrow / typical / wide) and two commands (`daft list`, `daft sync`).

## How to constrain width

In a real terminal: `resize -s 5 60`, `printf '\e[8;5;60t'`, or `stty cols 60`
on platforms that support it. Easier path: resize the terminal window manually.

For TUI commands (`sync`), confirm by reading the rendered header. The full
default column header set is:

- Phased (`sync`, `clone`, `prune`, `repo remove`):
  `Status   Branch  Path  Base  Changes  Remote  Age  Owner  Hash  Commit`
  (Annotation column has an empty label — sits between Status and Branch as
  blank space.)
- Phaseless (`daft list`):
  `Branch  Path  Base  Changes  Remote  Age  Owner  Commit`

## 60 cols

- [ ] `daft list` — full default header set rendered. Branch / Path / Commit
      show ellipsis truncation as needed. **No column missing.** (Ratatui may
      clip the rightmost columns at the buffer edge — that is the spec's "accept
      overflow rather than dropping data" behavior. Confirm the column **set**
      is intact, even if pixels off-screen.)
- [ ] `daft sync` — same as above; Status column on the left, Hash and Commit on
      the right.

## 100 cols

- [ ] `daft list` — full default header set. Light Branch / Path / Commit
      truncation possible if branch names are long; no missing columns.
- [ ] `daft sync` — same.

## 160 cols

- [ ] `daft list` — full default header set, no truncation.
- [ ] `daft sync` — same.

## Mid-`sync` resize regression

- [ ] Start `daft sync` in a 100-col terminal. Mid-run, resize the terminal to
      60 cols. Confirm: no columns appear or disappear during the resize; only
      the widths of Branch / Path / Commit shift. (Pre-fix this was the worst
      symptom — columns popped in and out as data streamed in.)
