//! Port between the runner core and project-specific command execution.
//!
//! The runner core (sandbox lifecycle, step scheduling, assertion checking)
//! is intentionally agnostic about what kind of binary it is exercising —
//! it knows how to lay out a sandbox, expand `$VAR`s, and feed a command
//! string to "something that runs it." That something is a
//! [`CommandExecutor`].
//!
//! For daft, [`super::daft_executor::DaftCommandExecutor`] implements the
//! port: it prepends `target/release/` to `PATH`, injects the `DAFT_*` feature
//! flags that suppress daemon spawns during tests, and isolates `DAFT_CONFIG_DIR`
//! / `DAFT_DATA_DIR` per sandbox. For test-doubling, an in-memory fake
//! suffices (see the `FakeExecutor` in `runner::tests`).
//!
//! Why a port, not direct calls: the rest of umbrella #509 plugs into "how a
//! step runs" — clonefile snapshots (#511), ramdisk sandboxes (#512), shared
//! fixture caches (#513), shared binary dirs (#514). Each becomes an adapter
//! change rather than a runner-core change. See the issue #516 PR description
//! for the mapping.

use anyhow::Result;
use std::path::Path;

use super::sandbox::Sandbox;

/// Executes scenario step commands against a sandbox.
///
/// Implementations are responsible for:
/// - Expanding `$VAR` references in the command (typically via
///   [`Sandbox::expand_vars`]).
/// - Constructing the process environment (PATH, project-specific env vars,
///   git identity isolation).
/// - Spawning the command in `cwd` and capturing stdout / stderr / exit code.
///
/// The runner core resolves `cwd` from the step's `cwd:` field (or defaults
/// it to `sandbox.work_dir`) before calling — keeping that logic in the core
/// means the executor doesn't need to know the [`super::schema::Step`] shape.
///
/// Implementations must be `Send + Sync` so the parallel scheduler can share
/// a single executor across rayon workers.
pub trait CommandExecutor: Send + Sync {
    /// Run `command` inside `sandbox` with the given working directory and
    /// return its captured output.
    ///
    /// `command` is the raw step command from the scenario YAML, before
    /// variable expansion — implementations call `sandbox.expand_vars` on it
    /// (so a fake executor recording invocations sees expanded, post-`$VAR`
    /// strings, matching what the user-facing command actually executed).
    fn execute(&self, command: &str, cwd: &Path, sandbox: &Sandbox) -> Result<CommandOutput>;
}

/// Captured result of running a step command.
#[derive(Default)]
pub struct CommandOutput {
    /// Process exit code (`-1` when killed by a signal — matches the
    /// long-standing convention of [`std::process::ExitStatus::code`] returning
    /// `None`).
    pub exit_code: i32,
    /// Bytes written to stdout.
    pub stdout: Vec<u8>,
    /// Bytes written to stderr.
    pub stderr: Vec<u8>,
}
