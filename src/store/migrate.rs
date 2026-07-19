//! Schema migration runner.
//!
//! Migrations are checked-in `.sql` files in `src/store/migrations/`,
//! numbered `NNN_<name>.sql`. Each file is one logical schema upgrade and
//! runs in its own transaction. The on-disk schema version is stored in
//! SQLite's `user_version` PRAGMA so we don't need a sidecar table.
//!
//! Daft has more than one database, each with its own **migration lineage**
//! bundled as a [`MigrationSet`]: the per-repo coordinator store
//! (`migrations/NNN_*.sql`) and the global repo catalog
//! (`migrations/catalog/NNN_*.sql`). `user_version` lives per file, so the
//! lineages evolve independently.
//!
//! Two safety rules per lineage:
//!   1. The migration vector is monotonically appended to — new versions
//!      only, never reorder, never edit a shipped migration in place.
//!   2. Opening a DB whose `user_version` is *higher* than the lineage's
//!      highest migration is rejected: a newer daft wrote data this binary
//!      doesn't understand.

use crate::store::error::{Result, StoreError};
use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};
use std::path::Path;

/// One database's migration lineage: the ordered migrations plus the
/// version a fully-migrated file lands on.
pub struct MigrationSet {
    migrations: Migrations<'static>,
    current_version: i64,
}

impl MigrationSet {
    /// Highest schema version this binary understands for the lineage.
    pub fn current_version(&self) -> i64 {
        self.current_version
    }

    #[cfg(test)]
    pub(crate) fn validate(&self) -> std::result::Result<(), rusqlite_migration::Error> {
        self.migrations.validate()
    }
}

/// The per-repo coordinator store lineage (`jobs/<uuid>/coordinator.db`).
pub fn coordinator_set() -> MigrationSet {
    MigrationSet {
        migrations: Migrations::new(vec![
            M::up(include_str!("migrations/001_initial.sql")),
            M::up(include_str!("migrations/002_visitor_seeds.sql")),
            M::up(include_str!("migrations/003_invocation_status.sql")),
            M::up(include_str!("migrations/004_hook_profiles.sql")),
            M::up(include_str!("migrations/005_worktree_sizes.sql")),
            M::up(include_str!("migrations/006_forge_prs.sql")),
            M::up(include_str!("migrations/007_forge_health.sql")),
            M::up(include_str!("migrations/008_forge_pr_row_fields.sql")),
            M::up(include_str!("migrations/009_worktree_identities.sql")),
        ]),
        // rusqlite_migration's version counter is `migrations.len() as u32`
        // after every migration is applied. Kept as i64 for consistency with
        // the on-disk `user_version` PRAGMA type.
        current_version: 9,
    }
}

/// The global repo-catalog lineage (`catalog/catalog.db`).
pub fn catalog_set() -> MigrationSet {
    MigrationSet {
        migrations: Migrations::new(vec![
            M::up(include_str!("migrations/catalog/001_catalog.sql")),
            M::up(include_str!("migrations/catalog/002_repo_sizes.sql")),
        ]),
        current_version: 2,
    }
}

/// Highest coordinator-store schema version this binary understands.
/// Side-effect free so the open-time refuse-newer check can call it.
pub fn current_version() -> i64 {
    coordinator_set().current_version
}

/// Apply all pending migrations for `set` against `conn`. Refuses if the
/// on-disk `user_version` is *higher* than the lineage's current version.
pub fn run_set(set: &MigrationSet, conn: &mut Connection, db_path: &Path) -> Result<()> {
    let on_disk: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if on_disk > set.current_version {
        return Err(StoreError::SchemaTooNew {
            path: db_path.to_path_buf(),
            found: on_disk,
            expected: set.current_version,
        });
    }
    set.migrations.to_latest(conn)?;
    Ok(())
}

