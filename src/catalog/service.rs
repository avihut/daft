//! The catalog service — single choke point between commands and the
//! catalog store.
//!
//! Commands never touch `rusqlite` or the `store::repos` layer directly;
//! they hold a [`Catalog`] and call its methods. Two open modes with very
//! different contracts:
//!
//! * [`Catalog::open_rw`] — open-or-create, runs migrations, full writer
//!   pool. For registration and the `daft repo` verbs.
//! * [`Catalog::open_ro`] — fail-fast reader for hot paths (tab
//!   completion, lookup fallbacks). Never creates the file, never blocks
//!   longer than the reader busy-timeout (300 ms), and callers must treat
//!   every error as "no catalog". `Ok(None)` means the catalog simply
//!   doesn't exist yet.

use crate::catalog::normalize;
use crate::store::error::StoreError;
use crate::store::repos::with_write_txn;
use crate::store::{CatalogRepoRow, CatalogReposRepo, Pool, connection, migrate, paths};
use chrono::Utc;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("repository '{needle}' not found in the catalog")]
    NotFound {
        needle: String,
        /// Close live-entry names, for "did you mean" hints.
        suggestions: Vec<String>,
    },

    #[error("the name '{name}' is already used by '{path}'")]
    NameTaken { name: String, path: String },

    #[error("invalid catalog name '{name}': {reason}")]
    InvalidName { name: String, reason: String },

    #[error("repo catalog unavailable: {0}")]
    Unavailable(#[from] StoreError),
}

pub type Result<T> = std::result::Result<T, CatalogError>;

/// Everything registration needs to know about a repo. Paths must be
/// canonical (callers go through `registration::gather_facts`).
#[derive(Debug, Clone)]
pub struct RegistrationFacts {
    pub uuid: String,
    /// Name used only when the uuid is new to the catalog; existing entries
    /// keep their (possibly user-chosen) name.
    pub default_name: String,
    pub path: String,
    pub git_common_dir: String,
    pub remote_url: Option<String>,
    pub default_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationOutcome {
    pub assigned_name: String,
    /// The default name was taken by another live repo and got `-N` suffixed.
    pub suffixed: bool,
    /// The uuid existed as a removed entry and came back to life.
    pub resurrected: bool,
    /// A brand-new row was inserted (vs. refreshing an existing one).
    pub created: bool,
}

enum Inner {
    Rw(Pool),
    Ro(rusqlite::Connection),
}

pub struct Catalog {
    inner: Inner,
}

impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mode = match &self.inner {
            Inner::Rw(_) => "rw",
            Inner::Ro(_) => "ro",
        };
        f.debug_struct("Catalog").field("mode", &mode).finish()
    }
}

impl Catalog {
    /// Open or create the global catalog, running migrations.
    pub fn open_rw() -> Result<Self> {
        let path = paths::catalog_db()?;
        Self::open_rw_at(&path)
    }

    /// Fail-fast read-only open. `Ok(None)` when the catalog file does not
    /// exist; never creates it. See the module docs for the hot-path
    /// contract.
    pub fn open_ro() -> Result<Option<Self>> {
        let Some(path) = paths::catalog_db_probe() else {
            return Ok(None);
        };
        Self::open_ro_at(&path)
    }

    /// Open-or-create at an explicit DB path (test seam; production goes
    /// through [`Catalog::open_rw`]).
    pub fn open_rw_at(db_path: &Path) -> Result<Self> {
        let pool = Pool::open_with(db_path, &migrate::catalog_set())?;
        Ok(Self {
            inner: Inner::Rw(pool),
        })
    }

    /// Read-only open at an explicit DB path (test seam).
    pub fn open_ro_at(db_path: &Path) -> Result<Option<Self>> {
        if !db_path.exists() {
            return Ok(None);
        }
        let conn = connection::open_read_only(db_path, connection::READER_BUSY_TIMEOUT_MS)?;
        let on_disk: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(StoreError::from)?;
        let expected = migrate::catalog_set().current_version();
        if on_disk > expected {
            return Err(CatalogError::Unavailable(StoreError::SchemaTooNew {
                path: db_path.to_path_buf(),
                found: on_disk,
                expected,
            }));
        }
        Ok(Some(Self {
            inner: Inner::Ro(conn),
        }))
    }

