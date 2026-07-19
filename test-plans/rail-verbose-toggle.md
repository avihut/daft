---
branch: daft-729/feat/rail-verbose-toggle
---

# Live verbose toggle on the plan-execute rail

The automated coverage drives a PTY, which proves the mechanics but not the
feel. These need a real terminal.

## The toggle itself

- [ ] `daft exec --all -- <slow chatty command>` — press `v` mid-run; live rows
      grow a rolling output window and the footer hint flips to `v quiet`
- [ ] Press `v` again — windows collapse, latest line returns to the row's own
      annotation, hint flips back to `v verbose`
- [ ] Rows that already finished compactly fold out once, under a repeat of
      their receipt line, headed by `verbose on — replaying N finished rows`
- [ ] Toggling on/off/on does not replay the same log twice
- [ ] Start with `-v` and press `v` — later receipts arrive compact; what
      already printed stays
- [ ] Failed rows are not replayed (their output threaded when they failed)

## Reading the result

- [ ] The fold-out block is attributable at a glance — you can tell which row
      each replayed log belongs to
- [ ] The hint is legible but recedes; it never competes with the rows
- [ ] Nothing jumps or double-prints as the density changes under load (try a
      20+ worktree fan-out)

## Terminal handling

- [ ] Typing `v` prints nothing into the region
- [ ] Other keys typed during a run are swallowed silently, not echoed
- [ ] After the run: typing echoes normally and line editing works (`stty -a`
      shows `icanon` and `echo`)
- [ ] Same after `Ctrl-C`, and after a run that ends in failure

## Ctrl-C (#663 must be untouched)

- [ ] First `Ctrl-C` cancels the workers; the rail closes as `Cancelled after t`
      rather than being torn down
- [ ] A command that ignores SIGTERM needs a second `Ctrl-C` to die
- [ ] No literal `^C` is echoed into the region
- [ ] Exit status is still 130

## Prompts and interactive jobs under a live region

- [ ] `daft clone --install` (or any prompt after the plan commits): the prompt
      echoes what you type and accepts Enter normally
- [ ] `v` typed while the prompt is up goes to the prompt, not the rail
- [ ] After the prompt, `v` toggles again
- [ ] `daft run` with a multi-job task containing an interactive job: the job
      owns the terminal, typing is visible, and the rail resumes after

## Other rails

- [ ] `daft start` / `daft go` with hooks: pressing `v` mid-phase switches
      hook-job rendering from the next line onward
- [ ] `daft run` with several jobs: same
- [ ] Non-TTY (`daft exec --all ... | cat`, CI): no hint, no key handling,
      output unchanged
