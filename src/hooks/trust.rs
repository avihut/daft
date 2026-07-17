//! Trust management for hook execution.
//!
//! This module provides the trust database that tracks which repositories
//! are trusted to run hooks. Trust is stored in the user's config directory,
//! not in the repository itself, to prevent malicious repositories from
//! self-trusting.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use super::trust_dto::{TrustDatabaseV1_0_0, TrustDatabaseV2_0_0};
use crate::output::deferred_warn;

/// Trust level for a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    /// Never run hooks (default for unknown repositories).
    #[default]
    Deny,
    /// Ask before each hook execution.
    Prompt,
    /// Run hooks without prompting.
    Allow,
}

impl TrustLevel {
    /// Parse a trust level from a string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "deny" => Some(TrustLevel::Deny),
            "prompt" => Some(TrustLevel::Prompt),
            "allow" => Some(TrustLevel::Allow),
            _ => None,
        }
    }

    /// Returns whether hooks should be executed for this trust level
    /// (without considering prompting).
    pub fn allows_execution(&self) -> bool {
        matches!(self, TrustLevel::Prompt | TrustLevel::Allow)
    }

    /// Returns whether hooks can be executed without prompting.
    pub fn allows_without_prompt(&self) -> bool {
        matches!(self, TrustLevel::Allow)
    }
}

impl fmt::Display for TrustLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrustLevel::Deny => write!(f, "deny"),
            TrustLevel::Prompt => write!(f, "prompt"),
            TrustLevel::Allow => write!(f, "allow"),
        }
    }
}

/// Trust entry for a single repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntry {
    /// Trust level for this repository.
    pub level: TrustLevel,
    /// When trust was granted (Unix epoch seconds).
    #[serde(default)]
    pub granted_at: i64,
    /// How trust was granted.
    #[serde(default = "default_granted_by")]
    pub granted_by: String,
    /// Repository fingerprint for identity verification.
    /// Stores the remote URL at the time trust was granted.
    /// `None` means the entry was created before fingerprinting was added.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub fingerprint: Option<String>,
}

fn default_granted_by() -> String {
    "user".to_string()
}

impl TrustEntry {
    /// Create a new trust entry with the current timestamp.
    pub fn new(level: TrustLevel) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            level,
            granted_at: epoch,
            granted_by: "user".to_string(),
            fingerprint: None,
        }
    }

    /// Create a new trust entry with a fingerprint (remote URL).
    pub fn with_fingerprint(level: TrustLevel, fingerprint: String) -> Self {
        let mut entry = Self::new(level);
        entry.fingerprint = Some(fingerprint);
        entry
    }

    /// Format the granted_at timestamp for display.
    pub fn formatted_time(&self) -> String {
        use chrono::{Local, TimeZone};
        Local
            .timestamp_opt(self.granted_at, 0)
            .single()
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

/// Pattern-based trust rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustPattern {
    /// Glob pattern to match repository paths.
    pub pattern: String,
    /// Trust level for matching repositories.
    pub level: TrustLevel,
    /// Optional comment explaining this pattern.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Trust database stored in user config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustDatabase {
    /// Database schema version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Default trust level for unknown repositories.
    #[serde(default)]
    pub default_level: TrustLevel,
    /// Per-repository trust entries.
    #[serde(default)]
    pub repositories: HashMap<String, TrustEntry>,
    /// Per-repository layout overrides (V3+). Skipped from direct serde
    /// because we use custom V3 serialization via `save_to()`.
    #[serde(skip)]
    pub(crate) layouts: HashMap<String, String>,
    /// Pattern-based trust rules.
    #[serde(default)]
    pub patterns: Vec<TrustPattern>,
}

fn default_version() -> u32 {
    3
}

impl Default for TrustDatabase {
    fn default() -> Self {
        Self {
            version: 3,
            default_level: TrustLevel::Deny,
            repositories: HashMap::new(),
            layouts: HashMap::new(),
            patterns: Vec::new(),
        }
    }
}

impl TrustDatabase {
    /// Load the trust database from the default location.
    ///
    /// Returns `Err` when the registry exists but is unreadable/corrupt, so
    /// diagnostic callers (`daft doctor`, `daft hooks status`) surface the
    /// problem instead of silently reporting "untrusted". Callers on the
    /// hook-execution hot path use `.unwrap_or_default()` to fail **closed**
    /// (empty ⇒ Deny) — the resulting skipped-hook notice already tells the user
    /// hooks aren't running, and the next write (`update`) backs the corrupt
    /// file up and heals it loudly. Deliberately NOT warning here: `load()` runs
    /// on every hooked git operation, so a warning would spam the terminal on
    /// every push/checkout while the file is broken (#666 review).
    pub fn load() -> Result<Self> {
        let path = Self::default_path()?;
        Self::load_from(&path)
    }

    /// Load the trust database from a specific path.
    ///
    /// This method handles automatic migration from older schema versions:
    /// - V1: Original schema with string timestamps (ISO 8601)
    /// - V2: Schema with epoch timestamps (i64)
    /// - V3: Unified repo store with trust + layout per entry
    ///
    /// The migration from V1 to V2 converts string timestamps to Unix epoch.
    /// The migration from V2 to V3 wraps trust entries and adds layout support.
    ///
    /// Note: Due to a historical bug, some V2 databases may have version=1.
    /// We detect this by checking the granted_at field type.
    pub fn load_from(path: &Path) -> Result<Self> {
        use super::trust_dto::RepoStoreV3_0_0;
        use version_migrate::{IntoDomain, MigratesTo};

        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read trust database from {}", path.display()))?;

        // Parse as generic JSON to detect version and schema
        let json: serde_json::Value = serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse JSON from {}", path.display()))?;

        // Determine version - default to 1 for legacy data without version field
        let stated_version = json.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

        match stated_version {
            3 => {
                let v3: RepoStoreV3_0_0 = serde_json::from_value(json).with_context(|| {
                    format!("Failed to parse V3 repo store from {}", path.display())
                })?;
                Ok(v3.into_domain())
            }
            _ => {
                // Detect actual schema by checking granted_at field type
                // V1 has string timestamps, V2 has integer timestamps
                let actual_version = detect_schema_version(&json, stated_version);

                let db = match actual_version {
                    1 => {
                        let v1: TrustDatabaseV1_0_0 =
                            serde_json::from_value(json).with_context(|| {
                                format!("Failed to parse V1 trust database from {}", path.display())
                            })?;
                        let v2: TrustDatabaseV2_0_0 = v1.migrate();
                        let v3: RepoStoreV3_0_0 = v2.migrate();
                        v3.into_domain()
                    }
                    _ => {
                        let v2: TrustDatabaseV2_0_0 =
                            serde_json::from_value(json).with_context(|| {
                                format!("Failed to parse V2 trust database from {}", path.display())
                            })?;
                        let v3: RepoStoreV3_0_0 = v2.migrate();
                        v3.into_domain()
                    }
                };

                // In-memory migration only — no disk write here. Persisting the
                // migrated V3 form (and retiring a legacy trust.json) happens on
                // the next `update()`, under the registry lock. Keeping reads
                // side-effect-free stops a pure reader from racing writers by
                // rewriting the file mid-read (#666). The old in-place migration
                // also used a fixed `repos.json.tmp` name that two concurrent
                // migrators would collide on — that is gone with it.
                Ok(db)
            }
        }
    }

