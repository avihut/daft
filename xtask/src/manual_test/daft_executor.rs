//! Daft-specific adapter for the [`CommandExecutor`] port.
//!
//! Owns every assumption the runner makes about daft itself:
//!   - `target/release/` is on `PATH` so locally-built `daft` wins over
//!     any system install. The directory is resolvable via
//!     `DAFT_BINARY_DIR=<path>` (#514) so a worktree can point at a
//!     shared bin that was already built somewhere else — under
//!     `<git-common-dir>/.daft-shared-bin/<hash>/target/release/` by
//!     the helper `mise-tasks/test/manual/_shared_bin_lib.sh`.
//!     Default when unset stays at `<project_root>/target/release`.
//!   - `DAFT_CONFIG_DIR` and `DAFT_DATA_DIR` are per-sandbox so suites running
//!     in parallel never read each other's trust / repo state.
//!   - `DAFT_TESTING=1` prevents orphaned background processes from
//!     accumulating across a parallel suite — load average used to climb
//!     into the hundreds without it. The flag flips daft's central
//!     `should_skip_background_tasks` gate in `src/main.rs`, which is the
//!     single source of truth for update-check / trust-prune / log-clean
//!     suppression under tests.
//!
//! Keeping all of this in the adapter is what lets the runner core compile
//! and run against a non-daft executor (see [`super::runner`]'s `FakeExecutor`
//! tests).

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::executor::{CommandExecutor, CommandOutput};
use super::sandbox::Sandbox;

/// Adapter that runs scenario commands against a locally-built `daft`.
pub struct DaftCommandExecutor {
    /// Directory containing the daft binary (and any symlinked multicalls
    /// like `git-worktree-clone`). Prepended to `PATH` so locally-built
    /// binaries win over a system install.
    binary_dir: PathBuf,
    /// Per-sandbox config dir surfaced as `DAFT_CONFIG_DIR`.
    daft_config_dir: PathBuf,
    /// Per-sandbox data dir surfaced as `DAFT_DATA_DIR` and `$DAFT_DATA_DIR`
    /// (the var-expansion form is registered on the sandbox at construction
    /// time so scenario commands can reference it directly).
    daft_data_dir: PathBuf,
    /// Per-sandbox state dir surfaced as `DAFT_STATE_DIR`. Isolates the
    /// SQLite store (job records, visitor-config seeds) and the coordinator
    /// socket so scenario runs never read or pollute the developer's real
    /// `~/.local/state/daft`.
    ///
    /// Deliberately NOT under the sandbox base dir: the coordinator binds a
    /// unix socket at `<state>/coordinator-<uuid>.sock`, and `sun_path` caps
    /// at ~104 bytes on macOS — the sandbox base path alone nearly exhausts
    /// that. A short `TempDir` directly under the system temp root keeps the
    /// socket bindable and cleans itself up when the executor drops.
    daft_state_dir: tempfile::TempDir,
}

impl DaftCommandExecutor {
    /// Construct an adapter for `sandbox` and register the daft-specific
    /// variables (`$BINARY_DIR`, `$DAFT_DATA_DIR`) on the sandbox so scenario
    /// commands can refer to them.
    pub fn new_for_sandbox(sandbox: &mut Sandbox, project_root: &Path) -> Result<Self> {
        let binary_dir = resolve_binary_dir(project_root);
        let daft_config_dir = sandbox.base_dir.join("daft-config");
        let daft_data_dir = sandbox.base_dir.join("daft-data");
        // `tempdir_in("/tmp")`, not `tempdir()`: macOS's $TMPDIR
        // (/var/folders/…/T/) is ~50 chars by itself, which would put the
        // socket path right back over the sun_path limit.
        let daft_state_dir = tempfile::Builder::new()
            .prefix("daft-st-")
            .tempdir_in("/tmp")
            .context("creating daft state dir")?;

        std::fs::create_dir_all(&daft_config_dir)
            .with_context(|| format!("creating daft config dir: {}", daft_config_dir.display()))?;
        std::fs::create_dir_all(&daft_data_dir)
            .with_context(|| format!("creating daft data dir: {}", daft_data_dir.display()))?;

        // Surface the adapter-managed paths to scenario commands. These were
        // historically baked into the sandbox's own var store; keeping them
        // here is what lets the sandbox stay daft-agnostic.
        sandbox.register_var("BINARY_DIR", binary_dir.to_string_lossy().into_owned());
        // `$WORKSPACE_ROOT` is the canonical absolute path to the daft
        // checkout that hosts the test runner. Most scenarios don't need
        // it — `$BINARY_DIR` is enough for running daft against a
        // sandbox. But `runner-output:end-to-end` re-invokes `xtask`
        // against a fixture under `tests/manual/scenarios/.../_fixtures/`,
        // and after #514 `$BINARY_DIR` may live under
        // `<git-common-dir>/.daft-shared-bin/<hash>/target/release/`
        // (not `<workspace>/target/release/`), so `$BINARY_DIR/../..`
        // is no longer a valid workspace anchor. Scenarios that need
        // a workspace-relative path must `cd "$WORKSPACE_ROOT"` first.
        sandbox.register_var(
            "WORKSPACE_ROOT",
            project_root.to_string_lossy().into_owned(),
        );
        sandbox.register_var(
            "DAFT_DATA_DIR",
            daft_data_dir.to_string_lossy().into_owned(),
        );

        Ok(Self {
            binary_dir,
            daft_config_dir,
            daft_data_dir,
            daft_state_dir,
        })
    }

