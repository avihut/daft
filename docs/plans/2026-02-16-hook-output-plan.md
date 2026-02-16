# Hook Output with Spinners and Rolling Windows - Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.

**Goal:** Make YAML hook execution show real-time output with spinners, rolling
tail windows, elapsed timers, and configurable behavior — matching and exceeding
lefthook's output UX.

**Architecture:** Add `indicatif` for multi-spinner management. Create a
`HookProgressRenderer` that wraps `indicatif::MultiProgress` to manage per-job
spinners and rolling output lines. Modify `yaml_executor::run_shell_command()`
to stream output via a channel instead of silently buffering. Add
`daft.hooks.output.*` git config keys for user customization. Fall back to plain
text when not a TTY.

**Tech Stack:** Rust, `indicatif` 0.18, `console` (transitive via indicatif)

---

### Task 1: Add `indicatif` dependency

**Files:**

- Modify: `Cargo.toml:24-46`

**Step 1: Add indicatif to dependencies**

In `Cargo.toml`, add after line 45 (`globset = "0.4.18"`):

```toml
indicatif = "0.18"
```

**Step 2: Verify it compiles**

Run: `cargo check` Expected: compiles with no errors

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add indicatif dependency for hook progress display"
```

---

### Task 2: Add output config to settings

**Files:**

- Modify: `src/settings.rs:24-28` (doc table), `src/settings.rs:156-170` (keys
  module), `src/settings.rs:267-302` (HooksConfig struct + Default),
  `src/settings.rs:383-427` (load_hooks_config), `src/settings.rs:429-475`
  (load_hooks_config_global)
- Modify: `src/hooks/mod.rs:248-265` (re-export or reference)

**Step 1: Write unit tests for the new config fields**

In `src/settings.rs`, find the existing `#[cfg(test)] mod tests` block. Add
tests for the new output config:

```rust
#[test]
fn test_hook_output_config_defaults() {
    let config = HookOutputConfig::default();
    assert!(!config.quiet);
    assert_eq!(config.timer_delay_secs, 5);
    assert_eq!(config.tail_lines, 6);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_hook_output_config_defaults` Expected: FAIL —
`HookOutputConfig` not defined

**Step 3: Add HookOutputConfig struct and config keys**

In `src/settings.rs`, add the struct near line 267 (before `HooksConfig`):

```rust
/// Configuration for hook output display.
#[derive(Debug, Clone)]
pub struct HookOutputConfig {
    /// Suppress hook stdout/stderr (only show spinner + result line).
    pub quiet: bool,
    /// Seconds before showing elapsed timer on spinners.
    pub timer_delay_secs: u32,
    /// Number of rolling output tail lines per job (0 = no tail).
    pub tail_lines: u32,
}

impl Default for HookOutputConfig {
    fn default() -> Self {
        Self {
            quiet: false,
            timer_delay_secs: 5,
            tail_lines: 6,
        }
    }
}
```

Add config keys in the `keys::hooks` module (near line 165):

```rust
pub const OUTPUT_QUIET: &str = "daft.hooks.output.quiet";
pub const OUTPUT_TIMER_DELAY: &str = "daft.hooks.output.timerDelay";
pub const OUTPUT_TAIL_LINES: &str = "daft.hooks.output.tailLines";
```

Add the field to `HooksConfig` struct:

```rust
pub struct HooksConfig {
    // ... existing fields ...
    /// Output display configuration.
    pub output: HookOutputConfig,
}
```

And in `Default for HooksConfig`:

```rust
output: HookOutputConfig::default(),
```

Add loading in `load_hooks_config()` (after the timeout loading, around line
420):

```rust
// Load output settings
if let Some(value) = git.config_get(keys::hooks::OUTPUT_QUIET)? {
    config.output.quiet = parse_bool(&value, false);
}
if let Some(value) = git.config_get(keys::hooks::OUTPUT_TIMER_DELAY)? {
    if let Ok(delay) = value.parse::<u32>() {
        config.output.timer_delay_secs = delay;
    }
}
if let Some(value) = git.config_get(keys::hooks::OUTPUT_TAIL_LINES)? {
    if let Ok(lines) = value.parse::<u32>() {
        config.output.tail_lines = lines;
    }
}
```

Add the same loading in `load_hooks_config_global()` (around line 462), using
`config_get_global` instead.

Update the doc table at the top of `src/settings.rs` (lines 24-28) to include
the new keys.