    /// Save the trust database to a specific path in V3 format.
    ///
    /// The write is atomic: contents go to a same-directory temp file which is
    /// then renamed over `path`. This is lock-free — serialization across
    /// processes is `update()`'s job; `save_to` only guarantees a reader never
    /// sees a torn file.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        use super::trust_dto::{RepoEntryV3_0_0, TrustEntryV2_0_0};
        use std::io::Write;

        // Ensure parent directory exists
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }

        let mut entries: HashMap<String, RepoEntryV3_0_0> = HashMap::new();

        for (path_key, trust_entry) in &self.repositories {
            entries
                .entry(path_key.clone())
                .or_insert_with(|| RepoEntryV3_0_0 {
                    trust: None,
                    layout: None,
                })
                .trust = Some(TrustEntryV2_0_0 {
                level: trust_entry.level,
                granted_at: trust_entry.granted_at,
                granted_by: trust_entry.granted_by.clone(),
                fingerprint: trust_entry.fingerprint.clone(),
            });
        }

        for (path_key, layout) in &self.layouts {
            entries
                .entry(path_key.clone())
                .or_insert_with(|| RepoEntryV3_0_0 {
                    trust: None,
                    layout: None,
                })
                .layout = Some(layout.clone());
        }

        let json = serde_json::json!({
            "version": 3,
            "repositories": serde_json::to_value(&entries)?,
            "patterns": serde_json::to_value(&self.patterns)?,
        });

        let contents =
            serde_json::to_string_pretty(&json).context("Failed to serialize trust database")?;

        // Atomic replace: a same-directory temp file with a random name, then
        // rename over the destination. Same-dir keeps the rename atomic (no
        // cross-filesystem copy); the random name means concurrent writers never
        // collide on a fixed temp path (#666).
        let tmp_dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(tmp_dir)
            .with_context(|| format!("Failed to create temp file in {}", tmp_dir.display()))?;
        tmp.write_all(contents.as_bytes())
            .with_context(|| format!("Failed to write trust database to {}", path.display()))?;
        // Flush data to disk before the rename so a crash can't leave the
        // renamed-into-place file pointing at unwritten (zero/garbage) blocks.
        tmp.as_file()
            .sync_all()
            .with_context(|| format!("Failed to flush trust database to {}", path.display()))?;
        tmp.persist(path)
            .map_err(|e| e.error)
            .with_context(|| format!("Failed to write trust database to {}", path.display()))?;

        Ok(())
    }

    /// Atomically apply a mutation to the on-disk registry under an exclusive
    /// lock, serializing concurrent daft processes.
    ///
    /// This is the ONLY safe way to write the default registry. It holds an
    /// advisory `flock` across the whole read-modify-write, and reloads a
    /// *fresh* copy inside the lock, so two processes can't lost-update each
    /// other (#666). Because the closure sees state loaded inside the lock, any
    /// read-decide-write logic (e.g. "trust only if not already trusted") must
    /// live inside `f` to be race-free. The registry is always rewritten; use
    /// [`update_if`](Self::update_if) to skip the write on a no-op.
    pub fn update<T>(f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let dir = crate::daft_config_dir()?;
        Self::update_in(&dir, f)
    }

    /// Like [`update`](Self::update) but the closure returns whether it changed
    /// anything; the registry is only rewritten when it returns `true`. This
    /// keeps no-op operations (pruning nothing, removing an absent entry) from
    /// touching — or creating — `repos.json`.
    pub fn update_if(f: impl FnOnce(&mut Self) -> Result<bool>) -> Result<()> {
        let dir = crate::daft_config_dir()?;
        // Fast path: a conditional update against a registry that doesn't exist
        // yet has nothing to change. Skip it entirely so a no-op (pruning
        // nothing, removing an absent entry) never creates the config dir or a
        // lock file — keeping `worktree-remove` from littering a fresh config
        // dir, and unsandboxed tests from writing the real one.
        if !dir.join("repos.json").exists() && !dir.join("trust.json").exists() {
            return Ok(());
        }
        Self::with_lock(&dir, |db| {
            let changed = f(db)?;
            Ok((changed, ()))
        })
    }

    /// `update()` against an explicit config directory. Test seam — production
    /// code calls [`update`](Self::update).
    pub(crate) fn update_in<T>(dir: &Path, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        Self::with_lock(dir, |db| Ok((true, f(db)?)))
    }

    /// Locked read-modify-write core. Acquires an exclusive advisory lock on a
    /// sidecar file, reloads a fresh copy inside the lock, runs `f`, and — when
    /// `f` reports a change — atomically persists the result and retires any
    /// legacy `trust.json`.
    fn with_lock<T>(dir: &Path, f: impl FnOnce(&mut Self) -> Result<(bool, T)>) -> Result<T> {
        use fs2::FileExt;

        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create directory {}", dir.display()))?;

        // Lock a SIDECAR file, never repos.json itself: `flock` binds to the
        // open file description (the inode). If we locked repos.json and then
        // renamed a temp file over it, the lock would follow the orphaned inode
        // while a concurrent writer opened the fresh inode lock-free (#666).
        let lock_path = dir.join("repos.json.lock");
        let lock_file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open registry lock {}", lock_path.display()))?;
        lock_file
            .lock_exclusive()
            .with_context(|| format!("Failed to lock registry {}", lock_path.display()))?;

        // Critical section. `lock_file` stays in scope until the function
        // returns, so the lock is held for the whole closure and released only
        // after the result has been produced.
        (|| -> Result<T> {
            let repos = dir.join("repos.json");
            let legacy = dir.join("trust.json");
            let src = if repos.exists() {
                repos.clone()
            } else if legacy.exists() {
                legacy.clone()
            } else {
                repos.clone()
            };

            // Load the current registry. If it's corrupt, DON'T touch the file
            // yet — defer to the `changed` branch below. Backing a corrupt file
            // up on a no-op (background prune, worktree-remove of an unknown
            // repo) would rename the live registry aside and write nothing,
            // leaving it absent — the #666 incident all over again.
            let (mut db, corrupt) = match Self::load_from(&src) {
                Ok(db) => (db, None),
                Err(e) => (Self::default(), Some(e)),
            };

            let (changed, out) = f(&mut db)?;
            if changed {
                if let Some(e) = corrupt {
                    // We're about to REPLACE a corrupt file — preserve it first
                    // so a recoverable-but-corrupt registry is never lost, and
                    // warn. This is the foreground heal path, so the warning is
                    // seen (unlike the background pruner, which no-ops above).
                    // Deferred rather than `eprintln!`ed: no live region reaches
                    // a trust mutation today, but that is a property of the
                    // current call graph, not something this side of the lock
                    // can check (#720).
                    let backup = back_up_corrupt(&src)?;
                    deferred_warn::warn(format!(
                        "warning: daft trust registry at {} was unreadable ({e:#}); \
                         backed it up to {} and started a fresh registry. \
                         Re-run `daft hooks trust` to restore grants.",
                        src.display(),
                        backup.display()
                    ));
                }
                db.save_to(&repos)?;
                // Retire a legacy trust.json now that V3 repos.json is written.
                if src == legacy && legacy.exists() {
                    let _ = fs::remove_file(&legacy);
                }
            }
            Ok(out)
        })()
    }

    /// Get the default path for the trust database.
    ///
    /// Prefers `repos.json` if it exists, falls back to `trust.json` for
    /// backwards compatibility, and defaults to `repos.json` for new installs.
    pub fn default_path() -> Result<PathBuf> {
        let config_dir = crate::daft_config_dir()?;
        let repos_path = config_dir.join("repos.json");
        if repos_path.exists() {
            return Ok(repos_path);
        }
        let trust_path = config_dir.join("trust.json");
        if trust_path.exists() {
            return Ok(trust_path);
        }
        Ok(repos_path)
    }

    /// Get the trust level for a repository.
    ///
    /// Checks in order:
    /// 1. Exact repository match (canonicalized)
    /// 2. Pattern matches
    /// 3. Default level
    pub fn get_trust_level(&self, git_dir: &Path) -> TrustLevel {
        let canonical = git_dir
            .canonicalize()
            .unwrap_or_else(|_| git_dir.to_path_buf());
        let git_dir_str = canonical.to_string_lossy();

        // Check exact match
        if let Some(entry) = self.repositories.get(git_dir_str.as_ref()) {
            return entry.level;
        }

        // Check patterns
        for pattern in &self.patterns {
            if matches_glob(&pattern.pattern, &git_dir_str) {
                return pattern.level;
            }
        }

        // Return default
        self.default_level
    }

    /// Get the full trust entry for a repository (if an explicit entry exists).
    ///
    /// Unlike `get_trust_level`, this does not fall through to patterns or the
    /// default level. Returns `None` if no explicit entry exists.
    pub fn get_trust_entry(&self, git_dir: &Path) -> Option<&TrustEntry> {
        let canonical = git_dir
            .canonicalize()
            .unwrap_or_else(|_| git_dir.to_path_buf());
        let git_dir_str = canonical.to_string_lossy();
        self.repositories.get(git_dir_str.as_ref())
    }

    /// Set the trust level for a repository.
    ///
    /// The path is canonicalized before storage to ensure consistent lookups
    /// (callers may pass relative or non-canonical paths).
    pub fn set_trust_level(&mut self, git_dir: &Path, level: TrustLevel) {
        let canonical = git_dir
            .canonicalize()
            .unwrap_or_else(|_| git_dir.to_path_buf());
        let git_dir_str = canonical.to_string_lossy().to_string();
        self.repositories
            .insert(git_dir_str, TrustEntry::new(level));
    }

    /// Set the trust level for a repository with a fingerprint (remote URL).
    pub fn set_trust_level_with_fingerprint(
        &mut self,
        git_dir: &Path,
        level: TrustLevel,
        fingerprint: String,
    ) {
        let canonical = git_dir
            .canonicalize()
            .unwrap_or_else(|_| git_dir.to_path_buf());
        let git_dir_str = canonical.to_string_lossy().to_string();
        self.repositories.insert(
            git_dir_str,
            TrustEntry::with_fingerprint(level, fingerprint),
        );
    }

    /// Remove trust for a repository.
    pub fn remove_trust(&mut self, git_dir: &Path) -> bool {
        let canonical = git_dir
            .canonicalize()
            .unwrap_or_else(|_| git_dir.to_path_buf());
        let git_dir_str = canonical.to_string_lossy();
        self.repositories.remove(git_dir_str.as_ref()).is_some()
    }

    /// Get the layout override for a repository.
    pub fn get_layout(&self, git_dir: &Path) -> Option<&str> {
        let canonical = git_dir
            .canonicalize()
            .unwrap_or_else(|_| git_dir.to_path_buf());
        self.layouts
            .get(&*canonical.to_string_lossy())
            .map(|s| s.as_str())
    }

    /// Set the layout override for a repository.
    pub fn set_layout(&mut self, git_dir: &Path, layout: String) {
        let canonical = git_dir
            .canonicalize()
            .unwrap_or_else(|_| git_dir.to_path_buf());
        self.layouts
            .insert(canonical.to_string_lossy().to_string(), layout);
    }

    /// Remove the layout override for a repository.
    pub fn remove_layout(&mut self, git_dir: &Path) -> bool {
        let canonical = git_dir
            .canonicalize()
            .unwrap_or_else(|_| git_dir.to_path_buf());
        let git_dir_str = canonical.to_string_lossy();
        self.layouts.remove(git_dir_str.as_ref()).is_some()
    }

    /// Reset all per-repo settings to defaults (trust, layout, and any future
    /// fields). Use when a repo is re-cloned and stale config should not carry
    /// over. Returns `true` if any entry was actually removed.
    pub fn reset_repo(&mut self, git_dir: &Path) -> bool {
        // Bind both results before the `||` so neither call short-circuits.
        let removed_trust = self.remove_trust(git_dir);
        let removed_layout = self.remove_layout(git_dir);
        removed_trust || removed_layout
    }

    /// Add a pattern-based trust rule.
    pub fn add_pattern(&mut self, pattern: String, level: TrustLevel, comment: Option<String>) {
        self.patterns.push(TrustPattern {
            pattern,
            level,
            comment,
        });
    }

    /// Remove a pattern-based trust rule.
    pub fn remove_pattern(&mut self, pattern: &str) -> bool {
        let initial_len = self.patterns.len();
        self.patterns.retain(|p| p.pattern != pattern);
        self.patterns.len() < initial_len
    }

    /// Clear all trust entries, layouts, and patterns.
    pub fn clear(&mut self) {
        self.repositories.clear();
        self.layouts.clear();
        self.patterns.clear();
    }

    /// List all trusted repositories.
    pub fn list_trusted(&self) -> Vec<(&str, &TrustEntry)> {
        self.repositories
            .iter()
            .filter(|(_, entry)| entry.level != TrustLevel::Deny)
            .map(|(path, entry)| (path.as_str(), entry))
            .collect()
    }

    /// Check if a repository has explicit trust configured.
    pub fn has_explicit_trust(&self, git_dir: &Path) -> bool {
        let canonical = git_dir
            .canonicalize()
            .unwrap_or_else(|_| git_dir.to_path_buf());
        let git_dir_str = canonical.to_string_lossy();
        self.repositories.contains_key(git_dir_str.as_ref())
    }

    /// Remove entries whose paths no longer exist on disk.
    ///
    /// Returns the list of paths that were removed (from both repositories and
    /// layouts). Persisted by the enclosing `update`/`update_if`.
    pub fn prune(&mut self) -> Vec<String> {
        let stale: Vec<String> = self
            .repositories
            .keys()
            .chain(self.layouts.keys())
            .filter(|path| !Path::new(path.as_str()).exists())
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        for key in &stale {
            self.repositories.remove(key);
            self.layouts.remove(key);
        }

        stale
    }

    /// Collect `(path, remote_url)` for entries that exist on disk but have no
    /// fingerprint. This runs a `git` subprocess per repo, so call it **outside**
    /// the registry lock and feed the result to [`apply_fingerprints`].
    pub fn gather_missing_fingerprints(&self) -> Vec<(String, String)> {
        self.repositories
            .iter()
            .filter(|(_, entry)| entry.fingerprint.is_none())
            .filter_map(|(path, _)| {
                get_remote_url_for_git_dir(Path::new(path.as_str())).map(|url| (path.clone(), url))
            })
            .collect()
    }

    /// Apply fingerprints from [`gather_missing_fingerprints`] to entries that
    /// still exist and still lack one. Pure in-memory (no git, no IO), so it is
    /// safe under the registry lock. Returns the number applied.
    pub fn apply_fingerprints(&mut self, fingerprints: &[(String, String)]) -> usize {
        let mut count = 0;
        for (path, url) in fingerprints {
            if let Some(entry) = self.repositories.get_mut(path)
                && entry.fingerprint.is_none()
            {
                entry.fingerprint = Some(url.clone());
                count += 1;
            }
        }
        count
    }

    /// Backfill fingerprints in one shot (gather + apply). The pruner splits the
    /// two so the git subprocesses run lock-free; direct callers can use this.
    pub fn backfill_fingerprints(&mut self) -> usize {
        let gathered = self.gather_missing_fingerprints();
        self.apply_fingerprints(&gathered)
    }
}

