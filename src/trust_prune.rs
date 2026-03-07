//! Background trust database pruning.
//!
//! Removes stale entries (paths that no longer exist on disk) from the trust
//! database. Runs automatically in the background on every daft invocation,
//! throttled to once per 24 hours, using the same fire-and-forget detached
//! process pattern as the update check.
//!
//! Zero latency impact on commands — the prune never blocks.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::hooks::TrustDatabase;
use crate::settings::keys;

/// Environment variable to disable automatic trust pruning.
pub const NO_TRUST_PRUNE_ENV: &str = "DAFT_NO_TRUST_PRUNE";

/// How long (in seconds) before the cache is considered stale.
const CACHE_TTL_SECONDS: i64 = 24 * 60 * 60; // 24 hours

/// Current cache schema version.
const CACHE_VERSION: u32 = 1;

/// Cached timestamp of the last prune run.
#[derive(Debug, Serialize, Deserialize)]
pub struct TrustPruneCache {
    /// Schema version for future migrations.
    pub version: u32,
    /// Unix timestamp of when the prune was last performed.
    pub pruned_at: i64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Trigger a background trust prune if the cache is stale.
/// Never panics — wrapped in `catch_unwind`.
pub fn maybe_prune_trust() {
    let _ = std::panic::catch_unwind(maybe_prune_trust_inner);
}

/// Entry point for the `daft __prune-trust` background process.
/// Loads the trust database, removes stale entries, and writes the cache.
pub fn run_prune_trust() -> Result<()> {
    let mut db = TrustDatabase::load().context("Failed to load trust database")?;

    let removed = db.prune();
    let backfilled = db.backfill_fingerprints();

    if !removed.is_empty() || backfilled > 0 {
        db.save().context("Failed to save trust database")?;
    }

    // Always update the cache timestamp, even if nothing was pruned
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock error")?
        .as_secs() as i64;

    let cache = TrustPruneCache {
        version: CACHE_VERSION,
        pruned_at: now,
    };

    save_cache(&cache)
}

// ---------------------------------------------------------------------------
// Internal implementation
// ---------------------------------------------------------------------------

fn maybe_prune_trust_inner() {
    // Don't run inside the background prune process itself
    if env::args().any(|a| a == "__prune-trust") {
        return;
    }

    if is_trust_prune_disabled() {
        return;
    }

    let path = match cache_path() {
        Ok(p) => p,
        Err(_) => return,
    };

    let cache = load_cache(&path);

    match &cache {
        Some(c) if !is_cache_stale(c) => {}
        _ => {
            let _ = spawn_background_prune();
        }
    }
}

/// Returns the path to the prune cache file.
fn cache_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("Could not determine config directory")?;
    Ok(config_dir.join("daft").join("trust-prune.json"))
}

/// Load the cache from disk. Returns `None` on any error.
fn load_cache(path: &PathBuf) -> Option<TrustPruneCache> {
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Save the cache to disk, creating parent directories as needed.
fn save_cache(cache: &TrustPruneCache) -> Result<()> {
    let path = cache_path()?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }

    let contents =
        serde_json::to_string_pretty(cache).context("Failed to serialize trust prune cache")?;

    fs::write(&path, contents)
        .with_context(|| format!("Failed to write cache to {}", path.display()))?;

    Ok(())
}

/// Returns `true` if the cache is older than 24 hours or has a future timestamp.
fn is_cache_stale(cache: &TrustPruneCache) -> bool {
    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return true,
    };

    let age = now - cache.pruned_at;

    // Future timestamp (clock skew) or older than TTL
    !(0..=CACHE_TTL_SECONDS).contains(&age)
}

/// Spawn a detached background process to prune stale trust entries.
fn spawn_background_prune() -> Result<()> {
    let exe = env::current_exe().context("Could not determine current executable")?;

    Command::new(exe)
        .arg("__prune-trust")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to spawn background trust prune")?;

    Ok(())
}

/// Check if automatic trust pruning is disabled.
fn is_trust_prune_disabled() -> bool {
    // Explicit env var opt-out
    if env::var(NO_TRUST_PRUNE_ENV).is_ok() {
        return true;
    }

    // CI environment detection
    if is_ci_environment() {
        return true;
    }

    // Git config opt-out (global only — we may not be in a repo)
    if let Ok(output) = Command::new("git")
        .args(["config", "--global", "--get", keys::hooks::TRUST_PRUNE])
        .output()
    {
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_lowercase();
            if matches!(value.as_str(), "false" | "no" | "off" | "0") {
                return true;
            }
        }
    }

    false
}

/// Returns `true` if we appear to be running in a CI environment.
fn is_ci_environment() -> bool {
    let ci_vars = [
        "CI",
        "GITHUB_ACTIONS",
        "JENKINS_URL",
        "TRAVIS",
        "CIRCLECI",
        "GITLAB_CI",
        "BUILDKITE",
        "TF_BUILD",
    ];

    ci_vars.iter().any(|var| env::var(var).is_ok())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_cache_save_load_roundtrip() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("trust-prune.json");

        let cache = TrustPruneCache {
            version: CACHE_VERSION,
            pruned_at: 1700000000,
        };

        let contents = serde_json::to_string_pretty(&cache).unwrap();
        fs::write(&path, &contents).unwrap();

        let loaded = load_cache(&path).unwrap();
        assert_eq!(loaded.version, CACHE_VERSION);
        assert_eq!(loaded.pruned_at, 1700000000);
    }

    #[test]
    fn test_load_missing_file() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("nonexistent.json");
        assert!(load_cache(&path).is_none());
    }

    #[test]
    fn test_load_corrupt_file() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("corrupt.json");
        fs::write(&path, "not json {{{").unwrap();
        assert!(load_cache(&path).is_none());
    }

    #[test]
    fn test_fresh_cache() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let cache = TrustPruneCache {
            version: CACHE_VERSION,
            pruned_at: now - 60, // 1 minute ago
        };

        assert!(!is_cache_stale(&cache));
    }

    #[test]
    fn test_stale_cache() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let cache = TrustPruneCache {
            version: CACHE_VERSION,
            pruned_at: now - CACHE_TTL_SECONDS - 1,
        };

        assert!(is_cache_stale(&cache));
    }

    #[test]
    fn test_future_timestamp_is_stale() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let cache = TrustPruneCache {
            version: CACHE_VERSION,
            pruned_at: now + 3600, // 1 hour in the future
        };

        assert!(is_cache_stale(&cache));
    }

    #[test]
    fn test_maybe_prune_trust_does_not_panic() {
        // The public function is wrapped in catch_unwind and should never panic
        maybe_prune_trust();
    }
}