    /// Build the environment passed to the step's subprocess (direct exec
    /// or `bash -c` fallback).
    ///
    /// Layered so safety-critical entries (git identity, daemon suppression,
    /// config-dir isolation) cannot be overridden by scenario-defined env —
    /// scenario vars come first, safety vars last.
    fn build_env(&self, sandbox: &Sandbox) -> HashMap<String, String> {
        let mut env = HashMap::new();

        // Scenario vars first — these can be overridden by safety vars below.
        for (k, v) in sandbox.scenario_vars() {
            env.insert(k.clone(), v.clone());
        }

        // Safety vars LAST — cannot be overridden by scenario definitions.
        // Git identity — local to test, never touches global config.
        env.insert("GIT_AUTHOR_NAME".into(), "Manual Test".into());
        env.insert("GIT_AUTHOR_EMAIL".into(), "test@daft.test".into());
        env.insert("GIT_COMMITTER_NAME".into(), "Manual Test".into());
        env.insert("GIT_COMMITTER_EMAIL".into(), "test@daft.test".into());
        env.insert(
            "GIT_CONFIG_GLOBAL".into(),
            sandbox.git_config_path.to_string_lossy().into_owned(),
        );

        // Disable every daemon-style background spawn: the test harness
        // invokes `daft` many times back-to-back, and any detached child that
        // survives its parent (e.g. `daft __clean-logs`) accumulates as
        // init-reparented orphans and steals CPU — visible as load-average
        // climbing into the hundreds during parallel runs.
        //
        // `DAFT_TESTING` alone is sufficient: it flips daft's central
        // `should_skip_background_tasks` gate in main.rs, which suppresses
        // every `maybe_*` spawn helper before it runs. The per-feature env
        // vars (`DAFT_NO_UPDATE_CHECK` / `DAFT_NO_TRUST_PRUNE` /
        // `DAFT_NO_LOG_CLEAN`) remain as user-facing opt-outs but are
        // redundant for the test runner.
        env.insert("DAFT_TESTING".into(), "1".into());
        env.insert(
            "DAFT_CONFIG_DIR".into(),
            self.daft_config_dir.to_string_lossy().into_owned(),
        );
        env.insert(
            "DAFT_DATA_DIR".into(),
            self.daft_data_dir.to_string_lossy().into_owned(),
        );
        env.insert(
            "DAFT_STATE_DIR".into(),
            self.daft_state_dir.path().to_string_lossy().into_owned(),
        );

        // PATH — binary_dir first so locally-built daft wins. `to_string_lossy`
        // (not `display`) is the right idiom here: this is a string going into
        // the subprocess env, not human-readable terminal output.
        let existing_path = std::env::var("PATH").unwrap_or_default();
        env.insert(
            "PATH".into(),
            format!("{}:{existing_path}", self.binary_dir.to_string_lossy()),
        );

        env
    }

    /// Resolve `name` to an absolute binary path using the same PATH
    /// composition as [`Self::build_env`] (`binary_dir` first, then the
    /// runner's ambient `PATH`). Without this step, the fast-path
    /// `Command::new(name)` would defer to libc's `execvp`, which performs
    /// the lookup against the **runner's** `PATH` rather than the env we
    /// pass via `.envs()` — so locally built `daft` and `git-worktree-*`
    /// (in `target/release/`) would ENOENT. Bash didn't have this problem
    /// because the lookup happened inside the bash child, which saw the
    /// env we set. Returns `None` if no match is found; the caller falls
    /// back to `bash -c`, which preserves the original error reporting.
    fn resolve_binary(&self, name: &str) -> Option<PathBuf> {
        if name.contains('/') {
            let p = Path::new(name);
            return is_executable(p).then(|| p.to_path_buf());
        }
        let direct = self.binary_dir.join(name);
        if is_executable(&direct) {
            return Some(direct);
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|p| is_executable(p))
    }
}

