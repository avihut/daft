//! Cached snapshot of the user's shell aliases and functions.
//!
//! `daft exec` resolves user shortcuts like `gss`, `gcvm`, or
//! `mygitfunc()` by routing commands through `$SHELL`. The naive way —
//! `$SHELL -i -c CMD` — sources the user's full rc file every invocation,
//! which on heavy setups (zsh with oh-my-zsh, gitstatus, plugins) costs
//! multiple seconds. Worse, rc files frequently misbehave in
//! non-interactive contexts (p10k instant prompt, plugins that need a
//! TTY, `exec tmux` patterns) — when they do, `-i -c` fails and renders
//! `daft exec` itself useless.
//!
//! This module captures the alias table and shell-function definitions
//! once via `$SHELL -i`, persists them under `$XDG_CACHE_HOME/daft/`,
//! and reuses them on subsequent runs. With a populated cache, command
//! execution uses a pristine `$SHELL -c` and inlines the alias
//! definitions plus a `source` of the functions file before running the
//! user command via `eval` — no rc-file load at runtime.
//!
//! Capture writes alias and function output to dedicated temp files
//! whose paths are passed to the spawned shell via env vars
//! (`__DAFT_ALIAS_OUT`, `__DAFT_FN_OUT`). The shell's stdout and
//! stderr are discarded entirely, so rc-file noise (welcome banners,
//! p10k instant-prompt output, fortune cookies) cannot corrupt the
//! captured snapshot. A 10s deadline guards against rc-files that hang.
//!
//! Two files per shell:
//!   * `aliases-<shell>.txt` — small; alias definitions plus a metadata
//!     header (format version, epoch, shell name).
//!   * `functions-<shell>.sh` — eval-able function bodies. Sourced by the
//!     spawned shell rather than inlined, since `typeset -f` output can
//!     be hundreds of kilobytes (zsh + plugins) and would blow `ARG_MAX`
//!     if passed on the command line.
//!
//! Limitations: only `bash` and `zsh` are supported; per-worktree direnv
//! aliases are out of scope (the cache reflects the user's base shell
//! environment). When capture fails (unsupported shell, deadline, or
//! validation rejection) callers fall back to a rc-less `$SHELL -c CMD`
//! — aliases won't resolve in that path, but commands still run. The
//! `-i -c` runtime path is no longer used as a fallback because rc-file
//! breakage in non-interactive mode is precisely what the user's
//! commands need to survive.
//!
//! Cache invalidation is by TTL plus a format-version header: bumping
//! `CACHE_FORMAT_HEADER` auto-rejects older on-disk caches without
//! requiring `--refresh-aliases`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a captured alias snapshot stays fresh on disk.
pub const CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24);

/// Magic header on every saved alias snapshot. Bumping the integer
/// auto-invalidates older caches without requiring `--refresh-aliases`
/// — important when a previous daft version persisted poisoned
/// captures (rc-file stdout mixed into the alias body).
///
/// v1 was implicit (no header); v2 adds this line and FD-isolated
/// capture.
pub const CACHE_FORMAT_HEADER: &str = "# daft-alias-cache v2";

/// One of the shells whose alias output `daft` knows how to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Bash,
    Zsh,
}

impl ShellKind {
    /// Detect the shell from a `$SHELL`-style absolute path.
    pub fn from_path(shell_path: &str) -> Option<Self> {
        let basename = Path::new(shell_path).file_name()?.to_str()?;
        match basename {
            "bash" => Some(Self::Bash),
            "zsh" => Some(Self::Zsh),
            _ => None,
        }
    }

