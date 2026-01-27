//! Versioned Data Transfer Objects for trust database.
//!
//! Each schema version has its own struct. Migrations between versions are explicit
//! and type-safe using the `version-migrate` crate.
//!
//! # Version History
//!
//! - **V1.0.0**: Original schema with string timestamps (ISO 8601 format)
//! - **V2.0.0**: Current schema with Unix epoch timestamps (i64)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use version_migrate::{IntoDomain, MigratesTo, Versioned};

use super::trust::{TrustDatabase, TrustEntry, TrustLevel, TrustPattern};

/// V1: Original schema with string timestamps.
///
/// This version stored `granted_at` as an ISO 8601 timestamp string.
#[derive(Debug, Clone, Serialize, Deserialize, Versioned)]
#[versioned(version = "1.0.0")]
pub struct TrustDatabaseV1_0_0 {
    #[serde(default)]
    pub default_level: TrustLevel,
    #[serde(default)]
    pub repositories: HashMap<String, TrustEntryV1_0_0>,
    #[serde(default)]
    pub patterns: Vec<TrustPattern>,
}

/// Trust entry for V1 schema with string timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntryV1_0_0 {
    pub level: TrustLevel,
    #[serde(default)]
    pub granted_at: String, // ISO 8601 timestamp string
    #[serde(default = "default_granted_by")]
    pub granted_by: String,
}

fn default_granted_by() -> String {
    "user".to_string()
}

/// V2: Current schema with epoch timestamps.
///
/// This version stores `granted_at` as a Unix epoch timestamp (seconds since 1970-01-01).
#[derive(Debug, Clone, Serialize, Deserialize, Versioned)]
#[versioned(version = "2.0.0")]
pub struct TrustDatabaseV2_0_0 {
    #[serde(default)]
    pub default_level: TrustLevel,
    #[serde(default)]
    pub repositories: HashMap<String, TrustEntryV2_0_0>,
    #[serde(default)]
    pub patterns: Vec<TrustPattern>,
}

/// Trust entry for V2 schema with epoch timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntryV2_0_0 {
    pub level: TrustLevel,
    #[serde(default)]
    pub granted_at: i64, // Unix epoch seconds
    #[serde(default = "default_granted_by")]
    pub granted_by: String,
}

/// Migration: V1 -> V2 (string timestamps to epoch).
///
/// Converts ISO 8601 timestamp strings to Unix epoch seconds.
/// Invalid or empty timestamps default to 0.
impl MigratesTo<TrustDatabaseV2_0_0> for TrustDatabaseV1_0_0 {
    fn migrate(self) -> TrustDatabaseV2_0_0 {
        use chrono::DateTime;

        let repositories = self
            .repositories
            .into_iter()
            .map(|(path, entry)| {
                let granted_at = if entry.granted_at.is_empty() {
                    0
                } else {
                    DateTime::parse_from_rfc3339(&entry.granted_at)
                        .map(|dt| dt.timestamp())
                        .unwrap_or(0)
                };
                (
                    path,
                    TrustEntryV2_0_0 {
                        level: entry.level,
                        granted_at,
                        granted_by: entry.granted_by,
                    },
                )
            })
            .collect();

        TrustDatabaseV2_0_0 {
            default_level: self.default_level,
            repositories,
            patterns: self.patterns,
        }
    }
}