/// Apply all pending coordinator-store migrations against `conn`.
pub fn run(conn: &mut Connection, db_path: &Path) -> Result<()> {
    run_set(&coordinator_set(), conn, db_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection;
    use tempfile::TempDir;

    /// Fresh connection with PRAGMA bring-up but no auto-migration, so each
    /// test picks which lineage to apply.
    fn open_unmigrated(path: &Path) -> Connection {
        let is_fresh = !path.exists();
        let mut conn = Connection::open(path).unwrap();
        connection::bring_up(
            &mut conn,
            path,
            connection::WRITER_BUSY_TIMEOUT_MS,
            is_fresh,
            /* read_only */ false,
        )
        .unwrap();
        conn
    }

    #[test]
    fn migrations_validate() {
        coordinator_set()
            .validate()
            .expect("coordinator migration set is well-formed");
        catalog_set()
            .validate()
            .expect("catalog migration set is well-formed");
    }

    #[test]
    fn fresh_db_lands_at_current_version() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, current_version());
    }

    #[test]
    fn fresh_catalog_db_lands_at_catalog_version() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let mut conn = open_unmigrated(&path);
        run_set(&catalog_set(), &mut conn, &path).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, catalog_set().current_version());
    }

    #[test]
    fn catalog_migration_creates_catalog_repos_table() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let mut conn = open_unmigrated(&path);
        run_set(&catalog_set(), &mut conn, &path).unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'catalog_repos'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "catalog_repos");
        // Coordinator tables must NOT leak into the catalog lineage.
        let jobs: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'jobs'",
                [],
                |r| r.get(0),
            )
            .ok();
        assert_eq!(jobs, None);
    }

    #[test]
    fn catalog_migration_creates_repo_sizes_table() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let mut conn = open_unmigrated(&path);
        run_set(&catalog_set(), &mut conn, &path).unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'repo_sizes'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "repo_sizes");
    }

    #[test]
    fn refuses_open_when_user_version_is_newer() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        // Pretend a future daft wrote here.
        conn.execute_batch(&format!(
            "PRAGMA user_version = {};",
            current_version() + 99
        ))
        .unwrap();
        let err = run(&mut conn, &path).unwrap_err();
        assert!(matches!(err, StoreError::SchemaTooNew { .. }));
    }

    #[test]
    fn schema_too_new_is_per_lineage() {
        // A catalog DB one version past the catalog lineage must be refused
        // by the catalog set even though the coordinator lineage is higher.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let mut conn = open_unmigrated(&path);
        run_set(&catalog_set(), &mut conn, &path).unwrap();
        conn.execute_batch(&format!(
            "PRAGMA user_version = {};",
            catalog_set().current_version() + 1
        ))
        .unwrap();
        let err = run_set(&catalog_set(), &mut conn, &path).unwrap_err();
        assert!(matches!(err, StoreError::SchemaTooNew { .. }));
    }

    #[test]
    fn visitor_seeds_table_exists_after_migration() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'visitor_seeds'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "visitor_seeds");
    }

    #[test]
    fn hook_profiles_tables_exist_after_migration() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        for table in ["hook_profiles", "governor_events"] {
            let name: String = conn
                .query_row(
                    "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(name, table);
        }
    }

    #[test]
    fn forge_prs_table_exists_after_migration() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'forge_prs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "forge_prs");
    }

    #[test]
    fn forge_health_table_exists_after_migration() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'forge_health'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "forge_health");
    }

    #[test]
    fn forge_prs_row_fields_exist_after_migration() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        // 008 appends the synthesized-row columns to 006's table.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('forge_prs')
                 WHERE name IN ('head_repo_owner', 'updated_at')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn worktree_identities_table_exists_after_migration() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'worktree_identities'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "worktree_identities");
    }

    #[test]
    fn worktree_sizes_table_exists_after_migration() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'worktree_sizes'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "worktree_sizes");
    }

    #[test]
    fn invocations_table_gains_status_columns() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        conn.execute(
            "INSERT INTO invocations
                 (repo_hash, invocation_id, trigger_command, hook_type, worktree,
                  created_at, status, skip_reason)
             VALUES ('r', 'i', 'checkout', 'worktree-post-create', 'feat/x',
                     '2026-01-01T00:00:00Z', 'skipped', 'untrusted')",
            [],
        )
        .unwrap();
        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, skip_reason FROM invocations WHERE repo_hash = 'r'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "skipped");
        assert_eq!(reason.as_deref(), Some("untrusted"));
    }

    #[test]
    fn runs_pending_migrations_idempotently() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        // Second run is a no-op.
        run(&mut conn, &path).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, current_version());
    }
}
