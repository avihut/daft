//! Background update check for new daft versions.
//!
//! Implements a fire-and-forget update notification system:
//! 1. On every invocation, reads a cache file (~/.config/daft/update-check.json)
//! 2. If a newer version is cached, returns a notification to display after command output
//! 3. If the cache is stale (>24h) or missing, spawns a detached background process to check
//! 4. The background process fetches GitHub Releases API via `curl` and writes the cache
//!
//! Notification throttling: the "new version available" banner is shown at most once
//! per 24 hours for the same version. If a different newer version appears, the banner
//! is shown again immediately. State is tracked in a separate file
//! (~/.config/daft/update-notification.json) to avoid race conditions with the
//! background update process.
//!
//! Zero latency impact on commands — the check never blocks.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::settings::keys;
use crate::styles;

/// Environment variable to disable update checks.
pub const NO_UPDATE_CHECK_ENV: &str = "DAFT_NO_UPDATE_CHECK";

/// GitHub API URL for the latest release.
const GITHUB_RELEASES_URL: &str = "https://api.github.com/repos/avihut/daft/releases/latest";

/// How long (in seconds) before the cache is considered stale.
const CACHE_TTL_SECONDS: i64 = 24 * 60 * 60; // 24 hours

/// How long (in seconds) before the notification for the same version is shown again.
const NOTIFICATION_TTL_SECONDS: i64 = 24 * 60 * 60; // 24 hours

/// Current cache schema version.
const CACHE_VERSION: u32 = 1;

/// Current notification state schema version.
const NOTIFICATION_STATE_VERSION: u32 = 1;

/// Cached result from a previous update check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckCache {
    /// Schema version for future migrations.
    pub version: u32,
    /// Unix timestamp of when the check was performed.
    pub checked_at: i64,
    /// The latest version string (without 'v' prefix).
    pub latest_version: String,
}

/// Tracks when/what version was last shown to the user, so we can throttle
/// the "new version available" notification to once per 24 hours per version.
/// Stored separately from `UpdateCheckCache` because the cache is written by
/// a background process while this state is written by the foreground.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationState {
    /// Schema version for future migrations.
    pub version: u32,
    /// The version string that was last shown to the user.
    pub notified_version: String,
    /// Unix timestamp of when the notification was last shown.
    pub notified_at: i64,
}

/// Minimal GitHub release response — only the fields we need.
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
}

/// Information needed to display an update notification.
#[derive(Debug, Clone)]
pub struct UpdateNotification {
    pub current_version: String,
    pub latest_version: String,
    pub update_command: String,
}