**Step 4: Run tests**

Run: `cargo test test_hook_output_config_defaults` Expected: PASS

**Step 5: Run all tests and clippy**

Run: `cargo test && cargo clippy` Expected: all pass, no warnings

**Step 6: Commit**

```bash
git add src/settings.rs src/hooks/mod.rs
git commit -m "feat: add daft.hooks.output.* config keys for hook display settings"
```

---

### Task 3: Create HookProgressRenderer (TTY mode)

**Files:**

- Create: `src/output/hook_progress.rs`
- Modify: `src/output/mod.rs` (add module declaration)

**Step 1: Write tests for HookProgressRenderer**

Create `src/output/hook_progress.rs` with tests first. The renderer wraps
`indicatif::MultiProgress` and manages per-job state.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_renderer_creation() {
        let config = HookOutputConfig::default();
        let renderer = HookProgressRenderer::new(&config);
        assert!(renderer.is_some()); // None only when not TTY, but in tests we use hidden mode
    }

    #[test]
    fn test_start_and_finish_job() {
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("test-job");
        renderer.finish_job_success("test-job", std::time::Duration::from_secs(2));
    }

    #[test]
    fn test_update_job_output_rolling_window() {
        let mut config = HookOutputConfig::default();
        config.tail_lines = 3;
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("test-job");

        // Push more lines than tail_lines
        for i in 0..10 {
            renderer.update_job_output("test-job", &format!("line {i}"));
        }

        // Buffer should contain all 10 lines
        let output = renderer.get_buffered_output("test-job");
        assert_eq!(output.len(), 10);

        renderer.finish_job_success("test-job", std::time::Duration::from_secs(1));
    }

    #[test]
    fn test_quiet_mode_no_tail_lines() {
        let mut config = HookOutputConfig::default();
        config.quiet = true;
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("test-job");
        renderer.update_job_output("test-job", "should not show");
        // In quiet mode, tail lines should not be created
        renderer.finish_job_success("test-job", std::time::Duration::from_secs(1));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test hook_progress` Expected: FAIL — module doesn't exist

**Step 3: Implement HookProgressRenderer**

Write the full implementation in `src/output/hook_progress.rs`:

```rust
//! Hook progress renderer using indicatif for spinners and rolling output.
//!
//! Provides real-time visual feedback during hook execution with:
//! - Per-job spinners with elapsed timers
//! - Rolling tail windows showing last N lines of output
//! - Full output printed above active area when jobs finish

use crate::settings::HookOutputConfig;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// State for a single running job.
struct JobState {
    /// The main spinner progress bar.
    spinner: ProgressBar,
    /// Tail line progress bars (rolling window).
    tail_lines: Vec<ProgressBar>,
    /// Full output buffer (all lines, not just tail).
    output_buffer: Vec<String>,
    /// When the job started.
    start_time: Instant,
}

/// Renders hook job progress with spinners and rolling output windows.
pub struct HookProgressRenderer {
    mp: MultiProgress,
    jobs: HashMap<String, JobState>,
    config: HookOutputConfig,
    /// Style for the spinner line (before timer kicks in).
    spinner_style: ProgressStyle,
    /// Style for the spinner line (with elapsed timer).
    spinner_style_with_timer: ProgressStyle,
    /// Style for tail output lines.
    tail_style: ProgressStyle,
}

impl HookProgressRenderer {
    /// Create a new renderer. Returns the renderer (uses stderr for output).
    pub fn new(config: &HookOutputConfig) -> Self {
        Self::create(config, MultiProgress::new())
    }

    /// Create a hidden renderer for testing (no terminal output).
    #[cfg(test)]
    pub fn new_hidden(config: &HookOutputConfig) -> Self {
        Self::create(config, MultiProgress::with_draw_target(
            indicatif::ProgressDrawTarget::hidden(),
        ))
    }

    fn create(config: &HookOutputConfig, mp: MultiProgress) -> Self {
        let spinner_style = ProgressStyle::with_template("  {spinner} {msg}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");

        let spinner_style_with_timer =
            ProgressStyle::with_template("  {spinner} {msg} [{elapsed_precise}]")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");

        let tail_style = ProgressStyle::with_template("  │   {msg}").unwrap();

        Self {
            mp,
            jobs: HashMap::new(),
            config: config.clone(),
            spinner_style,
            spinner_style_with_timer,
            tail_style,
        }
    }

    /// Register and start a new job with a spinner.
    pub fn start_job(&mut self, name: &str) {
        let spinner = self.mp.add(ProgressBar::new_spinner());
        spinner.set_style(self.spinner_style.clone());
        spinner.set_message(name.to_string());
        spinner.enable_steady_tick(Duration::from_millis(80));

        // Create tail line progress bars (unless quiet or tail_lines == 0)
        let tail_lines = if self.config.quiet || self.config.tail_lines == 0 {
            Vec::new()
        } else {
            (0..self.config.tail_lines)
                .map(|_| {
                    let pb = self.mp.insert_after(&spinner, ProgressBar::new_spinner());
                    pb.set_style(self.tail_style.clone());
                    pb.set_message("");
                    pb
                })
                .collect()
        };

        self.jobs.insert(
            name.to_string(),
            JobState {
                spinner,
                tail_lines,
                output_buffer: Vec::new(),
                start_time: Instant::now(),
            },
        );
    }

    /// Push a new output line for a job, updating the rolling tail window.
    pub fn update_job_output(&mut self, name: &str, line: &str) {
        let Some(state) = self.jobs.get_mut(name) else {
            return;
        };

        state.output_buffer.push(line.to_string());

        // Switch to timer style if timer delay has elapsed
        if state.start_time.elapsed()
            >= Duration::from_secs(u64::from(self.config.timer_delay_secs))
        {
            state
                .spinner
                .set_style(self.spinner_style_with_timer.clone());
        }

        // Update rolling tail window
        if !state.tail_lines.is_empty() {
            let buf_len = state.output_buffer.len();
            let tail_count = state.tail_lines.len();
            let start = buf_len.saturating_sub(tail_count);

            for (i, tail_pb) in state.tail_lines.iter().enumerate() {
                let buf_idx = start + i;
                if buf_idx < buf_len {
                    tail_pb.set_message(state.output_buffer[buf_idx].clone());
                } else {
                    tail_pb.set_message(String::new());
                }
            }
        }
    }

    /// Finish a job as successful. Prints full output above spinners, then shows result line.
    pub fn finish_job_success(&mut self, name: &str, duration: Duration) {
        self.finish_job(name, true, duration);
    }

    /// Finish a job as failed. Prints full output above spinners, then shows result line.
    pub fn finish_job_failure(&mut self, name: &str, duration: Duration) {
        self.finish_job(name, false, duration);
    }

    fn finish_job(&mut self, name: &str, success: bool, duration: Duration) {
        let Some(state) = self.jobs.remove(name) else {
            return;
        };

        // Remove tail lines first
        for pb in &state.tail_lines {
            pb.finish_and_clear();
        }

        // Print full buffered output above the active spinner area
        if !state.output_buffer.is_empty() && !self.config.quiet {
            for line in &state.output_buffer {
                self.mp.println(format!("  │   {line}")).ok();
            }
        }

        // Replace spinner with result line
        let duration_str = format_duration(duration);
        let (marker, label) = if success {
            ("✓", name)
        } else {
            ("✗", name)
        };

        state
            .spinner
            .set_style(ProgressStyle::with_template("  {msg}").unwrap());
        state
            .spinner
            .finish_with_message(format!("{marker} {label} ({duration_str})"));
    }

    /// Get the buffered output for a job (for testing).
    #[cfg(test)]
    pub fn get_buffered_output(&self, name: &str) -> &[String] {
        self.jobs
            .get(name)
            .map(|s| s.output_buffer.as_slice())
            .unwrap_or(&[])
    }

    /// Print a message above the active spinner area.
    pub fn println(&self, msg: &str) {
        self.mp.println(msg).ok();
    }
}

/// Format a duration as human-readable (e.g., "2.3s", "1m 5s").
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let mins = secs as u64 / 60;
        let remaining = secs as u64 % 60;
        format!("{mins}m {remaining}s")
    }
}
```

Add to `src/output/mod.rs` (after the existing module declarations):

```rust
pub mod hook_progress;
```

**Step 4: Run tests**

Run: `cargo test hook_progress` Expected: PASS

**Step 5: Run clippy**

Run: `cargo clippy` Expected: no warnings

**Step 6: Commit**

```bash
git add src/output/hook_progress.rs src/output/mod.rs
git commit -m "feat: add HookProgressRenderer with spinners and rolling output"
```

---

### Task 4: Add output streaming to `run_shell_command()`

**Files:**

- Modify: `src/hooks/yaml_executor.rs:1091-1176` (run_shell_command function)

**Step 1: Write a test for output streaming**

Add to the existing `#[cfg(test)] mod tests` at the bottom of
`yaml_executor.rs`:

```rust
#[test]
fn test_run_shell_command_streams_output() {
    let ctx = make_ctx();
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    let result = run_shell_command_with_callback(
        "echo hello && echo world",
        &HashMap::new(),
        Path::new("/tmp"),
        &ctx,
        Duration::from_secs(10),
        Some(tx),
    )
    .unwrap();

    assert!(result.success);
    assert!(result.stdout.contains("hello"));

    let lines: Vec<String> = rx.try_iter().collect();
    assert!(lines.iter().any(|l| l.contains("hello")));
    assert!(lines.iter().any(|l| l.contains("world")));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_run_shell_command_streams_output` Expected: FAIL —
`run_shell_command_with_callback` not defined

**Step 3: Add callback variant of `run_shell_command`**

Modify `run_shell_command` at line 1091 to accept an optional sender. Create a
new function `run_shell_command_with_callback` and have the original delegate to
it with `None`:

```rust
/// Run a shell command and capture its output.
fn run_shell_command(
    cmd: &str,
    extra_env: &HashMap<String, String>,
    working_dir: &Path,
    ctx: &HookContext,
    timeout: Duration,
) -> Result<HookResult> {
    run_shell_command_with_callback(cmd, extra_env, working_dir, ctx, timeout, None)
}

/// Run a shell command, capture output, and optionally stream lines via a channel.
fn run_shell_command_with_callback(
    cmd: &str,
    extra_env: &HashMap<String, String>,
    working_dir: &Path,
    ctx: &HookContext,
    timeout: Duration,
    line_sender: Option<std::sync::mpsc::Sender<String>>,
) -> Result<HookResult> {
    // ... same setup as before through line 1120 ...

    let tx_stdout = line_sender.clone();
    let stdout_thread = std::thread::spawn(move || {
        let mut content = String::new();
        if let Some(stdout) = stdout_handle {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(ref tx) = tx_stdout {
                    tx.send(line.clone()).ok();
                }
                content.push_str(&line);
                content.push('\n');
            }
        }
        content
    });

    let tx_stderr = line_sender;
    let stderr_thread = std::thread::spawn(move || {
        let mut content = String::new();
        if let Some(stderr) = stderr_handle {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(ref tx) = tx_stderr {
                    tx.send(line.clone()).ok();
                }
                content.push_str(&line);
                content.push('\n');
            }
        }
        content
    });

    // ... rest unchanged (timeout, join, return HookResult) ...
}
```

