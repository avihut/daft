---
branch: daft-708/feat/tasks-daft-run
---

# Tasks & daft run

Non-interactive behavior (task resolution, unknown-task errors, trust bypass,
`{worktree_slug}` expansion, the daft.local.yml overlay) is covered by the
`tests/manual/scenarios/run/` YAML scenarios and the
`tests/integration/test_run.sh` bash tests. This plan covers what those can't:
the live, attended, long-running behavior — cancellation, TTY passthrough, and
output rendering — which only manifests against a real terminal and real
processes.

Run every check in a real TTY, in a throwaway repo (`mktemp -d`, local git
config only, never this repo). A useful scratch daft.yml:

```yaml
tasks:
  run:
    parallel: true
    jobs:
      - name: web
        run: python3 -m http.server 8973
      - name: ticker
        run: sh -c 'while true; do echo tick; sleep 1; done'
  once:
    jobs:
      - name: greet
        run: echo hello && sleep 2 && echo bye
  shell:
    jobs:
      - name: repl
        run: python3
        interactive: true
```

## Two-stage Ctrl+C

- [ ] `daft run` (the parallel `web` + `ticker` task): both jobs stream live,
      labeled output; the ticker's `tick` lines keep coming
- [ ] One Ctrl+C stops both jobs promptly (SIGTERM) and daft exits; the shell
      prompt returns within ~1s, not after the (absent) 300s hook timeout
- [ ] After exit, no orphaned children survive: `pgrep -f 'http.server 8973'`
      and `pgrep -f 'while true'` both return nothing
- [ ] A job that traps SIGTERM (`run: sh -c 'trap "" TERM; sleep 300'`): first
      Ctrl+C is absorbed, a **second** Ctrl+C SIGKILLs it and daft exits
- [ ] Exit code after a Ctrl+C cancel is 130
- [ ] Cancelled rows render as cancelled (⊘ / "cancelled"), not as a plain
      success or a bare failure

## No execution timeout

- [ ] A task job that runs well past 5 minutes (`sleep 400`) is NOT killed at
      300s — it runs until it exits or you cancel (contrast: the same job under
      a lifecycle hook still times out at 300s)

## Interactive job (TTY passthrough)

- [ ] `daft run shell` (the `interactive: true` python REPL): you get a real
      interactive prompt, can type expressions, and see results
- [ ] Ctrl+D / normal exit returns control to daft, which finishes cleanly
- [ ] Ctrl+C at the REPL behaves like a normal terminal interrupt; a second
      Ctrl+C tears the child down

## Output rendering

- [ ] `daft run once` (single-job task): pure passthrough — the job's raw output
      with **no** daft chrome (no header, no rows, no summary), exit code
      verbatim, exactly as if you ran the command yourself
- [ ] `daft run` (multi-job): the plan-then-execute rail —
      `┌ Running task     run on <branch>` header, a `├─ run` section anchor,
      one live row per job with its log threaded beneath, `└ Done in <t>` footer
- [ ] Ctrl+C on the rail: cancelled jobs persist as `⊘ <job> cancelled` rows and
      the footer reads `Cancelled after <t>`
- [ ] Piped (`daft run 2>&1 | cat`): no rail — the classic block output, as
      before
- [ ] `daft run --list` shows the tasks with job counts; unknown task name
      prints the available list
- [ ] Tab completion: with the shell integration loaded, `daft run <TAB>` offers
      the task names from daft.yml

## Trust

- [ ] In an untrusted repo, `daft run` prints the "not in your trust list"
      note + the `daft hooks trust` tip, then runs the task anyway (explicit
      invocation = consent)