/// Detected installation method, used to suggest the right update command.
#[derive(Debug, Clone, PartialEq)]
enum InstallMethod {
    Homebrew,
    CargoInstall,
    GitHubRelease,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Check if an update notification should be shown. Spawns a background check
/// if the cache is stale. Never panics — wrapped in `catch_unwind`.
///
/// Returns `None` if:
/// - Update checks are disabled (env var, git config, CI)
/// - No cached version or the cached version is not newer
/// - Any error occurs (silently swallowed)
pub fn maybe_check_for_update() -> Option<UpdateNotification> {
    std::panic::catch_unwind(maybe_check_for_update_inner).unwrap_or(None)
}

/// Entry point for the `daft __check-update` background process.
/// Fetches the latest version from GitHub and writes the cache file.
pub fn run_check_update() -> Result<()> {
    let latest = fetch_latest_version()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock error")?
        .as_secs() as i64;

    let cache = UpdateCheckCache {
        version: CACHE_VERSION,
        checked_at: now,
        latest_version: latest,
    };

    let path = cache_path()?;
    save_cache_to(&cache, &path)
}

/// Print the update notification to stderr.
pub fn print_notification(notification: &UpdateNotification) {
    let arrow = styles::dim("\u{2192}");
    let current = styles::dim(&notification.current_version);
    let latest = styles::green(&notification.latest_version);

    eprintln!();
    eprintln!("hint: A new version of daft is available: {current} {arrow} {latest}");
    eprintln!(
        "hint: To update: {}",
        styles::cyan(&notification.update_command)
    );
    eprintln!(
        "hint: To disable: {}",
        styles::dim("git config --global daft.updateCheck false")
    );
}

// ---------------------------------------------------------------------------
// Internal implementation
// ---------------------------------------------------------------------------

fn maybe_check_for_update_inner() -> Option<UpdateNotification> {
    // Don't run inside the background check process itself
    if env::args().any(|a| a == "__check-update") {
        return None;
    }

    if is_update_check_disabled() {
        return None;
    }

    let path = cache_path().ok()?;
    let cache = load_cache_from(&path);

    // Spawn a background check if cache is stale or missing
    match &cache {
        Some(c) if !is_cache_stale(c) => {}
        _ => {
            let _ = spawn_background_check();
        }
    }

    // Check if cached version is newer
    let cache = cache?;
    let current = crate::VERSION;

    if is_newer_version(current, &cache.latest_version) {
        // Throttle: show at most once per 24h per version
        if should_suppress_notification(&cache.latest_version) {
            return None;
        }

        let method = detect_install_method();
        Some(UpdateNotification {
            current_version: current.to_string(),
            latest_version: cache.latest_version,
            update_command: update_command_for(&method),
        })
    } else {
        None
    }
}

/// Returns the path to the update check cache file.
fn cache_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("Could not determine config directory")?;
    Ok(config_dir.join("daft").join("update-check.json"))
}

/// Load the cache from disk. Returns `None` on any error.
fn load_cache_from(path: &PathBuf) -> Option<UpdateCheckCache> {
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Save the cache to disk, creating parent directories as needed.
fn save_cache_to(cache: &UpdateCheckCache, path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }

    let contents =
        serde_json::to_string_pretty(cache).context("Failed to serialize update check cache")?;

    fs::write(path, contents)
        .with_context(|| format!("Failed to write cache to {}", path.display()))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Notification state persistence and throttling
// ---------------------------------------------------------------------------

/// Returns the path to the notification state file.
fn notification_state_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("Could not determine config directory")?;
    Ok(config_dir.join("daft").join("update-notification.json"))
}

/// Load the notification state from disk. Returns `None` on any error.
fn load_notification_state() -> Option<NotificationState> {
    let path = notification_state_path().ok()?;
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Save the notification state to disk, creating parent directories as needed.
fn save_notification_state(state: &NotificationState) -> Result<()> {
    let path = notification_state_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }

    let contents =
        serde_json::to_string_pretty(state).context("Failed to serialize notification state")?;

    fs::write(&path, contents)
        .with_context(|| format!("Failed to write notification state to {}", path.display()))?;

    Ok(())
}

/// Pure logic: returns `true` if the notification should be suppressed.
///
/// Suppresses when:
/// - Same version was notified less than 24 hours ago
/// - Future timestamp (clock skew) for the same version — treat as recently notified
///
/// Does NOT suppress when:
/// - No prior state (first time)
/// - Different version (new release detected)
/// - Same version but 24+ hours have passed
fn should_suppress_for_state(state: Option<&NotificationState>, latest_version: &str) -> bool {
    let state = match state {
        Some(s) => s,
        None => return false, // No prior state → show
    };

    if state.notified_version != latest_version {
        return false; // Different version → show immediately
    }

    // Same version — check time elapsed
    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return false, // Clock error → show to be safe
    };

    let age = now - state.notified_at;

    // Suppress if within TTL window or future timestamp (clock skew)
    age < NOTIFICATION_TTL_SECONDS
}

/// Check if the notification for `latest_version` should be suppressed.
fn should_suppress_notification(latest_version: &str) -> bool {
    let state = load_notification_state();
    should_suppress_for_state(state.as_ref(), latest_version)
}