**Step 4: Run tests**

Run: `cargo test test_run_shell_command_streams_output` Expected: PASS

**Step 5: Run full test suite**

Run: `cargo test` Expected: all pass (existing behavior unchanged since
`line_sender` is `None` by default)

**Step 6: Commit**

```bash
git add src/hooks/yaml_executor.rs
git commit -m "feat: add output streaming callback to run_shell_command"
```

---

### Task 5: Wire HookProgressRenderer into YAML sequential execution

**Files:**

- Modify: `src/hooks/yaml_executor.rs:92-154` (execute_yaml_hook_with_rc),
  `src/hooks/yaml_executor.rs:190-244` (execute_sequential),
  `src/hooks/yaml_executor.rs:996-1055` (execute_single_job)
- Modify: `src/hooks/executor.rs:118-149` (execute method — pass config through)

**Step 1: Write integration-style test**

Add to `yaml_executor.rs` tests:

```rust
#[test]
fn test_sequential_hook_streams_output_to_renderer() {
    let hook_def = HookDef {
        parallel: Some(false),
        jobs: Some(vec![JobDef {
            name: Some("echo-job".to_string()),
            run: Some("echo 'hello from hook'".to_string()),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let ctx = make_ctx();
    let mut output = TestOutput::default();

    let result = execute_yaml_hook(
        "test-hook",
        &hook_def,
        &ctx,
        &mut output,
        ".daft",
        Path::new("/tmp"),
    )
    .unwrap();

    assert!(result.success);
    assert!(result.stdout.contains("hello from hook"));
}
```

**Step 2: Run test**

Run: `cargo test test_sequential_hook_streams_output_to_renderer` Expected: PASS
(this tests current behavior — output is captured)