/// Pick the directory that holds the `daft` binary (plus its multicall
/// symlinks) the test runner will use.
///
/// `DAFT_BINARY_DIR=<path>` overrides; otherwise default to
/// `<project_root>/target/release/`. The override lets `mise run
/// test:manual` build into a shared content-hashed location under
/// `<git-common-dir>/.daft-shared-bin/<hash>/target/release/` so that
/// sibling worktrees at the same source state share a single build
/// (see `mise-tasks/test/manual/_shared_bin_lib.sh`).
///
/// Lives in the daft-specific adapter rather than `env.rs` so the
/// `DAFT_BINARY_DIR` env var name does not leak into the runner core
/// (per CLAUDE.md's "runner core never names daft" boundary).
fn resolve_binary_dir(project_root: &Path) -> PathBuf {
    match std::env::var("DAFT_BINARY_DIR") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => project_root.join("target/release"),
    }
}

/// Whether `p` is a regular file with at least one execute bit set
/// (owner/group/other). Matches `execvp`'s "would this succeed?" check
/// — without it, `resolve_binary` could return a non-executable regular
/// file (a data file, a config) that happens to share the command name,
/// the fast path would take it, and `spawn` would fail with `EPERM`
/// rather than gracefully falling back to bash.
fn is_executable(p: &Path) -> bool {
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Strip a trailing `2>&1` (with optional surrounding whitespace) and
/// return the rest of the command alongside a flag indicating whether the
/// redirect was present. The fast path simulates the redirect after exec
/// by appending the child's stderr buffer onto its stdout buffer; bash
/// would have interleaved the streams at write time. For our assertion
/// patterns (`output_contains` against substrings) this is good enough.
///
/// Without this, every command ending in `2>&1` would bail to bash on the
/// `&` byte-scan — that's ~200 scenario steps, the single biggest source
/// of bash fallbacks.
fn strip_trailing_stderr_redirect(command: &str) -> (String, bool) {
    let trimmed = command.trim_end();
    if let Some(rest) = trimmed.strip_suffix("2>&1") {
        return (rest.trim_end().to_string(), true);
    }
    (command.to_string(), false)
}

/// Split a leading run of `NAME=VALUE` tokens off `argv`. Each leading
/// token whose pre-`=` portion is a valid identifier (alpha-underscore
/// start, alpha-numeric-underscore continuation) becomes an env-var
/// override on the child; the remaining argv starts at the first
/// non-env-prefix token. Matches bash's `NAME=VALUE cmd …` syntax.
///
/// Returns `None` if the remaining argv is empty — i.e. the command was
/// nothing but env assignments. Those need bash because bash assigns
/// them into the shell's own env and exits 0; direct exec has no
/// equivalent.
fn split_env_prefix(argv: Vec<String>) -> Option<(Vec<(String, String)>, Vec<String>)> {
    let mut env_overrides = Vec::new();
    let mut iter = argv.into_iter();
    let mut rest: Vec<String> = Vec::new();
    while let Some(tok) = iter.next() {
        if let Some((name, value)) = tok.split_once('=') {
            let mut chars = name.chars();
            let first_ok = chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
            let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
            if first_ok && rest_ok {
                env_overrides.push((name.to_string(), value.to_string()));
                continue;
            }
        }
        rest.push(tok);
        rest.extend(iter);
        break;
    }
    if rest.is_empty() {
        return None;
    }
    Some((env_overrides, rest))
}

/// Decide whether `command` can be exec'd directly, skipping the `bash -c`
/// wrapper. Returns `Some(argv)` iff a cheap byte-scan finds no shell
/// metacharacter and `shlex::split` produces a non-empty argv.
///
/// Departures from #560's literal spec, both made to preserve current
/// behaviour rather than to widen scope:
///   - `${` is on the bail list. `Sandbox::expand_vars` only resolves
///     `$VARNAME` (uppercase, no braces), so a literal `${VAR}` would be
///     expanded by bash today and would survive verbatim under direct
///     exec.
///   - `\n` and `\r` are on the bail list. YAML `run: |` blocks become
///     multi-line strings (one statement per line). `shlex::split` treats
///     newlines as plain whitespace, which would collapse a three-line
///     script into a single argv passed to the first token — usually
///     wrong (e.g., `cd /tmp/foo` is a builtin under bash but a no-op
///     subprocess when `/usr/bin/cd` exists on macOS, swallowing all
///     subsequent lines as ignored args).
///
/// Bare `$` is not bailed on: by the time this runs, `expand_vars` has
/// already substituted every sandbox-registered variable, so any
/// surviving `$` is a literal.
fn try_direct_argv(command: &str) -> Option<Vec<String>> {
    if command.contains(['|', '>', '<', '&', ';', '`', '*', '?', '[', '\n', '\r'])
        || command.contains("$(")
        || command.contains("${")
    {
        return None;
    }
    shlex::split(command).filter(|argv| !argv.is_empty())
}

impl CommandExecutor for DaftCommandExecutor {
    fn execute(&self, command: &str, cwd: &Path, sandbox: &Sandbox) -> Result<CommandOutput> {
        let expanded = sandbox.expand_vars(command);
        // Fast path: when the command is a plain invocation with no shell
        // features and its binary resolves on our composed PATH, skip the
        // `bash -c` wrapper. Bash spawn costs ~17ms on macOS M1 Max. See
        // #560 for the full motivation.
        //
        // Two pre-passes widen the fast-path catchment beyond `try_direct_argv`'s
        // strict metachar reject. Both preserve current behaviour; neither
        // expands the runner's contract:
        //   - Trailing `2>&1` is stripped and faked after exec by appending
        //     the child's stderr buffer onto stdout. The `&` byte that would
        //     otherwise force bash is removed before the scan.
        //   - Leading `NAME=VALUE` tokens become `Command::env` overrides on
        //     the child rather than running through bash's env-prefix syntax.
        let (stripped, merge_stderr_into_stdout) = strip_trailing_stderr_redirect(&expanded);
        let fast_cmd = try_direct_argv(&stripped)
            .and_then(split_env_prefix)
            .and_then(|(env_overrides, argv)| {
                let bin = self.resolve_binary(&argv[0])?;
                let mut c = Command::new(bin);
                c.args(&argv[1..]);
                // Apply env-prefix overrides BEFORE `.envs(build_env(...))`
                // below. The safety layer (`DAFT_TESTING`, `GIT_AUTHOR_*`, …)
                // intentionally wins on conflict — same precedence bash sees
                // because `build_env` runs after the prefix in either path.
                for (k, v) in env_overrides {
                    c.env(k, v);
                }
                Some(c)
            });
        let took_fast_path = fast_cmd.is_some();
        let mut cmd = fast_cmd.unwrap_or_else(|| {
            let mut c = Command::new("bash");
            c.args(["-c", &expanded]);
            c
        });
        // process_group(0) puts the child in its own process group so the
        // terminal's SIGINT (sent to the foreground process group) doesn't
        // hit it. Without this, Ctrl+C delivered to the runner is also
        // delivered to every in-flight subprocess and they exit with
        // signal-killed status — the runner then sees a "step failed"
        // (non-zero exit) and marks the scenario as Fail instead of
        // Cancelled. The runner's own ctrlc handler is the sole intended
        // observer of SIGINT; subprocesses must be insulated from it.
        let output = cmd
            .process_group(0)
            .current_dir(cwd)
            .envs(self.build_env(sandbox))
            .output()
            .with_context(|| format!("Failed to execute: {expanded}"))?;

        let mut stdout = output.stdout;
        let mut stderr = output.stderr;
        if took_fast_path && merge_stderr_into_stdout {
            stdout.append(&mut stderr);
        }
        Ok(CommandOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout,
            stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Serialises every test in this module that constructs a
    /// `DaftCommandExecutor`. `new_for_sandbox` reads `DAFT_BINARY_DIR`
    /// (via `resolve_binary_dir`), and `cargo test` runs tests in
    /// parallel within a single binary — without a process-wide lock,
    /// the env-var override test could race the no-override fallback
    /// tests and flip the assertions on either side.
    ///
    /// Pairs with the helpers below: every test that hits the
    /// constructor acquires this lock first and explicitly establishes
    /// the env-var state it expects.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// RAII guard that holds [`ENV_LOCK`], snapshots `DAFT_BINARY_DIR`'s
    /// state on construction, and restores it on drop. Lets each test
    /// freely set / unset the env var without leaking the change to
    /// other tests.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn acquire() -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
            let prev = std::env::var("DAFT_BINARY_DIR").ok();
            // SAFETY: edition 2024 marks `remove_var`/`set_var` as
            // `unsafe fn` because env mutation is process-global and
            // not thread-safe. Held lock serialises every constructor
            // test in this module against itself; no other test in
            // xtask reads/writes `DAFT_BINARY_DIR`. CLAUDE.md allows
            // unsafe in tests.
            unsafe { std::env::remove_var("DAFT_BINARY_DIR") };
            Self { _lock: lock, prev }
        }

        fn set(&self, value: &Path) {
            unsafe { std::env::set_var("DAFT_BINARY_DIR", value) };
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("DAFT_BINARY_DIR", v),
                    None => std::env::remove_var("DAFT_BINARY_DIR"),
                }
            }
        }
    }

    /// Dummy `project_root` value. The adapter only uses it to compute
    /// `project_root.join("target/release")` for PATH construction — no I/O
    /// against this path, so a non-existent dummy is fine.
    fn project_root() -> PathBuf {
        PathBuf::from("/nonexistent/dummy-project-root")
    }

    /// Build a `Sandbox` whose `base_dir` points at a fresh temp directory.
    /// The returned `TempDir` must outlive the sandbox: dropping it removes
    /// the directory tree.
    fn sandbox_with_tempdir() -> (Sandbox, TempDir) {
        let tmp = tempfile::tempdir().expect("create temp sandbox base dir");
        let mut sandbox = Sandbox::new_with_vars(HashMap::new());
        sandbox.base_dir = tmp.path().to_path_buf();
        (sandbox, tmp)
    }

    #[test]
    fn build_env_has_git_identity_and_daft_flags() {
        let _env = EnvGuard::acquire();
        let (mut sandbox, _tmp) = sandbox_with_tempdir();

        let exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, &project_root()).unwrap();
        let env = exec.build_env(&sandbox);

        assert_eq!(env.get("GIT_AUTHOR_NAME").unwrap(), "Manual Test");
        // `DAFT_TESTING=1` is the single daemon-suppression contract — it
        // trips daft's central `should_skip_background_tasks` gate, which
        // makes the per-feature `DAFT_NO_UPDATE_CHECK` / `DAFT_NO_TRUST_PRUNE`
        // / `DAFT_NO_LOG_CLEAN` flags redundant for the test runner.
        assert_eq!(env.get("DAFT_TESTING").unwrap(), "1");
        // The runner intentionally stops setting per-feature suppression
        // flags; DAFT_TESTING=1 alone gates all three maybe_* startup helpers
        // via daft::should_skip_background_tasks. Symmetric messages so any
        // future regression points straight at the contract.
        const SINGLE_FLAG_CONTRACT: &str =
            "DAFT_TESTING=1 is the single daemon-suppression contract; per-feature \
             DAFT_NO_* flags must not be reintroduced by the runner adapter";
        assert!(
            !env.contains_key("DAFT_NO_UPDATE_CHECK"),
            "{SINGLE_FLAG_CONTRACT}"
        );
        assert!(
            !env.contains_key("DAFT_NO_TRUST_PRUNE"),
            "{SINGLE_FLAG_CONTRACT}"
        );
        assert!(
            !env.contains_key("DAFT_NO_LOG_CLEAN"),
            "{SINGLE_FLAG_CONTRACT}"
        );
        assert!(env.get("PATH").unwrap().contains("target/release"));
        assert!(env.get("DAFT_CONFIG_DIR").unwrap().contains("daft-config"));
        assert!(env.get("DAFT_DATA_DIR").unwrap().contains("daft-data"));
    }

    #[test]
    fn registers_binary_dir_and_data_dir_vars_on_sandbox() {
        let _env = EnvGuard::acquire();
        let (mut sandbox, _tmp) = sandbox_with_tempdir();

        let _exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, &project_root()).unwrap();

        // After construction, $BINARY_DIR and $DAFT_DATA_DIR are expandable
        // through the sandbox's normal variable expansion.
        let expanded = sandbox.expand_vars("$BINARY_DIR/daft and data=$DAFT_DATA_DIR");
        assert!(expanded.contains("target/release"));
        assert!(expanded.contains("daft-data"));
    }

    #[test]
    fn try_direct_argv_accepts_plain_invocations() {
        let cases: &[(&str, &[&str])] = &[
            ("daft list", &["daft", "list"]),
            (
                "git-worktree-clone --layout contained /tmp/foo",
                &["git-worktree-clone", "--layout", "contained", "/tmp/foo"],
            ),
            ("echo hello", &["echo", "hello"]),
            (
                "daft hooks list --json",
                &["daft", "hooks", "list", "--json"],
            ),
        ];
        for (input, expected) in cases {
            let argv = try_direct_argv(input)
                .unwrap_or_else(|| panic!("expected fast-path for {input:?}"));
            let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
            assert_eq!(argv, expected, "input={input:?}");
        }
    }

    #[test]
    fn try_direct_argv_bails_on_each_shell_metachar() {
        // Each input contains exactly one metachar from the bail list. If
        // any of these slip past the byte-scan, the fast path would change
        // behaviour vs the bash wrapper.
        let cases = &[
            "a | b", "a > b", "a < b", "a && b", "a; b", "`cmd`", "a $(b)", "a ${b}", "a*", "a?",
            "a[bc]",
        ];
        for input in cases {
            assert!(
                try_direct_argv(input).is_none(),
                "should bail on metachar: {input:?}",
            );
        }
    }

    #[test]
    fn try_direct_argv_bails_on_multiline_scripts() {
        // YAML `run: |` blocks deliver one statement per line. Without
        // the newline bail, shlex would collapse the lines into a single
        // argv (e.g. `cd /tmp\ngit merge x` → `["cd","/tmp","git","merge","x"]`)
        // and `/usr/bin/cd` would silently ignore the rest. Routing
        // multi-line scripts to bash preserves their per-line semantics.
        assert!(try_direct_argv("cd /tmp/foo\ngit merge feature").is_none());
        assert!(try_direct_argv("echo a\r\necho b").is_none());
    }

    #[test]
    fn try_direct_argv_rejects_empty_and_whitespace() {
        assert!(try_direct_argv("").is_none());
        assert!(try_direct_argv("   ").is_none());
        assert!(try_direct_argv("\t\n").is_none());
    }

    #[test]
    fn try_direct_argv_is_conservative_about_quoted_metachars() {
        // The byte-scan is unaware of quoting. A `;` inside quotes routes
        // to bash even though direct exec would also be safe. This is the
        // deliberate trade-off: simpler check, no shlex pre-pass needed
        // before the cheap reject.
        assert!(try_direct_argv("daft foo \"bar;baz\"").is_none());
        assert!(try_direct_argv("daft foo 'has a *'").is_none());
    }

    #[test]
    fn resolve_binary_prefers_binary_dir_over_ambient_path() {
        let _env = EnvGuard::acquire();
        // Regression for the field-test bug behind #560's first run: the
        // fast path's `Command::new(name)` defers to libc `execvp`, which
        // searches the runner's ambient PATH and so cannot find locally
        // built `daft` in `target/release/`. `resolve_binary` must hit
        // `binary_dir` first to preserve the override the bash wrapper
        // (which read the child env's PATH) gave us for free.
        let project_tmp = tempfile::tempdir().expect("create project root tempdir");
        let binary_dir = project_tmp.path().join("target/release");
        std::fs::create_dir_all(&binary_dir).unwrap();
        let fake_bin = binary_dir.join("daft-560-shim");
        std::fs::write(&fake_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(&fake_bin).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_bin, perms).unwrap();

        let (mut sandbox, _tmp) = sandbox_with_tempdir();
        let exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, project_tmp.path()).unwrap();

        let resolved = exec
            .resolve_binary("daft-560-shim")
            .expect("must resolve from binary_dir");
        assert_eq!(resolved, fake_bin);
    }

    #[test]
    fn resolve_binary_skips_non_executable_regular_files() {
        let _env = EnvGuard::acquire();
        // Defensive: a regular file with the right name but no execute bit
        // must NOT resolve. Without the `is_executable` check, the fast
        // path would take this path and spawn would fail with EPERM rather
        // than gracefully falling back to bash.
        let project_tmp = tempfile::tempdir().expect("create project root tempdir");
        let binary_dir = project_tmp.path().join("target/release");
        std::fs::create_dir_all(&binary_dir).unwrap();
        let non_exec = binary_dir.join("daft-560-no-exec");
        std::fs::write(&non_exec, b"not-a-binary").unwrap();
        // Permissions default to 0o644 — no execute bit. No chmod.

        let (mut sandbox, _tmp) = sandbox_with_tempdir();
        let exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, project_tmp.path()).unwrap();
        assert!(exec.resolve_binary("daft-560-no-exec").is_none());
    }

    #[test]
    fn resolve_binary_returns_none_for_missing() {
        let _env = EnvGuard::acquire();
        let (mut sandbox, _tmp) = sandbox_with_tempdir();
        let exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, &project_root()).unwrap();
        assert!(exec
            .resolve_binary("definitely-not-a-real-binary-zzz")
            .is_none());
    }

    #[test]
    fn execute_runs_fast_path_command_directly() {
        let _env = EnvGuard::acquire();
        let (mut sandbox, _tmp) = sandbox_with_tempdir();
        let exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, &project_root()).unwrap();

        let out = exec
            .execute("echo hi-from-fast", &sandbox.base_dir, &sandbox)
            .unwrap();

        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi-from-fast");
    }

    #[test]
    fn strip_trailing_stderr_redirect_handles_2_amp_1() {
        let (rest, had) = strip_trailing_stderr_redirect("daft list 2>&1");
        assert!(had);
        assert_eq!(rest, "daft list");

        let (rest, had) = strip_trailing_stderr_redirect("daft list   2>&1   ");
        assert!(had);
        assert_eq!(rest, "daft list");

        let (rest, had) = strip_trailing_stderr_redirect("daft list");
        assert!(!had);
        assert_eq!(rest, "daft list");

        // Embedded `2>&1` mid-command stays in the command (will route to
        // bash via the `&` byte-scan); only trailing strips.
        let (rest, had) = strip_trailing_stderr_redirect("daft list 2>&1 | grep x");
        assert!(!had);
        assert_eq!(rest, "daft list 2>&1 | grep x");
    }

    #[test]
    fn split_env_prefix_peels_leading_assignments() {
        let argv = vec![
            "NO_COLOR=1".to_string(),
            "daft".to_string(),
            "list".to_string(),
        ];
        let (env, rest) = split_env_prefix(argv).unwrap();
        assert_eq!(env, vec![("NO_COLOR".to_string(), "1".to_string())]);
        assert_eq!(rest, vec!["daft", "list"]);

        let argv = vec![
            "FOO=a".to_string(),
            "BAR=b".to_string(),
            "cmd".to_string(),
            "--flag".to_string(),
        ];
        let (env, rest) = split_env_prefix(argv).unwrap();
        assert_eq!(env.len(), 2);
        assert_eq!(rest, vec!["cmd", "--flag"]);

        // No env prefix — pass through unchanged.
        let argv = vec!["daft".to_string(), "list".to_string()];
        let (env, rest) = split_env_prefix(argv).unwrap();
        assert!(env.is_empty());
        assert_eq!(rest, vec!["daft", "list"]);

        // First token contains `=` but invalid identifier — treated as
        // binary argument, not env prefix. (Defensive — should be rare.)
        let argv = vec!["1BAD=x".to_string(), "cmd".to_string()];
        let (env, rest) = split_env_prefix(argv).unwrap();
        assert!(env.is_empty());
        assert_eq!(rest, vec!["1BAD=x", "cmd"]);

        // All-env, no command — caller routes to bash.
        let argv = vec!["FOO=1".to_string()];
        assert!(split_env_prefix(argv).is_none());
    }

    #[test]
    fn execute_fast_path_merges_stderr_into_stdout_on_2_amp_1() {
        let _env = EnvGuard::acquire();
        // Real coverage for the `merge_stderr_into_stdout` branch. After
        // stripping `2>&1`, the remaining `ls /definitely-nonexistent-...`
        // has no shell metacharacters, so `try_direct_argv` succeeds and
        // we take the fast path. `ls` exits 1 and writes its error to
        // stderr; the merge should land the error in stdout and leave
        // stderr empty.
        let (mut sandbox, _tmp) = sandbox_with_tempdir();
        let exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, &project_root()).unwrap();

        let out = exec
            .execute(
                "ls /definitely-nonexistent-daft-test-path 2>&1",
                &sandbox.base_dir,
                &sandbox,
            )
            .unwrap();

        assert_ne!(out.exit_code, 0, "ls of missing path should fail");
        assert!(
            !out.stdout.is_empty(),
            "fast-path merge must put ls's error message into stdout"
        );
        assert!(
            out.stderr.is_empty(),
            "stderr should be empty after the merge"
        );
    }

    #[test]
    fn execute_bash_fallback_preserves_native_2_amp_1_semantics() {
        let _env = EnvGuard::acquire();
        // Inner `sh -c "...; ...; ..."` contains `;` and `>` in `1>&2`,
        // so even after stripping trailing `2>&1` the byte-scan still
        // bails and routes to bash. Bash then natively handles `2>&1`
        // on the outer command. This locks in that mixed env-prefix +
        // shell-feature commands still work end-to-end.
        let (mut sandbox, _tmp) = sandbox_with_tempdir();
        let exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, &project_root()).unwrap();

        let out = exec
            .execute(
                "MY_VAR=hi sh -c \"echo to-stdout; echo to-stderr 1>&2\" 2>&1",
                &sandbox.base_dir,
                &sandbox,
            )
            .unwrap();

        assert_eq!(out.exit_code, 0);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("to-stdout"), "stdout: {stdout}");
        assert!(stdout.contains("to-stderr"), "stdout: {stdout}");
    }

    #[test]
    fn execute_falls_back_to_bash_when_command_uses_shell_features() {
        let _env = EnvGuard::acquire();
        let (mut sandbox, _tmp) = sandbox_with_tempdir();
        let exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, &project_root()).unwrap();

        // `&&` forces the bash fallback; the fast path would produce a
        // single-line stdout treating `&&` and the second `echo` as
        // arguments to the first `echo`.
        let out = exec
            .execute(
                "echo hi-from-bash && echo and-again",
                &sandbox.base_dir,
                &sandbox,
            )
            .unwrap();

        assert_eq!(out.exit_code, 0);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut lines = stdout.lines();
        assert_eq!(lines.next(), Some("hi-from-bash"));
        assert_eq!(lines.next(), Some("and-again"));
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn resolve_binary_dir_falls_back_to_project_root_release() {
        // No env override → today's behaviour: `<project_root>/target/release/`.
        // Verifies the pre-#514 contract isn't lost for users who haven't
        // adopted shared-bin (e.g. ad-hoc `cargo build --release` followed
        // by a direct `cargo run --package xtask -- manual-test`).
        let _env = EnvGuard::acquire();
        let root = PathBuf::from("/projects/example-daft");
        assert_eq!(
            resolve_binary_dir(&root),
            PathBuf::from("/projects/example-daft/target/release"),
        );
    }

    #[test]
    fn resolve_binary_dir_honors_daft_binary_dir_env_var() {
        // Override semantics: `DAFT_BINARY_DIR=<path>` wins over the
        // project_root-derived default. Lets `mise run test:manual` point
        // the runner at a content-hashed shared cache instead of the
        // per-worktree `target/release/`.
        let env = EnvGuard::acquire();
        let override_dir = Path::new("/var/cache/daft-shared-bin/abc123/target/release");
        env.set(override_dir);
        assert_eq!(
            resolve_binary_dir(&PathBuf::from("/projects/example-daft")),
            override_dir.to_path_buf(),
        );
    }

    #[test]
    fn resolve_binary_dir_treats_empty_env_as_unset() {
        // An empty `DAFT_BINARY_DIR=` (common from `unset`-emulating
        // shell idioms) should fall through to the project-root default
        // rather than resolving to the workspace root + an empty join.
        let env = EnvGuard::acquire();
        env.set(Path::new(""));
        let root = PathBuf::from("/projects/example-daft");
        assert_eq!(
            resolve_binary_dir(&root),
            PathBuf::from("/projects/example-daft/target/release"),
        );
    }

    #[test]
    fn new_for_sandbox_honors_daft_binary_dir_env_var() {
        // End-to-end: the env override propagates through the constructor
        // and lands in both the sandbox-registered `$BINARY_DIR` and the
        // subprocess `PATH` that scenario steps see.
        let env = EnvGuard::acquire();
        let override_dir = tempfile::tempdir().expect("create override tempdir");
        env.set(override_dir.path());

        let (mut sandbox, _tmp) = sandbox_with_tempdir();
        let exec = DaftCommandExecutor::new_for_sandbox(&mut sandbox, &project_root()).unwrap();

        let expanded = sandbox.expand_vars("$BINARY_DIR");
        assert_eq!(expanded, override_dir.path().to_string_lossy());

        let path = exec.build_env(&sandbox).remove("PATH").unwrap();
        assert!(
            path.starts_with(override_dir.path().to_string_lossy().as_ref()),
            "PATH should begin with the override dir: {path}",
        );
    }
}
