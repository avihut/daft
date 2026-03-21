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

                // Atomic migration: write repos.json, then remove trust.json
                if path.file_name().and_then(|n| n.to_str()) == Some("trust.json") {
                    let repos_path = path.with_file_name("repos.json");
                    let tmp_path = path.with_file_name("repos.json.tmp");
                    db.save_to(&tmp_path)?;
                    fs::rename(&tmp_path, &repos_path)?;
                    let _ = fs::remove_file(path);
                } else {
                    db.save_to(path)?;
                }

                Ok(db)
            }
        }
    }

    /// Save the trust database to the default location (`repos.json`).
    pub fn save(&self) -> Result<()> {
        Self::repos_path().and_then(|p| self.save_to(&p))
    }

    /// Save the trust database to a specific path in V3 format.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        use super::trust_dto::{RepoEntryV3_0_0, TrustEntryV2_0_0};

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
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

        fs::write(path, contents)
            .with_context(|| format!("Failed to write trust database to {}", path.display()))?;

        Ok(())
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

    /// Get the canonical path for the V3 repo store.
    pub fn repos_path() -> Result<PathBuf> {
        Ok(crate::daft_config_dir()?.join("repos.json"))
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
    /// Returns the list of paths that were removed (from both repositories
    /// and layouts). The caller is responsible for calling `save()` to
    /// persist the changes.
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

    /// Backfill fingerprints for entries that exist on disk but have no
    /// fingerprint stored. Returns the number of entries updated. The caller
    /// is responsible for calling `save()` to persist the changes.
    pub fn backfill_fingerprints(&mut self) -> usize {
        let to_backfill: Vec<(String, String)> = self
            .repositories
            .iter()
            .filter(|(_, entry)| entry.fingerprint.is_none())
            .filter_map(|(path, _)| {
                let git_dir = Path::new(path.as_str());
                get_remote_url_for_git_dir(git_dir).map(|url| (path.clone(), url))
            })
            .collect();

        let count = to_backfill.len();
        for (path, url) in to_backfill {
            if let Some(entry) = self.repositories.get_mut(&path) {
                entry.fingerprint = Some(url);
            }
        }
        count
    }
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

        // trust.json should be removed and repos.json created (atomic migration)
        assert!(
            !path.exists(),
            "trust.json should be removed after migration"
        );
        let repos_path = temp_dir.path().join("repos.json");
        assert!(repos_path.exists(), "repos.json should be created");

        // Verify the migrated file is V3 format
        let contents = std::fs::read_to_string(&repos_path).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(saved["version"], 3);
        // V3 format wraps trust entries
        assert!(saved["repositories"]["/path/to/repo/.git"]["trust"].is_object());
        assert!(saved["repositories"]["/path/to/repo/.git"]["trust"]["granted_at"].is_number());
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

        // trust.json should be removed and repos.json created
        assert!(!path.exists());
        let repos_path = temp_dir.path().join("repos.json");
        let contents = std::fs::read_to_string(&repos_path).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(saved["version"], 3);
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

        // trust.json removed, repos.json created
        assert!(!path.exists());
        assert!(temp_dir.path().join("repos.json").exists());
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
    fn test_atomic_trust_json_migration() {
        let temp_dir = tempdir().unwrap();
        let trust_path = temp_dir.path().join("trust.json");
        let repos_path = temp_dir.path().join("repos.json");

        // Write V2 trust.json
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

        // Load from trust.json — should trigger atomic migration
        let db = TrustDatabase::load_from(&trust_path).unwrap();
        assert_eq!(db.version, 3);

        // trust.json should be removed
        assert!(
            !trust_path.exists(),
            "trust.json should be removed after migration"
        );

        // repos.json should be created with V3 format
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

        // Verify trust data roundtrips correctly
        let entry = db.repositories.get("/path/to/repo/.git").unwrap();
        assert_eq!(entry.level, TrustLevel::Allow);
        assert_eq!(entry.granted_at, 1738060200);
        assert_eq!(
            entry.fingerprint,
            Some("https://github.com/user/repo.git".to_string())
        );
        assert_eq!(db.patterns.len(), 1);
    }
}