/// Rename a corrupt registry file aside so it isn't lost when we start fresh.
/// Returns the backup path. Mirrors the manual recovery done in the #666
/// incident (`repos.json.corrupt-<ts>.bak`).
fn back_up_corrupt(path: &Path) -> Result<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repos.json");
    let backup = path.with_file_name(format!("{name}.corrupt-{ts}.bak"));
    fs::rename(path, &backup)
        .with_context(|| format!("Failed to back up corrupt registry {}", path.display()))?;
    Ok(backup)
}

/// Get the remote "origin" URL for a repository given its `.git` directory.
///
/// Returns `None` if the remote cannot be queried (no remote configured,
/// path doesn't exist, etc.).
pub fn get_remote_url_for_git_dir(git_dir: &Path) -> Option<String> {
    use std::process::Command;
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .env("GIT_DIR", git_dir)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Detect the actual schema version by examining the data structure.
///
/// Some databases may have version=1 but actually contain V2 data (with integer
/// timestamps) due to a historical bug where the version wasn't updated when
/// the schema changed.
///
/// Returns 1 if the data looks like V1 (string timestamps), 2 otherwise.
fn detect_schema_version(json: &serde_json::Value, stated_version: u32) -> u32 {
    // If stated version is already 2+, trust it
    if stated_version >= 2 {
        return stated_version;
    }

    // Check if repositories have any entries with granted_at
    if let Some(repos) = json.get("repositories").and_then(|v| v.as_object()) {
        for (_path, entry) in repos {
            if let Some(granted_at) = entry.get("granted_at") {
                // V1 has string timestamps, V2 has integer timestamps
                if granted_at.is_string() {
                    return 1;
                } else if granted_at.is_number() {
                    return 2;
                }
            }
        }
    }

    // No repositories with granted_at - could be either version
    // Default to V2 (current) since empty databases should use current schema
    2
}

/// Simple glob matching for trust patterns.
///
/// Supports:
/// - `*` matches any sequence of characters within a path component
/// - `**` matches any sequence of path components
fn matches_glob(pattern: &str, path: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();

    matches_glob_parts(&pattern_parts, &path_parts)
}

fn matches_glob_parts(pattern: &[&str], path: &[&str]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }

    let first_pattern = pattern[0];

    if first_pattern == "**" {
        // ** can match zero or more path components
        if pattern.len() == 1 {
            // ** at the end matches everything
            return true;
        }
        // Try matching ** with zero, one, two, etc. components
        for i in 0..=path.len() {
            if matches_glob_parts(&pattern[1..], &path[i..]) {
                return true;
            }
        }
        return false;
    }

    if path.is_empty() {
        return false;
    }

    if matches_component(first_pattern, path[0]) {
        return matches_glob_parts(&pattern[1..], &path[1..]);
    }

    false
}

