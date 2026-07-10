---
branch: daft-663/fix/pre-push-should-be-sigterm-cancellable
---

# Sync Cancellation (#663)

All scenarios run in a scratch sandbox repo with a local file remote — never
this repository. For hook scenarios, install a `pre-push` hook in the sandbox's
`.git/hooks/` (e.g. `sleep 60`, or `mise run test:unit` for the full incident
shape) and create an unpushed commit so the hook actually fires.

## TUI path (interactive terminal)

- [ ] `daft sync --push` with a slow pre-push hook: one Ctrl+C shows the
      cancelled table state, prints "Cancelling — … press Ctrl+C again to
      force-kill", returns the prompt promptly, exits 130, and `ps` shows no
      leftover hook processes
- [ ] Same, but press Ctrl+C twice quickly: exit is immediate; no processes
      survive (`ps -axo pid,pgid,stat,command | grep -i hook`)
- [ ] Ctrl+C during the fetch phase (throttle or point remote at a slow network)
      cancels without starting per-branch tasks
- [ ] Ctrl+C mid-rebase (`daft sync --rebase master --autostash`): the
      interrupted worktree is left clean (no rebase in progress, stash
      restored), row reads skipped, exit 130
- [ ] Terminal is restored to cooked mode after cancel (typing works, no
      raw-mode residue), and the partial summary line reports counts

## Incident repro (the #663 wedge)

- [ ] Pre-push hook runs `mise run test:unit` (or a script that `setpgrp`s a
      child and `kill -STOP`s it); one Ctrl+C unsticks and kills the stopped
      group — verify no `T`-state processes remain
- [ ] TERM-trapping hook (`trap '' TERM; while :; do sleep 1; done`): first
      Ctrl+C keeps daft responsive; second Ctrl+C force-kills

## Sequential path and signals

- [ ] `daft sync --push -vv` (sequential): Ctrl+C exits 130 with the "Sync
      cancelled" line; hook subtree dead
- [ ] `daft sync --push 2>/dev/null &` then `kill -TERM <pid>`: same graceful
      teardown (SIGTERM parity), exit 130
- [ ] `echo $?` after any cancelled sync is 130; after a normal sync, 0

## Auth-prompt stop detection

- [ ] Push over ssh with no agent (passphrase would prompt): the push fails with
      the "needs terminal auth" hint instead of hanging; other worktrees
      continue; sync completes without wedging

## Regression checks on shared surfaces

- [ ] `daft exec --all -- sleep 30`: two-stage Ctrl+C still works (SIGTERM then
      SIGKILL) — shared ctrlc/termination change
- [ ] `daft list --live`: Ctrl+C still exits cleanly with cursor and cooked mode
      restored
- [ ] An interactive daft prompt (e.g. `daft repo remove` confirmation) killed
      with SIGTERM exits 130, not 0
- [ ] `daft checkout-branch` autopush with a failing/slow pre-push hook:
      behavior fully unchanged — the push runs in daft's foreground group (no
      cancel flag, no isolation), so Ctrl+C kills daft and the push subtree
      together, and terminal auth prompts still appear
- [ ] `daft exec` in a repo with shell aliases, from a real terminal: aliases
      expand and there is no ~10s stall on first run (capture must not job-stop
      fighting for the tty; `daft __capture-aliases` session detach)
- [ ] `mise run test:unit` from a real interactive terminal passes — the capture
      tests only exercise the tty arm when a controlling terminal exists (CI and
      piped runs can't reproduce it)
