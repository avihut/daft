//! Cached snapshot of the user's shell aliases and functions.
//!
//! `daft exec` resolves user shortcuts like `gss`, `gcvm`, or
//! `mygitfunc()` by routing commands through `$SHELL`. The naive way —
//! `$SHELL -i -c CMD` — sources the user's full rc file every invocation,
//! which on heavy setups (zsh with oh-my-zsh, gitstatus, plugins) costs
//! multiple seconds.
//!
//! This module captures the alias table and shell-function definitions
//! once via `$SHELL -i -c '<dump>'`, persists them under
//! `$XDG_CACHE_HOME/daft/`, and reuses them on subsequent runs. With a
//! populated cache, command execution skips the rc-file load entirely
//! (`$SHELL -c` instead of `-i -c`) and inlines the alias definitions
//! plus a `source` of the functions file before running the user command
//! via `eval`.
//!
//! Two files per shell:
//!   * `aliases-<shell>.txt` — small; alias definitions plus a metadata
//!     header (epoch, shell name).
//!   * `functions-<shell>.sh` — eval-able function bodies. Sourced by the
//!     spawned shell rather than inlined, since `typeset -f` output can
//!     be hundreds of kilobytes (zsh + plugins) and would blow `ARG_MAX`
//!     if passed on the command line.
//!
//! Limitations: only `bash` and `zsh` are supported; per-worktree direnv
//! aliases are out of scope (the cache reflects the user's base shell
//! environment). For unsupported shells or capture failures, callers
//! fall back to `-i` (slow path) and shortcuts still resolve.
//!
//! Cache invalidation is by TTL. `daft exec --refresh-aliases` forces a
//! re-capture.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a captured alias snapshot stays fresh on disk.
pub const CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24);

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
    /// Returns `None` for unsupported shells or if capture fails — in
    /// that case the caller should fall back to `-i`.
    pub fn ensure(shell_path: &str, force_refresh: bool) -> Option<Self> {
        let kind = ShellKind::from_path(shell_path)?;
        let aliases_path = aliases_cache_path(kind)?;
        let functions_path = functions_cache_path(kind)?;

        if !force_refresh {
            if let Some(loaded) = Self::load(&aliases_path, &functions_path) {
                if loaded.is_fresh(CACHE_TTL) {
                    return Some(loaded);
                }
            }
        }

        let (alias_lines, functions_body) = Self::capture(shell_path, kind).ok()?;
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
    /// Returns `(alias_lines, functions_body)`. Stderr is discarded so
    /// rc-file noise doesn't leak into the cache.
    fn capture(shell_path: &str, kind: ShellKind) -> std::io::Result<(String, String)> {
        // Single `$SHELL -i` invocation runs `<alias-cmd>; echo …; <fn-cmd>`
        // and we split the output on the sentinel. One rc-file load instead
        // of two.
        const SENTINEL: &str = "::DAFT_ALIAS_CACHE_SENTINEL::";
        let combined = format!(
            "{}; echo '{}'; {}",
            kind.alias_print_command(),
            SENTINEL,
            kind.functions_print_command(),
        );
        let output = std::process::Command::new(shell_path)
            .arg("-i")
            .arg("-c")
            .arg(combined)
            .stderr(std::process::Stdio::null())
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let (alias_part, functions_part) = stdout.split_once(SENTINEL).unwrap_or((&stdout, ""));
        Ok((
            alias_part.trim_end().to_string(),
            functions_part.trim_start_matches('\n').to_string(),
        ))
    }
}

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
        format!("{}\n{}\n{}", epoch, shell.name(), alias_lines),
    )
}

fn save_functions(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, body)
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
        let payload = "1700000000\nfish\nabbr gss git status";
        assert!(AliasCache::parse_aliases(payload).is_none());
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
        assert!(ShellKind::Bash
            .alias_expansion_prefix()
            .contains("expand_aliases"));
        assert_eq!(ShellKind::Zsh.alias_expansion_prefix(), "");
    }
}