**Step 3: Thread `HookOutputConfig` through the execution chain**

Modify `ExecContext` (line 61) to include the output config:

```rust
struct ExecContext<'a> {
    hook_ctx: &'a HookContext,
    hook_env: &'a HashMap<String, String>,
    source_dir: &'a str,
    working_dir: &'a Path,
    rc: Option<&'a str>,
    output_config: &'a HookOutputConfig,
}
```

Modify `execute_yaml_hook_with_rc` to accept and pass through
`HookOutputConfig`. Since this is a public function, add the parameter:

```rust
pub fn execute_yaml_hook_with_rc(
    hook_name: &str,
    hook_def: &HookDef,
    ctx: &HookContext,
    output: &mut dyn Output,
    source_dir: &str,
    working_dir: &Path,
    rc: Option<&str>,
    output_config: &HookOutputConfig,
) -> Result<HookResult> {
```

Update `execute_yaml_hook` similarly and pass it through.

In `execute_single_job`, create a channel, call
`run_shell_command_with_callback`, and spawn a reader thread that feeds lines to
the renderer (or to `output.raw()` as non-TTY fallback). The renderer is created
in `execute_yaml_hook_with_rc` and passed down via `ExecContext`.

For sequential execution, the pattern is:

1. `start_job(name)` before running
2. Spawn channel, run command with callback
3. Poll channel in a loop, calling `update_job_output()` for each line
4. On completion, call `finish_job_success/failure()`

**Step 4: Update callers in `executor.rs`**

In `executor.rs:137` (`try_yaml_hook`), pass `self.config.output` to
`execute_yaml_hook_with_rc`.

In `executor.rs` line 210:

```rust
let result = yaml_executor::execute_yaml_hook_with_rc(
    hook_name,
    hook_def,
    ctx,
    output,
    source_dir,
    working_dir,
    rc,
    &self.config.output,
)?;
```

**Step 5: Run tests**

Run: `cargo test` Expected: all pass

**Step 6: Commit**

```bash
git add src/hooks/yaml_executor.rs src/hooks/executor.rs
git commit -m "feat: wire HookProgressRenderer into sequential YAML hook execution"
```

---

### Task 6: Wire HookProgressRenderer into YAML parallel execution

**Files:**

- Modify: `src/hooks/yaml_executor.rs:256-382` (execute_parallel)
- Modify: `src/hooks/yaml_executor.rs:448-760` (execute_dag_parallel)

**Step 1: Write test for parallel output**

```rust
#[test]
fn test_parallel_hook_captures_output() {
    let hook_def = HookDef {
        parallel: Some(true),
        jobs: Some(vec![
            JobDef {
                name: Some("job-a".to_string()),
                run: Some("echo 'output-a'".to_string()),
                ..Default::default()
            },
            JobDef {
                name: Some("job-b".to_string()),
                run: Some("echo 'output-b'".to_string()),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };
    let ctx = make_ctx();
    let mut output = TestOutput::default();

    let result = execute_yaml_hook(
        "test-hook",
        &hook_def,
        &ctx,
        &mut output,
        ".daft",
        Path::new("/tmp"),
    )
    .unwrap();

    assert!(result.success);
}
```

**Step 2: Run to verify it passes (baseline)**

Run: `cargo test test_parallel_hook_captures_output` Expected: PASS

**Step 3: Integrate renderer into parallel execution**

For `execute_parallel()`, the renderer must be shared across threads. Use
`Arc<Mutex<HookProgressRenderer>>`:

1. Create the renderer before spawning threads
2. Each thread gets an `Arc<Mutex<HookProgressRenderer>>` clone
3. Threads call `update_job_output()` through the mutex
4. After all threads complete, the main thread calls `finish_job_*()` for each
   result

For `execute_dag_parallel()`, same pattern but integrated with the existing
`DagState` mutex.

The key change in the thread body (around line 313):

```rust
// Instead of just calling run_shell_command directly:
let (tx, rx) = std::sync::mpsc::channel();
let result = run_shell_command_with_callback(
    &data.cmd, &data.env, &data.working_dir, &ctx_for_thread,
    Duration::from_secs(300), Some(tx),
);

// Drain any remaining lines from the channel
for line in rx.try_iter() {
    renderer.lock().unwrap().update_job_output(&data.name, &line);
}
```