/// Record that a notification was shown for the given version.
/// Silently ignores any errors (best-effort).
pub fn record_notification_shown(version: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let state = NotificationState {
        version: NOTIFICATION_STATE_VERSION,
        notified_version: version.to_string(),
        notified_at: now,
    };

    let _ = save_notification_state(&state);
}

/// Returns `true` if the cache is older than 24 hours or has a future timestamp.
fn is_cache_stale(cache: &UpdateCheckCache) -> bool {
    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return true,
    };

    let age = now - cache.checked_at;

    // Future timestamp (clock skew) or older than TTL
    !(0..=CACHE_TTL_SECONDS).contains(&age)
}

/// Compare two semver version strings. Returns `true` if `latest` is newer than `current`.
/// Strips leading 'v' prefix. Ignores pre-release suffixes (treats "1.0.0-beta.1" as "1.0.0").
/// Returns `false` on any parse error.
fn is_newer_version(current: &str, latest: &str) -> bool {
    let parse = |s: &str| -> Option<(u64, u64, u64)> {
        let s = s.strip_prefix('v').unwrap_or(s);
        // Strip pre-release suffix (everything after first '-')
        let s = s.split('-').next()?;
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        ))
    };

    match (parse(current), parse(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// Detect how daft was installed by examining the executable path.
fn detect_install_method() -> InstallMethod {
    let exe = match env::current_exe() {
        Ok(p) => p,
        Err(_) => return InstallMethod::GitHubRelease,
    };

    let path_str = exe.to_string_lossy();

    // Homebrew: /opt/homebrew/*, /usr/local/Cellar/*, /home/linuxbrew/*
    if path_str.contains("/homebrew/")
        || path_str.contains("/Cellar/")
        || path_str.contains("/linuxbrew/")
    {
        return InstallMethod::Homebrew;
    }

    // Cargo: ~/.cargo/bin/*
    if path_str.contains("/.cargo/bin/") || path_str.contains("\\.cargo\\bin\\") {
        return InstallMethod::CargoInstall;
    }

    InstallMethod::GitHubRelease
}

/// Return the update command string for the given install method.
fn update_command_for(method: &InstallMethod) -> String {
    match method {
        InstallMethod::Homebrew => "brew upgrade daft".to_string(),
        InstallMethod::CargoInstall => "cargo install daft".to_string(),
        InstallMethod::GitHubRelease => {
            "https://github.com/avihut/daft/releases/latest".to_string()
        }
    }
}

/// Spawn a detached background process to check for updates.
fn spawn_background_check() -> Result<()> {
    let exe = env::current_exe().context("Could not determine current executable")?;

    Command::new(exe)
        .arg("__check-update")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to spawn background update check")?;

    Ok(())
}

/// Fetch the latest version tag from GitHub Releases API using `curl`.
fn fetch_latest_version() -> Result<String> {
    let output = Command::new("curl")
        .args([
            "-sL",
            "--max-time",
            "5",
            "-H",
            "Accept: application/vnd.github+json",
            GITHUB_RELEASES_URL,
        ])
        .output()
        .context("Failed to run curl")?;

    if !output.status.success() {
        anyhow::bail!("curl exited with status {}", output.status);
    }

    let body =
        String::from_utf8(output.stdout).context("GitHub API response is not valid UTF-8")?;

    let release: GitHubRelease =
        serde_json::from_str(&body).context("Failed to parse GitHub API response")?;

    // Strip leading 'v' prefix from tag_name
    let version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name)
        .to_string();

    Ok(version)
}

/// Check if update checks are disabled via env var, git config, or CI environment.
fn is_update_check_disabled() -> bool {
    // Explicit env var opt-out
    if env::var(NO_UPDATE_CHECK_ENV).is_ok() {
        return true;
    }

    // CI environment detection
    if is_ci_environment() {
        return true;
    }

    // Git config opt-out (global only — we may not be in a repo)
    if let Ok(output) = Command::new("git")
        .args(["config", "--global", "--get", keys::UPDATE_CHECK])
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

    // -- Version comparison tests --

    #[test]
    fn test_newer_version_basic() {
        assert!(is_newer_version("1.0.0", "1.0.1"));
        assert!(is_newer_version("1.0.0", "1.1.0"));
        assert!(is_newer_version("1.0.0", "2.0.0"));
    }

    #[test]
    fn test_same_version() {
        assert!(!is_newer_version("1.0.0", "1.0.0"));
        assert!(!is_newer_version("1.0.18", "1.0.18"));
    }

    #[test]
    fn test_older_version() {
        assert!(!is_newer_version("1.0.1", "1.0.0"));
        assert!(!is_newer_version("2.0.0", "1.0.0"));
    }

    #[test]
    fn test_v_prefix() {
        assert!(is_newer_version("1.0.0", "v1.0.1"));
        assert!(is_newer_version("v1.0.0", "1.0.1"));
        assert!(is_newer_version("v1.0.0", "v1.0.1"));
    }

    #[test]
    fn test_pre_release_ignored() {
        // Pre-release suffixes are stripped, so "1.0.1-beta.1" is treated as "1.0.1"
        assert!(is_newer_version("1.0.0", "1.0.1-beta.1"));
        // "1.0.0-beta.1" is treated as "1.0.0" — same version, not newer
        assert!(!is_newer_version("1.0.0", "1.0.0-beta.1"));
    }

    #[test]
    fn test_invalid_version_strings() {
        assert!(!is_newer_version("invalid", "1.0.0"));
        assert!(!is_newer_version("1.0.0", "invalid"));
        assert!(!is_newer_version("", "1.0.0"));
        assert!(!is_newer_version("1.0.0", ""));
        assert!(!is_newer_version("1.0", "1.0.1"));
        assert!(!is_newer_version("1.0.0", "1.0"));
    }

    #[test]
    fn test_major_minor_patch_bumps() {
        assert!(is_newer_version("1.0.0", "1.0.1")); // patch
        assert!(is_newer_version("1.0.0", "1.1.0")); // minor
        assert!(is_newer_version("1.0.0", "2.0.0")); // major
        assert!(is_newer_version("0.9.9", "1.0.0")); // major crossing
        assert!(is_newer_version("1.9.9", "1.10.0")); // minor with higher digits
    }

    // -- Cache persistence tests --

    #[test]
    fn test_cache_save_load_roundtrip() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("update-check.json");

        let cache = UpdateCheckCache {
            version: CACHE_VERSION,
            checked_at: 1700000000,
            latest_version: "1.0.18".to_string(),
        };

        save_cache_to(&cache, &path).unwrap();
        let loaded = load_cache_from(&path).unwrap();
        assert_eq!(loaded.version, CACHE_VERSION);
        assert_eq!(loaded.checked_at, 1700000000);
        assert_eq!(loaded.latest_version, "1.0.18");
    }

    #[test]
    fn test_load_missing_file() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("nonexistent.json");
        assert!(load_cache_from(&path).is_none());
    }

    #[test]
    fn test_load_corrupt_file() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("corrupt.json");
        fs::write(&path, "not json at all {{{").unwrap();
        assert!(load_cache_from(&path).is_none());
    }

    // -- Cache staleness tests --

    #[test]
    fn test_fresh_cache() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let cache = UpdateCheckCache {
            version: CACHE_VERSION,
            checked_at: now - 60, // 1 minute ago
            latest_version: "1.0.0".to_string(),
        };

        assert!(!is_cache_stale(&cache));
    }

    #[test]
    fn test_stale_cache() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let cache = UpdateCheckCache {
            version: CACHE_VERSION,
            checked_at: now - CACHE_TTL_SECONDS - 1, // just past TTL
            latest_version: "1.0.0".to_string(),
        };

        assert!(is_cache_stale(&cache));
    }

    #[test]
    fn test_future_timestamp_is_stale() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let cache = UpdateCheckCache {
            version: CACHE_VERSION,
            checked_at: now + 3600, // 1 hour in the future (clock skew)
            latest_version: "1.0.0".to_string(),
        };

        assert!(is_cache_stale(&cache));
    }

    // -- Install method tests --

    #[test]
    fn test_update_command_strings() {
        assert_eq!(
            update_command_for(&InstallMethod::Homebrew),
            "brew upgrade daft"
        );
        assert_eq!(
            update_command_for(&InstallMethod::CargoInstall),
            "cargo install daft"
        );
        assert_eq!(
            update_command_for(&InstallMethod::GitHubRelease),
            "https://github.com/avihut/daft/releases/latest"
        );
    }

    // -- CI detection test --

    #[test]
    fn test_ci_detection_does_not_panic() {
        // Just ensure it doesn't panic regardless of environment
        let _ = is_ci_environment();
    }

    // -- Notification state persistence tests --

    #[test]
    fn test_notification_state_save_load_roundtrip() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("update-notification.json");

        let state = NotificationState {
            version: NOTIFICATION_STATE_VERSION,
            notified_version: "1.0.22".to_string(),
            notified_at: 1700000000,
        };

        let contents = serde_json::to_string_pretty(&state).unwrap();
        fs::write(&path, &contents).unwrap();

        let loaded_contents = fs::read_to_string(&path).unwrap();
        let loaded: NotificationState = serde_json::from_str(&loaded_contents).unwrap();
        assert_eq!(loaded.version, NOTIFICATION_STATE_VERSION);
        assert_eq!(loaded.notified_version, "1.0.22");
        assert_eq!(loaded.notified_at, 1700000000);
    }

    // -- Notification throttling tests --

    #[test]
    fn test_no_prior_state_does_not_suppress() {
        assert!(!should_suppress_for_state(None, "1.0.22"));
    }

    #[test]
    fn test_different_version_does_not_suppress() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let state = NotificationState {
            version: NOTIFICATION_STATE_VERSION,
            notified_version: "1.0.21".to_string(),
            notified_at: now - 60, // 1 minute ago
        };

        // Different version → should NOT suppress, even if recent
        assert!(!should_suppress_for_state(Some(&state), "1.0.22"));
    }

    #[test]
    fn test_same_version_recent_suppresses() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let state = NotificationState {
            version: NOTIFICATION_STATE_VERSION,
            notified_version: "1.0.22".to_string(),
            notified_at: now - 60, // 1 minute ago
        };

        // Same version, shown recently → should suppress
        assert!(should_suppress_for_state(Some(&state), "1.0.22"));
    }

    #[test]
    fn test_same_version_expired_does_not_suppress() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let state = NotificationState {
            version: NOTIFICATION_STATE_VERSION,
            notified_version: "1.0.22".to_string(),
            notified_at: now - NOTIFICATION_TTL_SECONDS - 1, // just past TTL
        };

        // Same version, but enough time has passed → should NOT suppress
        assert!(!should_suppress_for_state(Some(&state), "1.0.22"));
    }

    #[test]
    fn test_future_timestamp_suppresses() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let state = NotificationState {
            version: NOTIFICATION_STATE_VERSION,
            notified_version: "1.0.22".to_string(),
            notified_at: now + 3600, // 1 hour in the future (clock skew)
        };

        // Future timestamp for same version → treat as recently notified (suppress)
        assert!(should_suppress_for_state(Some(&state), "1.0.22"));
    }

    // -- maybe_check_for_update smoke test --

    #[test]
    fn test_maybe_check_for_update_does_not_panic() {
        // The public function is wrapped in catch_unwind and should never panic
        let _ = maybe_check_for_update();
    }
}