fn matches_component(pattern: &str, component: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if !pattern.contains('*') {
        return pattern == component;
    }

    // Simple wildcard matching within a component
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 2 {
        let prefix = parts[0];
        let suffix = parts[1];
        return component.starts_with(prefix)
            && component.ends_with(suffix)
            && component.len() >= prefix.len() + suffix.len();
    }

    // For more complex patterns, fall back to exact match
    pattern == component
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_trust_level_parse() {
        assert_eq!(TrustLevel::parse("deny"), Some(TrustLevel::Deny));
        assert_eq!(TrustLevel::parse("DENY"), Some(TrustLevel::Deny));
        assert_eq!(TrustLevel::parse("prompt"), Some(TrustLevel::Prompt));
        assert_eq!(TrustLevel::parse("allow"), Some(TrustLevel::Allow));
        assert_eq!(TrustLevel::parse("invalid"), None);
    }

    #[test]
    fn test_trust_level_allows() {
        assert!(!TrustLevel::Deny.allows_execution());
        assert!(TrustLevel::Prompt.allows_execution());
        assert!(TrustLevel::Allow.allows_execution());

        assert!(!TrustLevel::Deny.allows_without_prompt());
        assert!(!TrustLevel::Prompt.allows_without_prompt());
        assert!(TrustLevel::Allow.allows_without_prompt());
    }

    #[test]
    fn test_trust_database_default() {
        let db = TrustDatabase::default();
        assert_eq!(db.version, 3);
        assert_eq!(db.default_level, TrustLevel::Deny);
        assert!(db.repositories.is_empty());
        assert!(db.layouts.is_empty());
        assert!(db.patterns.is_empty());
    }

    #[test]
    fn test_trust_database_set_and_get() {
        let mut db = TrustDatabase::default();
        let git_dir = Path::new("/path/to/repo/.git");

        // Default should be deny
        assert_eq!(db.get_trust_level(git_dir), TrustLevel::Deny);

        // Set to allow
        db.set_trust_level(git_dir, TrustLevel::Allow);
        assert_eq!(db.get_trust_level(git_dir), TrustLevel::Allow);

        // Remove trust
        assert!(db.remove_trust(git_dir));
        assert_eq!(db.get_trust_level(git_dir), TrustLevel::Deny);
    }

    #[test]
    fn reset_repo_returns_true_only_when_something_is_removed() {
        let mut db = TrustDatabase::default();
        let git_dir = Path::new("/path/to/missing/.git");

        // No entries → nothing to remove.
        assert!(!db.reset_repo(git_dir), "no entries → returns false");

        // A trust entry alone → reset reports a removal.
        db.set_trust_level(git_dir, TrustLevel::Allow);
        assert!(db.reset_repo(git_dir), "with a trust entry → returns true");
        assert!(!db.reset_repo(git_dir), "after reset → returns false again");

        // A layout entry alone → reset reports a removal.
        db.set_layout(git_dir, "sibling".to_string());
        assert!(db.reset_repo(git_dir), "with a layout entry → returns true");
        assert!(!db.reset_repo(git_dir), "after reset → returns false again");
    }

    #[test]
    fn test_trust_database_save_and_load() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("repos.json");

        let mut db = TrustDatabase::default();
        db.set_trust_level(Path::new("/project/.git"), TrustLevel::Allow);
        db.add_pattern(
            "/trusted/*/.git".to_string(),
            TrustLevel::Allow,
            Some("Trusted org".to_string()),
        );

        db.save_to(&path).unwrap();

        let loaded = TrustDatabase::load_from(&path).unwrap();
        assert_eq!(
            loaded.get_trust_level(Path::new("/project/.git")),
            TrustLevel::Allow
        );
        assert_eq!(loaded.patterns.len(), 1);
        assert_eq!(loaded.patterns[0].pattern, "/trusted/*/.git");

        // Verify V3 format on disk
        let contents = std::fs::read_to_string(&path).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(saved["version"], 3);
        assert!(saved["repositories"]["/project/.git"]["trust"].is_object());
    }

    #[test]
    fn test_trust_database_pattern_matching() {
        let mut db = TrustDatabase::default();
        db.add_pattern(
            "/Users/dev/trusted/*/.git".to_string(),
            TrustLevel::Allow,
            None,
        );

        // Should match
        assert_eq!(
            db.get_trust_level(Path::new("/Users/dev/trusted/project/.git")),
            TrustLevel::Allow
        );

        // Should not match
        assert_eq!(
            db.get_trust_level(Path::new("/Users/dev/untrusted/project/.git")),
            TrustLevel::Deny
        );
    }

    #[test]
    fn test_glob_matching_simple() {
        assert!(matches_glob("*", "anything"));
        assert!(matches_glob("foo", "foo"));
        assert!(!matches_glob("foo", "bar"));
    }

    #[test]
    fn test_glob_matching_wildcard() {
        assert!(matches_glob("foo/*", "foo/bar"));
        assert!(matches_glob("foo/*/baz", "foo/bar/baz"));
        assert!(!matches_glob("foo/*", "foo/bar/baz"));
    }

    #[test]
    fn test_glob_matching_double_star() {
        assert!(matches_glob("foo/**", "foo/bar"));
        assert!(matches_glob("foo/**", "foo/bar/baz"));
        assert!(matches_glob("foo/**/baz", "foo/baz"));
        assert!(matches_glob("foo/**/baz", "foo/bar/baz"));
        assert!(matches_glob("foo/**/baz", "foo/a/b/c/baz"));
    }

    #[test]
    fn test_list_trusted() {
        let mut db = TrustDatabase::default();
        db.set_trust_level(Path::new("/project1/.git"), TrustLevel::Allow);
        db.set_trust_level(Path::new("/project2/.git"), TrustLevel::Prompt);
        db.set_trust_level(Path::new("/project3/.git"), TrustLevel::Deny);

        let trusted = db.list_trusted();
        assert_eq!(trusted.len(), 2);
    }

    #[test]
    fn test_prune_removes_nonexistent() {
        let mut db = TrustDatabase::default();
        db.set_trust_level(Path::new("/nonexistent/path/a/.git"), TrustLevel::Allow);
        db.set_trust_level(Path::new("/nonexistent/path/b/.git"), TrustLevel::Prompt);

        let removed = db.prune();
        assert_eq!(removed.len(), 2);
        assert!(db.repositories.is_empty());
    }

    #[test]
    fn test_prune_keeps_existing() {
        let temp = tempdir().unwrap();
        let existing_path = temp.path().join(".git");
        std::fs::create_dir_all(&existing_path).unwrap();

        let mut db = TrustDatabase::default();
        db.set_trust_level(&existing_path, TrustLevel::Allow);
        db.set_trust_level(Path::new("/nonexistent/path/.git"), TrustLevel::Allow);

        let removed = db.prune();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0], "/nonexistent/path/.git");
        assert_eq!(db.repositories.len(), 1);
        assert!(db.has_explicit_trust(&existing_path));
    }

    #[test]
    fn test_prune_empty_database() {
        let mut db = TrustDatabase::default();
        let removed = db.prune();
        assert!(removed.is_empty());
    }

    #[test]
    fn test_load_v1_format_migrates_to_v3() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("trust.json");

        // Create a V1 format file with string timestamp
        let v1_json = r#"{
            "version": 1,
            "default_level": "deny",
            "repositories": {
                "/path/to/repo/.git": {
                    "level": "allow",
                    "granted_at": "2025-01-28T10:30:00Z",
                    "granted_by": "user"
                }
            },
            "patterns": []
        }"#;
        std::fs::write(&path, v1_json).unwrap();

        // Load should migrate V1 -> V2 -> V3
        let db = TrustDatabase::load_from(&path).unwrap();
        assert_eq!(db.version, 3);
        assert_eq!(db.default_level, TrustLevel::Deny);

        let entry = db.repositories.get("/path/to/repo/.git").unwrap();
        assert_eq!(entry.level, TrustLevel::Allow);
        // 2025-01-28T10:30:00Z = 1738060200 seconds since epoch
        assert_eq!(entry.granted_at, 1738060200);
        assert_eq!(entry.granted_by, "user");

        // load_from is side-effect-free: it migrates in memory only and must
        // NOT touch disk. trust.json stays put; repos.json is not created until
        // the next update() persists the V3 form (see
        // test_update_migrates_legacy_trust_json).
        assert!(path.exists(), "load_from must not remove trust.json");
        assert!(
            !temp_dir.path().join("repos.json").exists(),
            "load_from must not create repos.json"
        );
    }

    #[test]
    fn test_load_mislabeled_v2_as_v1() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("trust.json");

        // Create a file that says version 1 but has integer timestamp (V2 schema)
        // This simulates the historical bug where version wasn't updated
        let mislabeled_json = r#"{
            "version": 1,
            "default_level": "allow",
            "repositories": {
                "/path/to/repo/.git": {
                    "level": "allow",
                    "granted_at": 1738060200,
                    "granted_by": "user"
                }
            },
            "patterns": []
        }"#;
        std::fs::write(&path, mislabeled_json).unwrap();

        // Load should detect it's actually V2, migrate to V3
        let db = TrustDatabase::load_from(&path).unwrap();
        assert_eq!(db.version, 3);

        let entry = db.repositories.get("/path/to/repo/.git").unwrap();
        assert_eq!(entry.granted_at, 1738060200);

        // load_from migrates in memory only — no disk writes.
        assert!(path.exists(), "load_from must not remove trust.json");
        assert!(!temp_dir.path().join("repos.json").exists());
    }

    #[test]
    fn test_detect_schema_version() {
        // V1: string timestamp
        let v1_json: serde_json::Value = serde_json::from_str(
            r#"{
            "version": 1,
            "repositories": {
                "/repo/.git": {
                    "level": "allow",
                    "granted_at": "2025-01-28T10:30:00Z"
                }
            }
        }"#,
        )
        .unwrap();
        assert_eq!(detect_schema_version(&v1_json, 1), 1);

        // V2: integer timestamp
        let v2_json: serde_json::Value = serde_json::from_str(
            r#"{
            "version": 1,
            "repositories": {
                "/repo/.git": {
                    "level": "allow",
                    "granted_at": 1738060200
                }
            }
        }"#,
        )
        .unwrap();
        assert_eq!(detect_schema_version(&v2_json, 1), 2);

        // Empty repositories - defaults to V2
        let empty_json: serde_json::Value = serde_json::from_str(
            r#"{
            "version": 1,
            "repositories": {}
        }"#,
        )
        .unwrap();
        assert_eq!(detect_schema_version(&empty_json, 1), 2);

        // Stated version 2 - trust it
        let stated_v2: serde_json::Value = serde_json::from_str(
            r#"{
            "version": 2,
            "repositories": {}
        }"#,
        )
        .unwrap();
        assert_eq!(detect_schema_version(&stated_v2, 2), 2);
    }

    #[test]
    fn test_trust_entry_with_fingerprint() {
        let entry = TrustEntry::with_fingerprint(
            TrustLevel::Allow,
            "git@github.com:user/repo.git".to_string(),
        );
        assert_eq!(entry.level, TrustLevel::Allow);
        assert_eq!(
            entry.fingerprint,
            Some("git@github.com:user/repo.git".to_string())
        );
        assert!(entry.granted_at > 0);
    }

    #[test]
    fn test_trust_entry_without_fingerprint() {
        let entry = TrustEntry::new(TrustLevel::Allow);
        assert_eq!(entry.level, TrustLevel::Allow);
        assert_eq!(entry.fingerprint, None);
    }

    #[test]
    fn test_set_and_get_trust_with_fingerprint() {
        let mut db = TrustDatabase::default();
        let git_dir = Path::new("/path/to/repo/.git");

        db.set_trust_level_with_fingerprint(
            git_dir,
            TrustLevel::Allow,
            "git@github.com:user/repo.git".to_string(),
        );

        let entry = db.get_trust_entry(git_dir).unwrap();
        assert_eq!(entry.level, TrustLevel::Allow);
        assert_eq!(
            entry.fingerprint,
            Some("git@github.com:user/repo.git".to_string())
        );
    }

    #[test]
    fn test_get_trust_entry_returns_none_for_missing() {
        let db = TrustDatabase::default();
        assert!(db.get_trust_entry(Path::new("/missing/.git")).is_none());
    }

    #[test]
    fn test_fingerprint_survives_serialization() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("repos.json");

        let mut db = TrustDatabase::default();
        db.set_trust_level_with_fingerprint(
            Path::new("/project/.git"),
            TrustLevel::Allow,
            "https://github.com/user/project.git".to_string(),
        );
        db.save_to(&path).unwrap();

        let loaded = TrustDatabase::load_from(&path).unwrap();
        let entry = loaded.repositories.get("/project/.git").unwrap();
        assert_eq!(
            entry.fingerprint,
            Some("https://github.com/user/project.git".to_string())
        );
    }

    #[test]
    fn test_loading_db_without_fingerprint_field() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("trust.json");

        // Simulate a V2 database without fingerprint fields (pre-upgrade)
        let json = r#"{
            "version": 2,
            "default_level": "deny",
            "repositories": {
                "/old/repo/.git": {
                    "level": "allow",
                    "granted_at": 1700000000,
                    "granted_by": "user"
                }
            },
            "patterns": []
        }"#;
        std::fs::write(&path, json).unwrap();

        // V2 trust.json triggers migration to V3 repos.json
        let db = TrustDatabase::load_from(&path).unwrap();
        assert_eq!(db.version, 3);
        let entry = db.repositories.get("/old/repo/.git").unwrap();
        assert_eq!(entry.level, TrustLevel::Allow);
        assert_eq!(entry.fingerprint, None);

        // load_from migrates in memory only — no disk writes.
        assert!(path.exists(), "load_from must not remove trust.json");
        assert!(!temp_dir.path().join("repos.json").exists());
    }

    #[test]
    fn test_backfill_fingerprints_skips_nonexistent_paths() {
        let mut db = TrustDatabase::default();
        // Entry at a path that doesn't exist — git can't resolve a remote
        db.set_trust_level(Path::new("/nonexistent/repo/.git"), TrustLevel::Allow);
        assert_eq!(db.backfill_fingerprints(), 0);
        // Fingerprint should remain None
        let entry = db.repositories.get("/nonexistent/repo/.git").unwrap();
        assert_eq!(entry.fingerprint, None);
    }

    #[test]
    fn test_backfill_fingerprints_skips_already_fingerprinted() {
        let mut db = TrustDatabase::default();
        db.set_trust_level_with_fingerprint(
            Path::new("/some/repo/.git"),
            TrustLevel::Allow,
            "https://github.com/user/repo.git".to_string(),
        );
        // Already has a fingerprint, so backfill should skip it
        assert_eq!(db.backfill_fingerprints(), 0);
        let entry = db.repositories.get("/some/repo/.git").unwrap();
        assert_eq!(
            entry.fingerprint,
            Some("https://github.com/user/repo.git".to_string())
        );
    }

    #[test]
    fn test_fingerprint_not_serialized_when_none() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("repos.json");

        let mut db = TrustDatabase::default();
        db.set_trust_level(Path::new("/project/.git"), TrustLevel::Allow);
        db.save_to(&path).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            !contents.contains("fingerprint"),
            "fingerprint should not appear in JSON when None"
        );
    }

    #[test]
    fn test_set_and_get_layout() {
        let mut db = TrustDatabase::default();
        let git_dir = Path::new("/path/to/repo/.git");

        assert!(db.get_layout(git_dir).is_none());

        db.set_layout(git_dir, "simple".to_string());
        assert_eq!(db.get_layout(git_dir), Some("simple"));

        db.set_layout(git_dir, "grouped".to_string());
        assert_eq!(db.get_layout(git_dir), Some("grouped"));
    }

    #[test]
    fn test_layout_survives_v3_round_trip() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("repos.json");

        let mut db = TrustDatabase::default();
        db.set_trust_level(Path::new("/project/.git"), TrustLevel::Allow);
        db.set_layout(Path::new("/project/.git"), "simple".to_string());
        db.set_layout(Path::new("/layout-only/.git"), "grouped".to_string());
        db.save_to(&path).unwrap();

        // Verify V3 JSON shape on disk
        let contents = std::fs::read_to_string(&path).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(saved["version"], 3);
        // /project/.git has both trust and layout
        assert!(saved["repositories"]["/project/.git"]["trust"].is_object());
        assert_eq!(saved["repositories"]["/project/.git"]["layout"], "simple");
        // /layout-only/.git has only layout, no trust key
        assert!(saved["repositories"]["/layout-only/.git"]["trust"].is_null());
        assert_eq!(
            saved["repositories"]["/layout-only/.git"]["layout"],
            "grouped"
        );

        // Reload and verify
        let loaded = TrustDatabase::load_from(&path).unwrap();
        assert_eq!(loaded.version, 3);
        assert_eq!(
            loaded.get_trust_level(Path::new("/project/.git")),
            TrustLevel::Allow
        );
        assert_eq!(
            loaded.get_layout(Path::new("/project/.git")),
            Some("simple")
        );
        assert_eq!(
            loaded.get_layout(Path::new("/layout-only/.git")),
            Some("grouped")
        );
        // layout-only entry should NOT have a trust entry
        assert!(!loaded.repositories.contains_key("/layout-only/.git"));
    }

    #[test]
    fn test_prune_cleans_layouts() {
        let mut db = TrustDatabase::default();
        db.set_layout(Path::new("/nonexistent/path/a/.git"), "simple".to_string());
        db.set_layout(Path::new("/nonexistent/path/b/.git"), "grouped".to_string());

        let removed = db.prune();
        assert_eq!(removed.len(), 2);
        assert!(db.layouts.is_empty());
    }

    #[test]
    fn test_update_migrates_legacy_trust_json() {
        let temp_dir = tempdir().unwrap();
        let trust_path = temp_dir.path().join("trust.json");
        let repos_path = temp_dir.path().join("repos.json");

        // Write V2 trust.json (legacy on-disk format).
        let v2_json = r#"{
            "version": 2,
            "default_level": "deny",
            "repositories": {
                "/path/to/repo/.git": {
                    "level": "allow",
                    "granted_at": 1738060200,
                    "granted_by": "user",
                    "fingerprint": "https://github.com/user/repo.git"
                }
            },
            "patterns": [
                {
                    "pattern": "/trusted/**/.git",
                    "level": "allow"
                }
            ]
        }"#;
        std::fs::write(&trust_path, v2_json).unwrap();

        // A locked update() reads the legacy file, writes V3 repos.json, and
        // retires trust.json — all under the registry lock. The no-op closure
        // exercises pure migration.
        TrustDatabase::update_in(temp_dir.path(), |_db| Ok(())).unwrap();

        // trust.json retired, repos.json created with V3 format.
        assert!(
            !trust_path.exists(),
            "trust.json should be retired after update"
        );
        assert!(repos_path.exists(), "repos.json should be created");

        let contents = std::fs::read_to_string(&repos_path).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(saved["version"], 3);
        assert!(saved["repositories"]["/path/to/repo/.git"]["trust"].is_object());
        assert_eq!(
            saved["repositories"]["/path/to/repo/.git"]["trust"]["level"],
            "allow"
        );
        assert_eq!(
            saved["repositories"]["/path/to/repo/.git"]["trust"]["fingerprint"],
            "https://github.com/user/repo.git"
        );
        assert_eq!(saved["patterns"][0]["pattern"], "/trusted/**/.git");

        // And the migrated data roundtrips when reloaded.
        let db = TrustDatabase::load_from(&repos_path).unwrap();
        let entry = db.repositories.get("/path/to/repo/.git").unwrap();
        assert_eq!(entry.level, TrustLevel::Allow);
        assert_eq!(entry.granted_at, 1738060200);
        assert_eq!(
            entry.fingerprint,
            Some("https://github.com/user/repo.git".to_string())
        );
        assert_eq!(db.patterns.len(), 1);
    }

    // ---- #666 regression tests: atomic, lock-serialized registry writes ----

    /// The lost-update bug: concurrent writers each rewrote the whole file from
    /// their own snapshot, silently dropping each other's entries. With the
    /// exclusive lock + reload-inside-the-lock in `update`, every distinct entry
    /// must survive. This fails against the old unlocked `load()...save()` path.
    ///
    /// Each `update_in` opens the lock file fresh (its own open file
    /// description), so the threads genuinely contend on the `flock` — a shared
    /// handle would re-lock the same OFD as a no-op and pass without contending.
    #[test]
    fn concurrent_updates_do_not_lose_entries() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let temp = Arc::new(tempdir().unwrap());
        let threads = 8;
        let rounds = 25;
        let barrier = Arc::new(Barrier::new(threads));

        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let temp = Arc::clone(&temp);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    for r in 0..rounds {
                        let key = format!("/repo/t{t}/r{r}/.git");
                        TrustDatabase::update_in(temp.path(), |db| {
                            db.set_trust_level(Path::new(&key), TrustLevel::Allow);
                            Ok(())
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let db = TrustDatabase::load_from(&temp.path().join("repos.json")).unwrap();
        assert_eq!(
            db.repositories.len(),
            threads * rounds,
            "entries were lost to a write race"
        );
        for t in 0..threads {
            for r in 0..rounds {
                let key = format!("/repo/t{t}/r{r}/.git");
                assert!(db.repositories.contains_key(&key), "missing entry {key}");
            }
        }
    }

    /// The torn-write bug: `fs::write` truncates in place, so a concurrent
    /// reader could observe a half-written file. With the tmp-file + rename in
    /// `save_to`, an unlocked reader always sees a complete, parseable file —
    /// either the old inode or the new one, never a torn one.
    #[test]
    fn concurrent_reads_never_see_a_torn_file() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;

        let temp = Arc::new(tempdir().unwrap());
        let path = temp.path().join("repos.json");

        // Seed a valid registry.
        TrustDatabase::update_in(temp.path(), |db| {
            db.set_trust_level(Path::new("/seed/.git"), TrustLevel::Allow);
            Ok(())
        })
        .unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let temp = Arc::clone(&temp);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut i = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    // Cycle a few keys so the file stays small and writes stay hot.
                    let key = format!("/w/{}/.git", i % 4);
                    TrustDatabase::update_in(temp.path(), |db| {
                        db.set_trust_level(Path::new(&key), TrustLevel::Allow);
                        Ok(())
                    })
                    .unwrap();
                    i += 1;
                }
            })
        };

        // Every unlocked read must parse successfully (load_from returns Err on a
        // torn/half-written file, so `.unwrap()` would panic).
        for _ in 0..300 {
            let db = TrustDatabase::load_from(&path).unwrap();
            assert!(
                db.get_trust_level(Path::new("/seed/.git")) == TrustLevel::Allow,
                "seed entry vanished — a write clobbered unrelated entries"
            );
        }

        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
    }

    /// A corrupt registry must be preserved for recovery, not silently
    /// overwritten, when the next `update` runs. The write still succeeds
    /// (self-healing) and the corrupt bytes land in a `.corrupt-*.bak` sibling.
    #[test]
    fn update_backs_up_corrupt_registry_and_starts_fresh() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("repos.json");
        std::fs::write(&path, "{ this is not valid json").unwrap();

        // load_from surfaces the corruption as an error...
        assert!(TrustDatabase::load_from(&path).is_err());

        // ...and update() recovers: backs the corrupt file up, writes fresh.
        TrustDatabase::update_in(temp.path(), |db| {
            db.set_trust_level(Path::new("/new/.git"), TrustLevel::Allow);
            Ok(())
        })
        .unwrap();

        let db = TrustDatabase::load_from(&path).unwrap();
        assert_eq!(
            db.get_trust_level(Path::new("/new/.git")),
            TrustLevel::Allow
        );

        let backups: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("repos.json.corrupt-")
            })
            .collect();
        assert_eq!(backups.len(), 1, "corrupt registry should be backed up");
    }

    /// A corrupt registry surfaces as an `Err` from `load()` so diagnostic
    /// commands (`doctor`, `hooks status`) can flag it, while the hook-execution
    /// path's `unwrap_or_default()` fails **closed** to an empty deny-all
    /// database. Uses `DAFT_CONFIG_DIR` (honored under `cfg(test)`) so it never
    /// touches the real config dir.
    #[test]
    #[serial_test::serial]
    fn corrupt_registry_surfaces_error_and_reads_fail_closed() {
        let temp = tempdir().unwrap();
        // SAFETY: serialized by `#[serial]`; env mutation is process-global.
        unsafe {
            std::env::set_var(crate::CONFIG_DIR_ENV, temp.path());
        }
        std::fs::write(temp.path().join("repos.json"), "{ not json").unwrap();

        let loaded = TrustDatabase::load();
        let denied = TrustDatabase::load().unwrap_or_default();
        // SAFETY: as above; restore before asserting so a failure can't leak it.
        unsafe {
            std::env::remove_var(crate::CONFIG_DIR_ENV);
        }

        assert!(
            loaded.is_err(),
            "a corrupt registry must surface as Err so diagnostics can flag it"
        );
        assert!(denied.repositories.is_empty());
        assert_eq!(
            denied.get_trust_level(Path::new("/anything/.git")),
            TrustLevel::Deny,
            "hot-path reads must fail closed to Deny on a corrupt registry"
        );
    }

    /// #666 review regression: a no-op `update_if` against a corrupt registry
    /// must leave it byte-for-byte intact — no `.corrupt-*.bak`, no delete.
    /// Backing it up on a no-op (background prune, worktree-remove of an unknown
    /// repo) would rename the live file aside and write nothing, re-creating the
    /// silent deny-all incident.
    #[test]
    fn no_op_update_if_leaves_corrupt_registry_intact() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("repos.json");
        let corrupt = "{ this is not valid json";
        std::fs::write(&path, corrupt).unwrap();

        // A no-op change (changed=false), as the pruner produces when there is
        // nothing to prune.
        TrustDatabase::with_lock(temp.path(), |_db| Ok((false, ()))).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupt,
            "a no-op must leave the corrupt registry byte-for-byte intact"
        );
        let backups = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt-"))
            .count();
        assert_eq!(backups, 0, "a no-op must not create a backup");
    }
}
