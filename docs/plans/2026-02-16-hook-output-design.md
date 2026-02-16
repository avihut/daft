# Hook Execution Output with Spinners and Rolling Windows

## Problem

YAML hook execution silently swallows stdout/stderr of successful jobs. Legacy
script hooks stream output via `output.raw()`, but YAML hooks capture output
into `HookResult` strings and never display them. Users cannot see what hooks
are doing while they run.

## Goals

- Show hook job output in real time during execution
- Provide spinners for running jobs so users know something is happening
- Show elapsed timers for long-running jobs
- Display a rolling tail window per job (last N lines) to keep output
  comprehensible
- Print full output when a job finishes (scrolls into terminal history)
- Make output behavior configurable via git config
- Gracefully degrade when stdout is not a TTY (CI, pipes)

## Visual Output

### During execution (parallel, 2 jobs running)

```
Running worktree-post-create hook (parallel)...
  ⠋ install-deps
  │   Installing dependencies...
  │   Resolving packages...
  │   added 847 packages
  ⠋ build-project [12s]
  │   Compiling daft v1.0.24
  │   Compiling clap v4.5.0
  │   Compiling serde v1.0.200
```

- Spinner on each running job, animating at ~80ms
- Timer appears after a configurable delay (default 5s), showing `[Xs]`
- Rolling window of up to 6 lines per job, showing the tail of stdout/stderr
- Output lines indented under their job with a `│` gutter

### When a job finishes

Full output scrolls above the active spinner area. The spinner line is replaced
with a check/cross and final duration:

```
  ✓ install-deps (2.3s)
  ✓ build-project (35.8s)
```

### On failure

```
  ✗ build-project (12.1s)
    error[E0308]: mismatched types
    ...full stderr...
```

### Non-TTY fallback

When stdout is not a terminal (piped, CI), disable spinners and print output
line-by-line as plain text, similar to current legacy hook behavior.

## Configuration

New `daft.hooks.output.*` git config keys:

| Key                            | Default | Description                                              |
| ------------------------------ | ------- | -------------------------------------------------------- |
| `daft.hooks.output.quiet`      | `false` | Suppress hook stdout/stderr (only show spinner + result) |
| `daft.hooks.output.timerDelay` | `5`     | Seconds before showing elapsed timer on spinners         |
| `daft.hooks.output.tailLines`  | `6`     | Number of rolling output lines per job (0 = no tail)     |

Set globally or per-repo:

```bash
git config --global daft.hooks.output.quiet true
git config daft.hooks.output.timerDelay 3
```

## Dependencies

Add `indicatif` (0.18) for multi-spinner management. The `console` crate comes
transitively and provides TTY detection and styling utilities.

```toml
indicatif = "0.18"
```

## Architecture

### New module: `src/output/hook_progress.rs`

`HookProgressRenderer` struct wrapping `indicatif::MultiProgress`. Manages
per-job state: spinner `ProgressBar`, tail line `ProgressBar`s, output buffer.

Key methods:

- `start_job(name)` -- add spinner + tail lines for a new job
- `update_job_output(name, line)` -- push a line into the rolling window
- `finish_job_success(name, duration)` -- replace spinner with check mark
- `finish_job_failure(name, duration)` -- replace spinner with cross mark
- `print_full_output(name)` -- print buffered output above active area

### Changes to `yaml_executor.rs`

`run_shell_command()` gains a callback/channel for streaming output lines as
they arrive instead of buffering everything silently. `execute_parallel()` and
`execute_sequential()` feed output lines to `HookProgressRenderer`.

### Changes to `executor.rs` (legacy hooks)

Legacy hooks already stream via `output.raw()`. Wrap this in the same renderer
when available for visual consistency.

### Changes to `settings.rs`

Add `daft.hooks.output.*` config keys and parsing. Extend `HooksConfig` with
output settings struct.

### Non-TTY detection

Check `console::Term::stdout().is_term()` at renderer creation. When not a TTY,
fall back to plain line-by-line output without spinners or cursor manipulation.