    /// The builtin invocation that prints eval-able alias definitions
    /// (one per line, in `alias name='body'` form for both shells).
    fn alias_print_command(self) -> &'static str {
        match self {
            Self::Bash => "alias -p",
            // zsh's `-p` doesn't exist; `-L` emits eval-able output with
            // a leading `alias ` keyword to match bash's format.
            Self::Zsh => "alias -L",
        }
    }

    /// The builtin invocation that prints eval-able shell-function
    /// definitions (one block per function).
    fn functions_print_command(self) -> &'static str {
        match self {
            // Both bash's `declare -f` and zsh's `typeset -f` emit
            // shell-eval-able function bodies. `typeset -f` is a synonym
            // accepted by zsh; bash also accepts it but defaults to
            // `declare -f` semantics, so we keep them shell-specific for
            // clarity.
            Self::Bash => "declare -f",
            Self::Zsh => "typeset -f",
        }
    }

    /// Shell-specific prefix that must precede the inlined alias
    /// definitions to make alias expansion work in non-interactive mode.
    /// bash needs `shopt -s expand_aliases`; zsh expands aliases by
    /// default.
    pub fn alias_expansion_prefix(self) -> &'static str {
        match self {
            Self::Bash => "shopt -s expand_aliases\n",
            Self::Zsh => "",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
        }
    }
}

/// On-disk snapshot of the user's alias table and shell functions for
/// one shell.
#[derive(Debug, Clone)]
pub struct AliasCache {
    pub shell: ShellKind,
    /// Eval-able alias definitions joined by newlines; safe to inline
    /// verbatim into a `$SHELL -c` script before the user command.
    pub alias_lines: String,
    /// On-disk path to the functions snapshot, sourced by the spawned
    /// shell. `None` when functions weren't captured (capture failed or
    /// in-memory caches built by tests).
    pub functions_path: Option<PathBuf>,
    pub captured_at: SystemTime,
}

/// Daft's per-user cache directory.
fn cache_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("daft"))
}

/// Path to the alias snapshot file for `shell`.
fn aliases_cache_path(shell: ShellKind) -> Option<PathBuf> {
    Some(cache_dir()?.join(format!("aliases-{}.txt", shell.name())))
}

/// Path to the functions snapshot file for `shell`.
fn functions_cache_path(shell: ShellKind) -> Option<PathBuf> {
    Some(cache_dir()?.join(format!("functions-{}.sh", shell.name())))
}

impl AliasCache {
    /// Return a usable cache for `shell_path`, capturing afresh if the
    /// on-disk copy is missing, stale, or `force_refresh` is set.
    /// Returns `None` for unsupported shells, capture failures (timeout
    /// or spawn error), or captures that fail post-validation — in any
    /// of those cases the caller should fall back to a rc-less
    /// `$SHELL -c CMD` so the command still runs (without alias
    /// resolution).
    pub fn ensure(shell_path: &str, force_refresh: bool) -> Option<Self> {
        let kind = match ShellKind::from_path(shell_path) {
            Some(k) => k,
            None => {
                debug_log_capture(&format!(
                    "shell {shell_path:?} is not bash/zsh — skipping alias capture"
                ));
                return None;
            }
        };
        let aliases_path = aliases_cache_path(kind)?;
        let functions_path = functions_cache_path(kind)?;

        if !force_refresh
            && let Some(loaded) = Self::load(&aliases_path, &functions_path)
            && loaded.is_fresh(CACHE_TTL)
        {
            debug_log_capture(&format!(
                "using cached snapshot at {}",
                aliases_path.display()
            ));
            return Some(loaded);
        }

        let (alias_lines, functions_body) = match Self::capture(shell_path, kind) {
            Ok(out) => out,
            Err(e) => {
                debug_log_capture(&format!("capture failed: {e}"));
                return None;
            }
        };
        if !looks_like_alias_dump(&alias_lines) {
            // Capture ran but produced something that doesn't match the
            // expected `alias <name>=…` shape. Likely the rc-file aborted
            // early or the shell isn't actually bash/zsh-compatible.
            // Better to fall back to the rc-less path than to persist a
            // garbage cache.
            debug_log_capture(&format!(
                "capture produced unrecognized output (first non-blank line not `alias …`); \
                 first 80 chars: {:?}",
                alias_lines.chars().take(80).collect::<String>()
            ));
            return None;
        }
        let captured_at = SystemTime::now();

        // Best-effort save: a write failure (e.g. read-only HOME)
        // shouldn't prevent the captured cache from being used in this
        // process. `functions_path` is recorded only if its file write
        // succeeded — the spawned shell would error on a missing file
        // otherwise.
        let alias_saved = save_aliases(&aliases_path, &alias_lines, kind, captured_at).is_ok();
        let functions_saved = save_functions(&functions_path, &functions_body).is_ok();

        Some(Self {
            shell: kind,
            alias_lines,
            functions_path: (alias_saved && functions_saved).then_some(functions_path),
            captured_at,
        })
    }