/// Convert V2 DTO to domain model.
impl IntoDomain<TrustDatabase> for TrustDatabaseV2_0_0 {
    fn into_domain(self) -> TrustDatabase {
        let repositories = self
            .repositories
            .into_iter()
            .map(|(path, entry)| {
                (
                    path,
                    TrustEntry {
                        level: entry.level,
                        granted_at: entry.granted_at,
                        granted_by: entry.granted_by,
                    },
                )
            })
            .collect();

        TrustDatabase {
            version: 2,
            default_level: self.default_level,
            repositories,
            patterns: self.patterns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_v1_to_v2_migration_with_valid_timestamp() {
        let v1 = TrustDatabaseV1_0_0 {
            default_level: TrustLevel::Deny,
            repositories: {
                let mut map = HashMap::new();
                map.insert(
                    "/path/to/repo/.git".to_string(),
                    TrustEntryV1_0_0 {
                        level: TrustLevel::Allow,
                        granted_at: "2025-01-28T10:30:00Z".to_string(),
                        granted_by: "user".to_string(),
                    },
                );
                map
            },
            patterns: vec![],
        };

        let v2 = v1.migrate();

        assert_eq!(v2.default_level, TrustLevel::Deny);
        let entry = v2.repositories.get("/path/to/repo/.git").unwrap();
        assert_eq!(entry.level, TrustLevel::Allow);
        // 2025-01-28T10:30:00Z = 1738060200 seconds since epoch
        assert_eq!(entry.granted_at, 1738060200);
        assert_eq!(entry.granted_by, "user");
    }

    #[test]
    fn test_v1_to_v2_migration_with_empty_timestamp() {
        let v1 = TrustDatabaseV1_0_0 {
            default_level: TrustLevel::Prompt,
            repositories: {
                let mut map = HashMap::new();
                map.insert(
                    "/path/to/repo/.git".to_string(),
                    TrustEntryV1_0_0 {
                        level: TrustLevel::Allow,
                        granted_at: String::new(),
                        granted_by: "user".to_string(),
                    },
                );
                map
            },
            patterns: vec![],
        };

        let v2 = v1.migrate();

        let entry = v2.repositories.get("/path/to/repo/.git").unwrap();
        assert_eq!(entry.granted_at, 0);
    }

    #[test]
    fn test_v1_to_v2_migration_with_invalid_timestamp() {
        let v1 = TrustDatabaseV1_0_0 {
            default_level: TrustLevel::Deny,
            repositories: {
                let mut map = HashMap::new();
                map.insert(
                    "/path/to/repo/.git".to_string(),
                    TrustEntryV1_0_0 {
                        level: TrustLevel::Allow,
                        granted_at: "invalid-timestamp".to_string(),
                        granted_by: "user".to_string(),
                    },
                );
                map
            },
            patterns: vec![],
        };

        let v2 = v1.migrate();

        let entry = v2.repositories.get("/path/to/repo/.git").unwrap();
        assert_eq!(entry.granted_at, 0);
    }

    #[test]
    fn test_v2_into_domain() {
        let v2 = TrustDatabaseV2_0_0 {
            default_level: TrustLevel::Allow,
            repositories: {
                let mut map = HashMap::new();
                map.insert(
                    "/path/to/repo/.git".to_string(),
                    TrustEntryV2_0_0 {
                        level: TrustLevel::Allow,
                        granted_at: 1738060200,
                        granted_by: "user".to_string(),
                    },
                );
                map
            },
            patterns: vec![TrustPattern {
                pattern: "/trusted/**/.git".to_string(),
                level: TrustLevel::Allow,
                comment: Some("Trusted org".to_string()),
            }],
        };

        let db: TrustDatabase = v2.into_domain();

        assert_eq!(db.version, 2);
        assert_eq!(db.default_level, TrustLevel::Allow);
        let entry = db.repositories.get("/path/to/repo/.git").unwrap();
        assert_eq!(entry.level, TrustLevel::Allow);
        assert_eq!(entry.granted_at, 1738060200);
        assert_eq!(db.patterns.len(), 1);
    }

    #[test]
    fn test_v1_to_v2_preserves_patterns() {
        let v1 = TrustDatabaseV1_0_0 {
            default_level: TrustLevel::Deny,
            repositories: HashMap::new(),
            patterns: vec![
                TrustPattern {
                    pattern: "/home/user/work/**/.git".to_string(),
                    level: TrustLevel::Allow,
                    comment: Some("Work projects".to_string()),
                },
                TrustPattern {
                    pattern: "/tmp/**/.git".to_string(),
                    level: TrustLevel::Deny,
                    comment: None,
                },
            ],
        };

        let v2 = v1.migrate();

        assert_eq!(v2.patterns.len(), 2);
        assert_eq!(v2.patterns[0].pattern, "/home/user/work/**/.git");
        assert_eq!(v2.patterns[0].level, TrustLevel::Allow);
        assert_eq!(v2.patterns[1].pattern, "/tmp/**/.git");
        assert_eq!(v2.patterns[1].level, TrustLevel::Deny);
    }
}
