---
branch: daft-678/feat/sync-resource-governor
---

# Sync Push Resource Governor (#678)

All manual testing happens in a throwaway sandbox under `/tmp` (`mktemp -d`),
never in the daft repository itself. Suggested rig: a local bare remote, a
contained-layout clone with 6+ feature branches (each with an upstream and one
unpushed commit), and a memory-hog pre-push hook, e.g.:

```sh
#!/bin/sh
# ~2 GiB resident for ~20s, then pass.
python3 -c "b = bytearray(2 * 1024**3); import time; time.sleep(20)"
```

## Stage 0/1 — cap + memory-aware admission

- [ ] `daft sync --push` with the heavy hook: at most max(2, cores/4) hooks run
      concurrently (watch `ps` / Activity Monitor); other rows show a dim
      `held: memory` / `held: capped`
- [ ] Available memory never drops below ~max(10% RAM, 2G) while the fleet
      pushes; the machine stays responsive throughout
- [ ] Post-run summary line appears: "N pushes throttled Xs to preserve memory
      headroom"
- [ ] `--jobs 1` fully serializes hook runs; `--jobs` requires `--push`
- [ ] `--no-throttle` (or `daft.governor.mode off`) restores ungoverned
      parallelism
- [ ] No-hook repo: `sync --push` behaves exactly as before (no governor lines,
      no sampler thread in `ps -M`)

## Stage 2 — learned profiles

- [ ] Second run with the same heavy hook is capped from its first launch (no
      slow-start probing);
      `sqlite3 <state>/jobs/<hash>/coordinator.db     'select * from hook_profiles;'`
      shows a row with a plausible peak
- [ ] Swap the hook for a trivial one (`exit 0`): the next run re-profiles; the
      run after that gets full parallelism immediately
- [ ] `governor_events` records `throttle` rows with held milliseconds

## Stage 3 — containment

- [ ] With pushes running, force pressure (or use a hook that balloons): the
      newest push's hook tree shows `T` in `ps -o stat` (frozen), its row shows
      `held: frozen`, and green pressure thaws it (`held:` clears back to
      `pushing`)
- [ ] Sustained pressure past ~10s kills the frozen push; its row shows
      `held: retry`, the retry succeeds once pressure clears, and the
      summary/exit reflect success
- [ ] Ctrl+C while a row shows `held: memory` → exit 130, no survivors
- [ ] Ctrl+C while a hook tree is frozen (`T` state) → exit 130, the frozen tree
      is thawed and torn down (no `T` stragglers in `ps`)

## Timeout

- [ ] `git config daft.sync.pushTimeout 10s` + a hook that sleeps 60s: the push
      fails after ~10s with the timeout hint, sync exits non-zero, and no hook
      processes survive
- [ ] A frozen unit's timeout clock pauses: freeze + thaw does not consume the
      budget

## Jobserver

- [ ] With the governor active, a hook running `make -d 2>&1 | grep jobserver`
      (or `cargo build -v`) shows jobserver tokens in use; total CPU across
      concurrent hooks stays around one machine's worth
- [ ] `daft.governor.jobserver off` removes MAKEFLAGS from the hook env

## Batched strategy

- [ ] `git config daft.sync.pushHookStrategy batched`: `sync --push` over N
      branches runs the hook once (count with a marker file); every branch lands
      on the remote; the TUI shows per-branch results
- [ ] A refusing hook fails every branch in the batch and sync exits non-zero
      with the hook's output
- [ ] A branch with a rebase conflict is excluded from the batch; branches
      without an upstream still report the per-branch skip
