//! Versioned Data Transfer Objects for trust database.
//!
//! Each schema version has its own struct. Migrations between versions are explicit
//! and type-safe using the `version-migrate` crate.
//!
//! # Version History
//!
//! - **V1.0.0**: Original schema with string timestamps (ISO 8601 format)
//! - **V2.0.0**: Schema with Unix epoch timestamps (i64)
//! - **V3.0.0**: Unified repo store with trust + layout per entry

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
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub fingerprint: Option<String>,
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
                        fingerprint: None,
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
                        fingerprint: entry.fingerprint,
                    },
                )
            })
            .collect();

        TrustDatabase {
            version: 3,
            default_level: self.default_level,
            repositories,
            layouts: HashMap::new(),
            patterns: self.patterns,
        }
    }
}

/// V3: Unified repo store with trust + layout.
#[derive(Debug, Clone, Serialize, Deserialize, Versioned)]
#[versioned(version = "3.0.0")]
pub struct RepoStoreV3_0_0 {
    #[serde(default)]
    pub repositories: HashMap<String, RepoEntryV3_0_0>,
    #[serde(default)]
    pub patterns: Vec<TrustPattern>,
}

/// Per-repository entry in V3 schema, combining trust and layout data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntryV3_0_0 {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust: Option<TrustEntryV2_0_0>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
}

/// Migration: V2 -> V3 (wrap trust entries, add layout field).
impl MigratesTo<RepoStoreV3_0_0> for TrustDatabaseV2_0_0 {
    fn migrate(self) -> RepoStoreV3_0_0 {
        let repositories = self
            .repositories
            .into_iter()
            .map(|(path, entry)| {
                (
                    path,
                    RepoEntryV3_0_0 {
                        trust: Some(entry),
                        layout: None,
                    },
                )
            })
            .collect();
        RepoStoreV3_0_0 {
            repositories,
            patterns: self.patterns,
        }
    }
}

/// Convert V3 DTO to domain model.
impl IntoDomain<TrustDatabase> for RepoStoreV3_0_0 {
    fn into_domain(self) -> TrustDatabase {
        let mut repositories = HashMap::new();
        let mut layouts = HashMap::new();
        for (path, entry) in self.repositories {
            if let Some(trust) = entry.trust {
                repositories.insert(
                    path.clone(),
                    TrustEntry {
                        level: trust.level,
                        granted_at: trust.granted_at,
                        granted_by: trust.granted_by,
                        fingerprint: trust.fingerprint,
                    },
                );
            }
            if let Some(layout) = entry.layout {
                layouts.insert(path, layout);
            }
        }
        TrustDatabase {
            version: 3,
            default_level: TrustLevel::Deny,
            repositories,
            layouts,
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
                        fingerprint: None,
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

        assert_eq!(db.version, 3);
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

    #[test]
    fn test_v2_to_v3_migration() {
        let v2 = TrustDatabaseV2_0_0 {
            default_level: TrustLevel::Deny,
            repositories: {
                let mut map = HashMap::new();
                map.insert(
                    "/path/to/repo/.git".to_string(),
                    TrustEntryV2_0_0 {
                        level: TrustLevel::Allow,
                        granted_at: 1738060200,
                        granted_by: "user".to_string(),
                        fingerprint: Some("https://github.com/user/repo.git".to_string()),
                    },
                );
                map
            },
            patterns: vec![TrustPattern {
                pattern: "/trusted/**/.git".to_string(),
                level: TrustLevel::Allow,
                comment: None,
            }],
        };

        let v3: RepoStoreV3_0_0 = v2.migrate();

        assert_eq!(v3.repositories.len(), 1);
        let entry = v3.repositories.get("/path/to/repo/.git").unwrap();
        assert!(entry.trust.is_some());
        assert!(entry.layout.is_none());

        let trust = entry.trust.as_ref().unwrap();
        assert_eq!(trust.level, TrustLevel::Allow);
        assert_eq!(trust.granted_at, 1738060200);
        assert_eq!(trust.granted_by, "user");
        assert_eq!(
            trust.fingerprint,
            Some("https://github.com/user/repo.git".to_string())
        );

        assert_eq!(v3.patterns.len(), 1);
        assert_eq!(v3.patterns[0].pattern, "/trusted/**/.git");
    }

    #[test]
    fn test_v3_into_domain() {
        let v3 = RepoStoreV3_0_0 {
            repositories: {
                let mut map = HashMap::new();
                map.insert(
                    "/path/to/repo/.git".to_string(),
                    RepoEntryV3_0_0 {
                        trust: Some(TrustEntryV2_0_0 {
                            level: TrustLevel::Allow,
                            granted_at: 1738060200,
                            granted_by: "user".to_string(),
                            fingerprint: None,
                        }),
                        layout: Some("simple".to_string()),
                    },
                );
                // Entry with only layout, no trust
                map.insert(
                    "/layout-only/.git".to_string(),
                    RepoEntryV3_0_0 {
                        trust: None,
                        layout: Some("grouped".to_string()),
                    },
                );
                map
            },
            patterns: vec![],
        };

        let db: TrustDatabase = v3.into_domain();

        assert_eq!(db.version, 3);
        // Trust entry present for repo with trust data
        let entry = db.repositories.get("/path/to/repo/.git").unwrap();
        assert_eq!(entry.level, TrustLevel::Allow);
        assert_eq!(entry.granted_at, 1738060200);
        // No trust entry for layout-only repo
        assert!(!db.repositories.contains_key("/layout-only/.git"));

        // Both layouts present
        assert_eq!(db.layouts.get("/path/to/repo/.git").unwrap(), "simple");
        assert_eq!(db.layouts.get("/layout-only/.git").unwrap(), "grouped");
    }
}
