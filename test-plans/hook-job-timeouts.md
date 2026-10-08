---
branch: daft-1009/yaml-jobs-hook-timeout
---

# Hook job timeouts

The YAML scenarios `tests/manual/scenarios/hooks/job-timeout*.yml` cover the
`daft.hooks.timeout` setting, the job → hook → git → 5m precedence, piped mode,
legacy scripts, and invalid values, all with the test renderer hidden. This plan
covers what they can't: how a timeout looks in a real terminal, background jobs,
and machine-level behavior under load.

Set up a scratch repo with a `pre-merge` gate that sleeps past its limit, e.g.
`timeout: 3` on a job running `echo started; sleep 30; echo done`.

## Foreground rendering

- [ ] Rail (TTY): the job row shows ✗, and a notice line reads
      `Job 'gate' timed out after 3s` — not folded away like
      `Job 'x' failed (exit code: N)`
- [ ] The abort line reads
      `pre-merge hook failed: job 'gate' timed out after 3s` and no
      `exit code -1` appears anywhere
- [ ] `--verbose` merge: the job's output (`started`) is in the threaded log,
      and the timeout line still appears
- [ ] Piped to a file (`daft merge … 2>&1 | cat`): the plain renderer shows the
      same two lines
- [ ] The merge returns at ~3s, not after the 30s sleep

## Teardown

- [ ] `pgrep -f 'sleep 30'` right after the refusal finds nothing
- [ ] A job that traps SIGTERM (`trap '' TERM; sleep 60; echo done`) is
      SIGKILLed ~10s after the limit, and the merge then returns
- [ ] A job that leaves a pipe holder behind (`sleep 60 & echo started`, no
      redirect) with `timeout: 3`: the merge goes through at ~3s (the job keeps
      its exit 0), and `sleep 60` is gone

## Background jobs

- [ ] A `background: true` post-create job with `timeout: 3` and a 30s sleep:
      `daft hooks jobs` shows it `failed` with exit code 124 at ~3s, not still
      `running`
- [ ] `daft hooks jobs logs <job>` shows the output it printed before the limit
- [ ] `--hooks foreground` on the same command: the job times out inline at the
      same 3s

## Configuration

- [ ] `daft config` lists "Hook job timeout" as a duration, default `5m`, and
      the editor hints `30m, 2h, 7d, bare seconds, or off`
- [ ] `git config daft.hooks.timeout 'forty minutes'`, then a merge: one warning
      (not one per hook fire) naming the bad value and that jobs use 5m
- [ ] `daft hooks jobs retry` on a timed-out job uses `daft.hooks.timeout`, not
      the job's daft.yml `timeout:`, as documented
- [ ] `daft hooks dump` prints `timeout: 300` and `timeout: 40m` exactly as
      written

## The ticket's case

- [ ] A real slow gate (`mise run check-all` or similar, 5–10 min under load)
      with `timeout: 20m` on the job passes, where it was killed at 5m0s before