    fn read<T>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> crate::store::Result<T>,
    ) -> Result<T> {
        match &self.inner {
            Inner::Rw(pool) => {
                let conn = pool.reader()?;
                Ok(f(&conn)?)
            }
            Inner::Ro(conn) => Ok(f(conn)?),
        }
    }

    fn write<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> crate::store::Result<T>,
    ) -> Result<T> {
        match &self.inner {
            Inner::Rw(pool) => {
                let mut conn = pool.writer()?;
                Ok(with_write_txn(&mut conn, f)?)
            }
            Inner::Ro(_) => unreachable!(
                "catalog write on a read-only handle — open_rw() is the only path to writes"
            ),
        }
    }

    /// Register (or refresh) a repo. Known uuids keep their name, get their
    /// facts refreshed, and resurrect if removed. New uuids retire any
    /// previous live entry at the same path (re-clone rule) and enter under
    /// `default_name`, auto-suffixed when a live entry already claims it.
    pub fn register(&self, facts: &RegistrationFacts) -> Result<RegistrationOutcome> {
        let now = Utc::now();
        let normalized = facts.remote_url.as_deref().map(normalize::normalize_url);
        self.write(|tx| {
            CatalogReposRepo::retire_live_at_path(tx, &facts.path, &facts.uuid, now)?;
            if let Some(existing) = CatalogReposRepo::get_by_uuid(tx, &facts.uuid)? {
                CatalogReposRepo::update_registration(
                    tx,
                    &facts.uuid,
                    &facts.path,
                    &facts.git_common_dir,
                    facts.remote_url.as_deref(),
                    normalized.as_deref(),
                    facts.default_branch.as_deref(),
                    now,
                )?;
                return Ok(RegistrationOutcome {
                    assigned_name: existing.name,
                    suffixed: false,
                    resurrected: existing.removed_at.is_some(),
                    created: false,
                });
            }

            let name = normalize::suffixed_name(&facts.default_name, |candidate| {
                CatalogReposRepo::live_name_exists(tx, candidate).unwrap_or(true)
            });
            let suffixed = name != facts.default_name;
            CatalogReposRepo::insert(
                tx,
                &CatalogRepoRow {
                    uuid: facts.uuid.clone(),
                    name: name.clone(),
                    path: facts.path.clone(),
                    git_common_dir: facts.git_common_dir.clone(),
                    remote_url: facts.remote_url.clone(),
                    remote_url_normalized: normalized.clone(),
                    default_branch: facts.default_branch.clone(),
                    created_at: now,
                    updated_at: now,
                    removed_at: None,
                },
            )?;
            Ok(RegistrationOutcome {
                assigned_name: name,
                suffixed,
                resurrected: false,
                created: true,
            })
        })
    }

    /// Resolve a user-supplied needle. Precedence: live name → uuid →
    /// canonical path / git-common-dir → removed name. Returns `None` when
    /// nothing matches (callers decide between silent fallthrough and
    /// [`Catalog::not_found`]).
    pub fn resolve(&self, needle: &str) -> Result<Option<CatalogRepoRow>> {
        self.read(|conn| {
            if let Some(row) = CatalogReposRepo::find_live_by_name(conn, needle)? {
                return Ok(Some(row));
            }
            if uuid::Uuid::parse_str(needle).is_ok()
                && let Some(row) = CatalogReposRepo::get_by_uuid(conn, needle)?
            {
                return Ok(Some(row));
            }
            if (needle.contains(std::path::MAIN_SEPARATOR) || needle.starts_with('.'))
                && let Ok(canonical) = Path::new(needle).canonicalize()
            {
                let canonical = canonical.to_string_lossy();
                if let Some(row) = CatalogReposRepo::find_by_path_any(conn, &canonical)? {
                    return Ok(Some(row));
                }
                if let Some(row) = CatalogReposRepo::find_by_git_common_dir(conn, &canonical)? {
                    return Ok(Some(row));
                }
            }
            CatalogReposRepo::find_by_name_any(conn, needle)
        })
    }

    /// Resolve strictly among live entries by name (the `daft go` fallback
    /// path — removed repos and paths must not hijack branch names).
    pub fn resolve_live_name(&self, name: &str) -> Result<Option<CatalogRepoRow>> {
        self.read(|conn| CatalogReposRepo::find_live_by_name(conn, name))
    }

    pub fn get_by_uuid(&self, uuid: &str) -> Result<Option<CatalogRepoRow>> {
        self.read(|conn| CatalogReposRepo::get_by_uuid(conn, uuid))
    }

    pub fn find_live_by_url(&self, url: &str) -> Result<Option<CatalogRepoRow>> {
        let key = normalize::normalize_url(url);
        self.read(|conn| CatalogReposRepo::find_live_by_url_normalized(conn, &key))
    }

    /// Build the `NotFound` error for `needle`, with did-you-mean
    /// suggestions drawn from live names.
    pub fn not_found(&self, needle: &str) -> CatalogError {
        let names = self.live_names().unwrap_or_default();
        let suggestions = crate::suggest::find_similar(needle, &names, 3)
            .into_iter()
            .map(str::to_string)
            .collect();
        CatalogError::NotFound {
            needle: needle.to_string(),
            suggestions,
        }
    }

    /// Rename the entry at `uuid`. Errors with [`CatalogError::NameTaken`]
    /// when a *different* live entry already claims the name.
    pub fn rename(&self, uuid: &str, new_name: &str) -> Result<()> {
        if let Err(reason) = normalize::validate_catalog_name(new_name) {
            return Err(CatalogError::InvalidName {
                name: new_name.to_string(),
                reason,
            });
        }
        let claimed = self.read(|conn| CatalogReposRepo::find_live_by_name(conn, new_name))?;
        if let Some(other) = claimed
            && other.uuid != uuid
        {
            return Err(CatalogError::NameTaken {
                name: new_name.to_string(),
                path: other.path,
            });
        }
        self.write(|tx| CatalogReposRepo::rename(tx, uuid, new_name, Utc::now()))
    }

    pub fn mark_removed(&self, uuid: &str) -> Result<()> {
        self.write(|tx| CatalogReposRepo::mark_removed(tx, uuid, Utc::now()))
    }

    pub fn refresh_default_branch(&self, uuid: &str, default_branch: &str) -> Result<()> {
        self.write(|tx| CatalogReposRepo::set_default_branch(tx, uuid, default_branch, Utc::now()))
    }

    pub fn list(&self, include_removed: bool) -> Result<Vec<CatalogRepoRow>> {
        self.read(|conn| {
            if include_removed {
                CatalogReposRepo::list_all(conn)
            } else {
                CatalogReposRepo::list_live(conn)
            }
        })
    }

    pub fn live_names(&self) -> Result<Vec<String>> {
        Ok(self.list(false)?.into_iter().map(|row| row.name).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn catalog(tmp: &TempDir) -> Catalog {
        let db = paths::catalog_db_under(tmp.path()).unwrap();
        Catalog::open_rw_at(&db).unwrap()
    }

    fn facts(uuid: &str, name: &str, path: &str) -> RegistrationFacts {
        RegistrationFacts {
            uuid: uuid.into(),
            default_name: name.into(),
            path: path.into(),
            git_common_dir: format!("{path}/.git"),
            remote_url: Some(format!("git@example.com:org/{name}.git")),
            default_branch: Some("main".into()),
        }
    }

    #[test]
    fn register_then_resolve_by_name_uuid_and_path() {
        let tmp = TempDir::new().unwrap();
        let cat = catalog(&tmp);
        let out = cat.register(&facts("u1", "api", "/w/api")).unwrap();
        assert_eq!(out.assigned_name, "api");
        assert!(out.created && !out.suffixed && !out.resurrected);

        assert_eq!(cat.resolve("api").unwrap().unwrap().uuid, "u1");
        assert_eq!(cat.resolve("u1").unwrap(), None, "non-uuid needle misses");
        let real_uuid = uuid::Uuid::now_v7().to_string();
        cat.register(&facts(&real_uuid, "client", "/w/client"))
            .unwrap();
        assert_eq!(
            cat.resolve(&real_uuid).unwrap().unwrap().name,
            "client",
            "uuid needles resolve"
        );
    }

    #[test]
    fn name_collision_suffixes_and_reports() {
        let tmp = TempDir::new().unwrap();
        let cat = catalog(&tmp);
        cat.register(&facts("u1", "api", "/w/one")).unwrap();
        let out = cat.register(&facts("u2", "api", "/w/two")).unwrap();
        assert_eq!(out.assigned_name, "api-2");
        assert!(out.suffixed);
        let out3 = cat.register(&facts("u3", "api", "/w/three")).unwrap();
        assert_eq!(out3.assigned_name, "api-3");
    }

    #[test]
    fn reregistering_known_uuid_keeps_name_and_resurrects() {
        let tmp = TempDir::new().unwrap();
        let cat = catalog(&tmp);
        cat.register(&facts("u1", "api", "/w/api")).unwrap();
        cat.rename("u1", "my-api").unwrap();
        cat.mark_removed("u1").unwrap();

        let out = cat.register(&facts("u1", "api", "/moved/api")).unwrap();
        assert_eq!(out.assigned_name, "my-api", "user-chosen name survives");
        assert!(out.resurrected && !out.created);
        let row = cat.get_by_uuid("u1").unwrap().unwrap();
        assert_eq!(row.path, "/moved/api");
        assert!(row.removed_at.is_none());
    }

    #[test]
    fn reclone_at_same_path_retires_previous_uuid() {
        let tmp = TempDir::new().unwrap();
        let cat = catalog(&tmp);
        cat.register(&facts("u-old", "api", "/w/api")).unwrap();
        let out = cat.register(&facts("u-new", "api", "/w/api")).unwrap();
        // Old uuid retired → its live name freed; new entry takes it over.
        assert_eq!(out.assigned_name, "api");
        assert!(!out.suffixed);
        assert!(
            cat.get_by_uuid("u-old")
                .unwrap()
                .unwrap()
                .removed_at
                .is_some()
        );
        assert_eq!(cat.resolve("api").unwrap().unwrap().uuid, "u-new");
    }

    #[test]
    fn rename_collision_errors_with_name_taken() {
        let tmp = TempDir::new().unwrap();
        let cat = catalog(&tmp);
        cat.register(&facts("u1", "api", "/w/api")).unwrap();
        cat.register(&facts("u2", "client", "/w/client")).unwrap();
        let err = cat.rename("u2", "api").unwrap_err();
        assert!(matches!(err, CatalogError::NameTaken { .. }));
        cat.rename("u1", "api")
            .expect("self-rename is a no-op success");
        let err = cat.rename("u2", "-bad").unwrap_err();
        assert!(matches!(err, CatalogError::InvalidName { .. }));
    }

    #[test]
    fn open_ro_never_creates_and_reads_live_data() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("catalog").join("catalog.db");

        assert!(Catalog::open_ro_at(&db).unwrap().is_none());
        assert!(!db.exists(), "read-only probe must not create the file");

        let rw = catalog(&tmp);
        rw.register(&facts("u1", "api", "/w/api")).unwrap();
        let ro = Catalog::open_ro_at(&db)
            .unwrap()
            .expect("catalog exists now");
        assert_eq!(ro.resolve("api").unwrap().unwrap().uuid, "u1");
    }

    #[test]
    fn open_ro_rejects_newer_schema() {
        let tmp = TempDir::new().unwrap();
        let db = paths::catalog_db_under(tmp.path()).unwrap();
        drop(Catalog::open_rw_at(&db).unwrap());
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        }
        let err = Catalog::open_ro_at(&db).unwrap_err();
        assert!(matches!(
            err,
            CatalogError::Unavailable(StoreError::SchemaTooNew { .. })
        ));
    }

    #[test]
    fn not_found_suggests_close_names() {
        let tmp = TempDir::new().unwrap();
        let cat = catalog(&tmp);
        cat.register(&facts("u1", "api-client", "/w/api-client"))
            .unwrap();
        let err = cat.not_found("api-clien");
        match err {
            CatalogError::NotFound { suggestions, .. } => {
                assert_eq!(suggestions, vec!["api-client".to_string()]);
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
