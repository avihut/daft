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
        }
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
    /// Pattern-based trust rules.
    #[serde(default)]
    pub patterns: Vec<TrustPattern>,
}

fn default_version() -> u32 {
    2
}

impl Default for TrustDatabase {
    fn default() -> Self {
        Self {
            version: 2,
            default_level: TrustLevel::Deny,
            repositories: HashMap::new(),
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
    /// - V2: Current schema with epoch timestamps (i64)
    ///
    /// The migration from V1 to V2 converts string timestamps to Unix epoch.
    ///
    /// Note: Due to a historical bug, some V2 databases may have version=1.
    /// We detect this by checking the granted_at field type.
    pub fn load_from(path: &Path) -> Result<Self> {
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

        // Detect actual schema by checking granted_at field type in first repository entry
        // V1 has string timestamps, V2 has integer timestamps
        let actual_version = detect_schema_version(&json, stated_version);

        let db = match actual_version {
            1 => {
                // V1: String timestamps - need migration
                let v1: TrustDatabaseV1_0_0 = serde_json::from_value(json).with_context(|| {
                    format!("Failed to parse V1 trust database from {}", path.display())
                })?;
                let v2: TrustDatabaseV2_0_0 = v1.migrate();
                let db: TrustDatabase = v2.into_domain();

                // Save migrated version
                db.save_to(path)?;
                db
            }
            _ => {
                // V2 or later: Load directly, then ensure version is updated
                let mut db: TrustDatabase = serde_json::from_value(json).with_context(|| {
                    format!("Failed to parse trust database from {}", path.display())
                })?;

                // Update version if it was mislabeled
                if db.version != 2 {
                    db.version = 2;
                    db.save_to(path)?;
                }

                db
            }
        };

        Ok(db)
    }

    /// Save the trust database to the default location.
    pub fn save(&self) -> Result<()> {
        let path = Self::default_path()?;
        self.save_to(&path)
    }

    /// Save the trust database to a specific path.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }

        let contents =
            serde_json::to_string_pretty(self).context("Failed to serialize trust database")?;

        fs::write(path, contents)
            .with_context(|| format!("Failed to write trust database to {}", path.display()))?;

        Ok(())
    }

    /// Get the default path for the trust database.
    pub fn default_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir().context("Could not determine config directory")?;
        Ok(config_dir.join("daft").join("trust.json"))
    }

    /// Get the trust level for a repository.
    ///
    /// Checks in order:
    /// 1. Exact repository match
    /// 2. Pattern matches
    /// 3. Default level
    pub fn get_trust_level(&self, git_dir: &Path) -> TrustLevel {
        let git_dir_str = git_dir.to_string_lossy();

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

    /// Set the trust level for a repository.
    pub fn set_trust_level(&mut self, git_dir: &Path, level: TrustLevel) {
        let git_dir_str = git_dir.to_string_lossy().to_string();
        self.repositories
            .insert(git_dir_str, TrustEntry::new(level));
    }

    /// Remove trust for a repository.
    pub fn remove_trust(&mut self, git_dir: &Path) -> bool {
        let git_dir_str = git_dir.to_string_lossy();
        self.repositories.remove(git_dir_str.as_ref()).is_some()
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

    /// Clear all trust entries and patterns.
    pub fn clear(&mut self) {
        self.repositories.clear();
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
        let git_dir_str = git_dir.to_string_lossy();
        self.repositories.contains_key(git_dir_str.as_ref())
    }
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
        assert_eq!(db.version, 2);
        assert_eq!(db.default_level, TrustLevel::Deny);
        assert!(db.repositories.is_empty());
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
        let path = temp_dir.path().join("trust.json");

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
    fn test_load_v1_format_migrates_to_v2() {
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

        // Load should migrate to V2
        let db = TrustDatabase::load_from(&path).unwrap();
        assert_eq!(db.version, 2);
        assert_eq!(db.default_level, TrustLevel::Deny);

        let entry = db.repositories.get("/path/to/repo/.git").unwrap();
        assert_eq!(entry.level, TrustLevel::Allow);
        // 2025-01-28T10:30:00Z = 1738060200 seconds since epoch
        assert_eq!(entry.granted_at, 1738060200);
        assert_eq!(entry.granted_by, "user");

        // Verify the file was updated to V2 format
        let contents = std::fs::read_to_string(&path).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(saved["version"], 2);
        // granted_at should now be an integer
        assert!(saved["repositories"]["/path/to/repo/.git"]["granted_at"].is_number());
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

        // Load should detect it's actually V2 and load correctly
        let db = TrustDatabase::load_from(&path).unwrap();
        assert_eq!(db.version, 2);
        assert_eq!(db.default_level, TrustLevel::Allow);

        let entry = db.repositories.get("/path/to/repo/.git").unwrap();
        assert_eq!(entry.granted_at, 1738060200);

        // Verify the file was updated with correct version
        let contents = std::fs::read_to_string(&path).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(saved["version"], 2);
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
}