    fn is_fresh(&self, ttl: Duration) -> bool {
        SystemTime::now()
            .duration_since(self.captured_at)
            .map(|age| age < ttl)
            .unwrap_or(false)
    }

    fn load(aliases_path: &Path, functions_path: &Path) -> Option<Self> {
        let content = std::fs::read_to_string(aliases_path).ok()?;
        let mut parsed = Self::parse_aliases(&content)?;
        // Functions file is optional — its absence just means the fast
        // path skips function sourcing. The unit-test-friendly parser
        // doesn't know about disk paths, so we attach the path here.
        if std::fs::metadata(functions_path).is_ok() {
            parsed.functions_path = Some(functions_path.to_path_buf());
        }
        Some(parsed)
    }

    fn parse_aliases(content: &str) -> Option<Self> {
        let mut lines = content.lines();
        // Reject anything that isn't the current format version. v1
        // caches have no header — they fail this check naturally.
        if lines.next()? != CACHE_FORMAT_HEADER {
            return None;
        }
        let epoch: u64 = lines.next()?.parse().ok()?;
        let shell = match lines.next()? {
            "bash" => ShellKind::Bash,
            "zsh" => ShellKind::Zsh,
            _ => return None,
        };
        let alias_lines = lines.collect::<Vec<_>>().join("\n");
        Some(Self {
            shell,
            alias_lines,
            functions_path: None,
            captured_at: UNIX_EPOCH + Duration::from_secs(epoch),
        })
    }

    /// Run a one-shot capture of aliases and functions via `$SHELL -i`.
    /// Returns `(alias_lines, functions_body)`.
    ///
    /// The spawned shell writes its `alias`/`typeset` output directly to
    /// two temp files whose paths are passed via env vars
    /// (`__DAFT_ALIAS_OUT`, `__DAFT_FN_OUT`). Stdout and stderr are
    /// discarded entirely. This isolates the captured snapshot from any
    /// rc-file pollution — `echo` calls, p10k instant prompt output,
    /// plugin status banners, fortune cookies — that would otherwise be
    /// indistinguishable from real `alias` output and end up in the
    /// cached file.
    ///
    /// A 10-second timeout guards against rc-files that hang the shell
    /// (e.g. `exec tmux` patterns waiting on a controlling terminal).
    fn capture(shell_path: &str, kind: ShellKind) -> std::io::Result<(String, String)> {
        Self::capture_with_timeout(shell_path, kind, CAPTURE_TIMEOUT)
    }