However, since `run_shell_command_with_callback` blocks until the command
finishes, we need to drain the channel from a separate reader thread or use a
non-blocking approach. The simplest approach: spawn a reader thread per job that
drains the channel and updates the renderer, while the main job thread runs the
command.

**Step 4: Run tests**

Run: `cargo test` Expected: all pass

**Step 5: Commit**

```bash
git add src/hooks/yaml_executor.rs
git commit -m "feat: wire HookProgressRenderer into parallel YAML hook execution"
```

---

### Task 7: Add non-TTY fallback

**Files:**

- Modify: `src/output/hook_progress.rs`

**Step 1: Write test**

```rust
#[test]
fn test_plain_renderer_streams_raw() {
    let config = HookOutputConfig::default();
    let mut renderer = PlainHookRenderer::new();
    let mut output_lines = Vec::new();

    renderer.start_job("test-job");
    renderer.update_job_output("test-job", "line 1", &mut output_lines);
    renderer.update_job_output("test-job", "line 2", &mut output_lines);
    renderer.finish_job_success("test-job", Duration::from_secs(2), &mut output_lines);

    assert!(output_lines.iter().any(|l| l.contains("line 1")));
    assert!(output_lines.iter().any(|l| l.contains("line 2")));
    assert!(output_lines.iter().any(|l| l.contains("✓")));
}
```

**Step 2: Run to fail**

Run: `cargo test test_plain_renderer` Expected: FAIL

**Step 3: Implement PlainHookRenderer**

Add a `PlainHookRenderer` that prints lines directly without spinners. Use a
trait or enum to abstract over both:

```rust
/// Trait for hook progress rendering, with TTY and plain implementations.
pub enum HookRenderer {
    /// Rich output with spinners and rolling windows (TTY mode).
    Progress(HookProgressRenderer),
    /// Plain line-by-line output (non-TTY / CI mode).
    Plain(PlainHookRenderer),
}

impl HookRenderer {
    /// Create the appropriate renderer based on terminal detection.
    pub fn auto(config: &HookOutputConfig) -> Self {
        if console::Term::stderr().is_term() {
            HookRenderer::Progress(HookProgressRenderer::new(config))
        } else {
            HookRenderer::Plain(PlainHookRenderer::new())
        }
    }
}
```

`PlainHookRenderer` just calls `output.raw()` for each line, similar to legacy
hook behavior.

**Step 4: Run tests**

Run: `cargo test hook_progress` Expected: PASS

**Step 5: Commit**

```bash
git add src/output/hook_progress.rs
git commit -m "feat: add non-TTY plain fallback for hook output"
```

---

### Task 8: Wire renderer into legacy hook execution

**Files:**

- Modify: `src/hooks/executor.rs:371-440` (execute_hook_file)
- Modify: `src/hooks/executor.rs:233-320` (execute_legacy)

**Step 1: Modify legacy execution to use HookRenderer**

In `execute_legacy()`, replace the direct `output.raw()` streaming with the
renderer. Create a `HookRenderer::auto()` before the hook loop, call
`start_job()` before execution, `update_job_output()` for each line, and
`finish_job_*()` after.

This replaces the `output.step("Running {} hook...")` at line 305 and the
streaming at lines 401/411.

**Step 2: Run integration tests**

Run: `mise run test-integration` Expected: all pass

**Step 3: Commit**

```bash
git add src/hooks/executor.rs
git commit -m "feat: use HookRenderer for legacy hook execution output"
```

---

### Task 9: Update documentation

**Files:**

- Modify: `docs/guide/hooks.md` (add output configuration section)
- Modify: `src/settings.rs` (doc comments at top)

**Step 1: Add output configuration docs**

Add a section to the hooks guide explaining the new `daft.hooks.output.*`
settings with examples.

**Step 2: Commit**

```bash
git add docs/guide/hooks.md src/settings.rs
git commit -m "docs: add hook output configuration documentation"
```

---

### Task 10: Run full CI checks

**Step 1: Format**

Run: `mise run fmt`

**Step 2: Clippy**

Run: `mise run clippy` Expected: zero warnings

**Step 3: Unit tests**

Run: `mise run test-unit` Expected: all pass

**Step 4: Integration tests**

Run: `mise run test-integration` Expected: all pass

**Step 5: Man pages**

Run: `mise run verify-man` Expected: up to date (no command help text changed)

**Step 6: Commit any fixups**

```bash
git add -A && git commit -m "chore: fix lint and formatting"
```