    fn capture_with_timeout(
        shell_path: &str,
        kind: ShellKind,
        timeout: Duration,
    ) -> std::io::Result<(String, String)> {
        let dir = tempfile::tempdir()?;
        let alias_out = dir.path().join("aliases.out");
        let fn_out = dir.path().join("functions.out");

        // The shell uses `>"$VAR"` rather than the static path so that a
        // rc-file with `set -u` doesn't blow up on the redirection
        // itself. `2>/dev/null` on the redirections suppresses errors
        // from shells that don't have `typeset -f` (defensive — both
        // bash and zsh do).
        let body = format!(
            r#"{} >"$__DAFT_ALIAS_OUT" 2>/dev/null
{} >"$__DAFT_FN_OUT" 2>/dev/null
"#,
            kind.alias_print_command(),
            kind.functions_print_command(),
        );

        let mut cmd = std::process::Command::new(shell_path);
        cmd.arg("-i")
            .arg("-c")
            .arg(body)
            .env("__DAFT_ALIAS_OUT", &alias_out)
            .env("__DAFT_FN_OUT", &fn_out)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // Own process group (#663): an interactive shell (or its rc
        // machinery) that draws a job-control stop freezes its own
        // group *alone*. Without this, the stop lands on the invoking
        // process's group — including this very watchdog loop, which
        // then can never fire. The safety net must not freeze together
        // with the thing it guards.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn()?;

        // Bounded wait. A rc-file that does `exec tmux` or similar would
        // otherwise block forever; the deadline lets the caller fall
        // back gracefully.
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match child.try_wait()? {
                Some(_status) => break,
                None => {
                    if std::time::Instant::now() >= deadline {
                        // Group kill: reaches rc-spawned grandchildren
                        // too, and SIGKILL is the one terminating signal
                        // a stopped group still acts on.
                        #[cfg(unix)]
                        {
                            let _ = nix::sys::signal::killpg(
                                nix::unistd::Pid::from_raw(child.id() as i32),
                                nix::sys::signal::Signal::SIGKILL,
                            );
                        }
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!(
                                "shell capture exceeded {}s — rc-file likely hangs in non-interactive mode",
                                timeout.as_secs()
                            ),
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }

        // The files may not exist if the rc-file aborted before the body
        // ran (e.g. `set -e` plus a failing line). Treat absence as
        // empty rather than erroring so the validator below can decide.
        let alias_lines = std::fs::read_to_string(&alias_out).unwrap_or_default();
        let functions_body = std::fs::read_to_string(&fn_out).unwrap_or_default();

        Ok((
            alias_lines.trim_end().to_string(),
            functions_body.trim_start_matches('\n').to_string(),
        ))
    }
}

/// Upper bound on how long the shell capture is allowed to run. rc-files
/// that hang (`exec tmux`, blocking reads, etc.) get killed at this
/// deadline so the caller falls back to the rc-less path.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

fn save_aliases(
    path: &Path,
    alias_lines: &str,
    shell: ShellKind,
    captured_at: SystemTime,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let epoch = captured_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    std::fs::write(
        path,
        format!(
            "{}\n{}\n{}\n{}",
            CACHE_FORMAT_HEADER,
            epoch,
            shell.name(),
            alias_lines
        ),
    )
}

fn save_functions(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, body)
}

/// Stderr trace of capture-pipeline state when `DAFT_EXEC_DEBUG=1` is
/// set. Helps users diagnose why aliases aren't resolving — without
/// this, a capture failure is invisible (exec just falls back to the
/// rc-less path, which works but doesn't expand shortcuts).
fn debug_log_capture(msg: &str) {
    if std::env::var_os("DAFT_EXEC_DEBUG").is_none() {
        return;
    }
    eprintln!("[daft-exec-debug] capture: {msg}");
}

/// Quick sanity check on captured alias output. An empty body is fine
/// (some users just have no aliases). Otherwise the first non-blank
/// line must look like `alias name=…`. This catches the case where a
/// rc-file aborts before `alias -L` runs but the wrapper still
/// produces *some* output (e.g. a stray comment), which would otherwise
/// be persisted as a "valid but empty" snapshot.
pub(crate) fn looks_like_alias_dump(body: &str) -> bool {
    let mut non_blank = body.lines().filter(|l| !l.trim().is_empty());
    match non_blank.next() {
        None => true, // empty is legitimate
        Some(line) => line.trim_start().starts_with("alias "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_bash_and_zsh_from_path() {
        assert_eq!(ShellKind::from_path("/bin/bash"), Some(ShellKind::Bash));
        assert_eq!(
            ShellKind::from_path("/usr/local/bin/zsh"),
            Some(ShellKind::Zsh)
        );
        assert_eq!(ShellKind::from_path("/bin/sh"), None);
        assert_eq!(ShellKind::from_path("/usr/bin/fish"), None);
    }

    #[test]
    fn parses_serialized_aliases_round_trip() {
        let captured_at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let dir = tempfile::tempdir().unwrap();
        let aliases_path = dir.path().join("aliases-zsh.txt");
        save_aliases(
            &aliases_path,
            "alias gss='git status -s'\nalias gcvm='git checkout master'",
            ShellKind::Zsh,
            captured_at,
        )
        .unwrap();

        let parsed = AliasCache::parse_aliases(&std::fs::read_to_string(&aliases_path).unwrap())
            .expect("parses");
        assert_eq!(parsed.shell, ShellKind::Zsh);
        assert_eq!(
            parsed.alias_lines,
            "alias gss='git status -s'\nalias gcvm='git checkout master'"
        );
        assert_eq!(parsed.captured_at, captured_at);
        assert!(parsed.functions_path.is_none());
    }

    #[test]
    fn rejects_unknown_shell_in_serialized_cache() {
        let payload = format!("{CACHE_FORMAT_HEADER}\n1700000000\nfish\nabbr gss git status");
        assert!(AliasCache::parse_aliases(&payload).is_none());
    }

    #[test]
    fn rejects_v1_cache_without_format_header() {
        // A pre-v2 cache (no header) — shipped before FD-isolated capture
        // landed and likely contains poisoned content. parse must reject
        // so the next run forces a fresh capture instead of replaying
        // the stale data.
        let v1_payload = "1700000000\nzsh\nalias gss='git status -s'";
        assert!(
            AliasCache::parse_aliases(v1_payload).is_none(),
            "v1 caches must be rejected to force re-capture"
        );
    }

    #[test]
    fn looks_like_alias_dump_accepts_empty_and_real_aliases() {
        assert!(looks_like_alias_dump(""));
        assert!(looks_like_alias_dump("\n\n  \n"));
        assert!(looks_like_alias_dump("alias g='git'"));
        assert!(looks_like_alias_dump(
            "alias g='git'\nalias gs='git status'"
        ));
    }

    #[test]
    fn looks_like_alias_dump_rejects_non_alias_first_line() {
        // Captured pollution shouldn't survive validation. (FD-isolated
        // capture means this is a defense-in-depth check, not the
        // primary defense.)
        assert!(!looks_like_alias_dump(
            "Welcome to my shell!\nalias g='git'"
        ));
        assert!(!looks_like_alias_dump("# random comment\nalias g='git'"));
    }

    #[test]
    fn is_fresh_respects_ttl() {
        let recent = AliasCache {
            shell: ShellKind::Bash,
            alias_lines: String::new(),
            functions_path: None,
            captured_at: SystemTime::now() - Duration::from_secs(60),
        };
        assert!(recent.is_fresh(Duration::from_secs(3600)));

        let stale = AliasCache {
            shell: ShellKind::Bash,
            alias_lines: String::new(),
            functions_path: None,
            captured_at: SystemTime::now() - Duration::from_secs(7200),
        };
        assert!(!stale.is_fresh(Duration::from_secs(3600)));
    }

    #[test]
    fn save_then_load_attaches_functions_path_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let aliases_path = dir.path().join("nested").join("aliases-zsh.txt");
        let functions_path = dir.path().join("nested").join("functions-zsh.sh");
        let captured_at = UNIX_EPOCH + Duration::from_secs(1_700_000_001);

        save_aliases(&aliases_path, "alias g='git'", ShellKind::Zsh, captured_at).unwrap();
        save_functions(&functions_path, "myfn () { echo hi; }\n").unwrap();

        let loaded = AliasCache::load(&aliases_path, &functions_path).unwrap();
        assert_eq!(loaded.shell, ShellKind::Zsh);
        assert_eq!(loaded.alias_lines, "alias g='git'");
        assert_eq!(loaded.captured_at, captured_at);
        assert_eq!(
            loaded.functions_path.as_deref(),
            Some(functions_path.as_path())
        );
    }

    #[test]
    fn load_omits_functions_path_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let aliases_path = dir.path().join("aliases-zsh.txt");
        let functions_path = dir.path().join("functions-zsh.sh"); // never written
        save_aliases(
            &aliases_path,
            "alias g='git'",
            ShellKind::Zsh,
            SystemTime::now(),
        )
        .unwrap();

        let loaded = AliasCache::load(&aliases_path, &functions_path).unwrap();
        assert!(loaded.functions_path.is_none());
    }

    #[test]
    fn alias_expansion_prefix_is_shell_specific() {
        assert!(
            ShellKind::Bash
                .alias_expansion_prefix()
                .contains("expand_aliases")
        );
        assert_eq!(ShellKind::Zsh.alias_expansion_prefix(), "");
    }

    /// Regression: rc-files that print to stdout (p10k instant prompt,
    /// "welcome" banners, plugin status messages, fortune cookies) must
    /// not corrupt the captured alias snapshot. Pre-fix, capture split
    /// `alias -p` output from the shell's stdout via a sentinel — but
    /// any rc-file pollution was indistinguishable from alias output and
    /// got persisted, then re-injected into every subsequent fast-path
    /// invocation, breaking the user's commands.
    #[cfg(unix)]
    #[test]
    fn capture_survives_rc_file_stdout_pollution() {
        use std::os::unix::fs::PermissionsExt;
        if std::process::Command::new("bash")
            .arg("-c")
            .arg("true")
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
        {
            // bash unavailable — skip rather than fail on minimal CI images.
            return;
        }

        let dir = tempfile::tempdir().unwrap();

        // A rc-file that emits a "welcome banner" before defining the
        // alias, plus more pollution after. Mimics real-world setups
        // (Powerlevel10k instant prompt, oh-my-zsh plugin status, nvm
        // version banners, etc.).
        std::fs::write(
            dir.path().join(".bashrc"),
            "echo 'POLLUTION_BEFORE_PROMPT_INIT'\n\
             printf 'fortune cookie quote\\n'\n\
             alias daft_pollution_marker='echo pmark'\n\
             echo 'POLLUTION_AFTER'\n",
        )
        .unwrap();

        // Wrapper executable that scopes HOME to the temp dir, so bash
        // -i sources our fixture .bashrc instead of the real user's.
        let wrapper = dir.path().join("bash-wrapper.sh");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexport HOME={}\nexec bash \"$@\"\n",
                dir.path().to_str().unwrap()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

        let (alias_lines, _functions) =
            AliasCache::capture(wrapper.to_str().unwrap(), ShellKind::Bash)
                .expect("capture should spawn the wrapper shell");

        assert!(
            alias_lines.contains("daft_pollution_marker"),
            "real alias must be captured: {alias_lines:?}"
        );
        assert!(
            !alias_lines.contains("POLLUTION_BEFORE_PROMPT_INIT"),
            "rc-file stdout must NOT leak into the cache: {alias_lines:?}"
        );
        assert!(
            !alias_lines.contains("POLLUTION_AFTER"),
            "rc-file stdout must NOT leak into the cache: {alias_lines:?}"
        );
        assert!(
            !alias_lines.contains("fortune cookie quote"),
            "rc-file stdout must NOT leak into the cache: {alias_lines:?}"
        );
    }

    /// Regression test for #663: a capture shell that job-control-stops
    /// its own process group must wedge *alone*. Before
    /// `process_group(0)`, the `kill -STOP 0` here landed on the
    /// invoking process's group — this very test suite — freezing the
    /// watchdog together with the shell it guards (the field incident's
    /// exact shape, minus the tty). With the isolation, the watchdog
    /// stays live, group-kills the child at the deadline, and capture
    /// returns TimedOut so `daft exec` falls back to the rc-less path.
    #[cfg(unix)]
    #[test]
    fn capture_survives_self_stopping_shell() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("stopping-shell.sh");
        std::fs::write(&wrapper, "#!/bin/sh\nkill -STOP 0\n").unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

        let started = std::time::Instant::now();
        let err = AliasCache::capture_with_timeout(
            wrapper.to_str().unwrap(),
            ShellKind::Bash,
            Duration::from_secs(1),
        )
        .expect_err("a stopped capture shell must time out, not wedge");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "watchdog fired late: {:?}",
            started.elapsed()
        );
    }
}
