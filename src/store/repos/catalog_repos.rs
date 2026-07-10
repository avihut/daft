//! Query layer for the `catalog_repos` table (global repo catalog).
//!
//! Lookup semantics baked into the queries:
//!
//! * "Live wins": `find_by_name_any` orders live rows before removed ones,
//!   so a name that was removed and later re-registered resolves to the
//!   live entry.
//! * `update_registration` refreshes location/remote facts and clears
//!   `removed_at` (a re-registered repo resurrects) but **never touches
//!   `name`** — implicit registration must not clobber a user-chosen name.
//! * `retire_live_at_path` implements the re-clone-at-same-path rule: the
//!   previous uuid at that path is marked removed, the new one stays live.

use crate::store::error::Result;
use crate::store::models::CatalogRepoRow;
use crate::store::repos::invocations::parse_rfc3339;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

pub struct CatalogReposRepo;

/// Shared SELECT head. `concat!`-ed into each query so the full SQL stays a
/// compile-time literal (the repo layer bans runtime query building).
macro_rules! select_catalog_repos {
    ($tail:literal) => {
        concat!(
            "SELECT uuid, name, path, git_common_dir, remote_url, \
             remote_url_normalized, default_branch, created_at, updated_at, \
             removed_at FROM catalog_repos ",
            $tail
        )
    };
}

impl CatalogReposRepo {
    pub fn insert(conn: &Connection, row: &CatalogRepoRow) -> Result<()> {
        conn.execute(
            "INSERT INTO catalog_repos
                 (uuid, name, path, git_common_dir, remote_url,
                  remote_url_normalized, default_branch, created_at,
                  updated_at, removed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                row.uuid,
                row.name,
                row.path,
                row.git_common_dir,
                row.remote_url,
                row.remote_url_normalized,
                row.default_branch,
                row.created_at.to_rfc3339(),
                row.updated_at.to_rfc3339(),
                row.removed_at.map(|t| t.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    /// Refresh the facts of an existing entry and resurrect it if removed.
    /// Deliberately does not update `name`.
    #[allow(clippy::too_many_arguments)]
    pub fn update_registration(
        conn: &Connection,
        uuid: &str,
        path: &str,
        git_common_dir: &str,
        remote_url: Option<&str>,
        remote_url_normalized: Option<&str>,
        default_branch: Option<&str>,
        updated_at: DateTime<Utc>,
    ) -> Result<()> {
        conn.execute(
            "UPDATE catalog_repos
                SET path = ?2, git_common_dir = ?3, remote_url = ?4,
                    remote_url_normalized = ?5, default_branch = ?6,
                    updated_at = ?7, removed_at = NULL
              WHERE uuid = ?1",
            params![
                uuid,
                path,
                git_common_dir,
                remote_url,
                remote_url_normalized,
                default_branch,
                updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn rename(
        conn: &Connection,
        uuid: &str,
        new_name: &str,
        updated_at: DateTime<Utc>,
    ) -> Result<()> {
        conn.execute(
            "UPDATE catalog_repos SET name = ?2, updated_at = ?3 WHERE uuid = ?1",
            params![uuid, new_name, updated_at.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn set_default_branch(
        conn: &Connection,
        uuid: &str,
        default_branch: &str,
        updated_at: DateTime<Utc>,
    ) -> Result<()> {
        conn.execute(
            "UPDATE catalog_repos SET default_branch = ?2, updated_at = ?3 WHERE uuid = ?1",
            params![uuid, default_branch, updated_at.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn mark_removed(conn: &Connection, uuid: &str, removed_at: DateTime<Utc>) -> Result<()> {
        conn.execute(
            "UPDATE catalog_repos SET removed_at = ?2, updated_at = ?2 WHERE uuid = ?1",
            params![uuid, removed_at.to_rfc3339()],
        )?;
        Ok(())
    }

    /// Mark every live entry at `path` removed except `keep_uuid`. Returns
    /// the number of retired rows.
    pub fn retire_live_at_path(
        conn: &Connection,
        path: &str,
        keep_uuid: &str,
        removed_at: DateTime<Utc>,
    ) -> Result<usize> {
        let n = conn.execute(
            "UPDATE catalog_repos SET removed_at = ?3, updated_at = ?3
              WHERE path = ?1 AND uuid != ?2 AND removed_at IS NULL",
            params![path, keep_uuid, removed_at.to_rfc3339()],
        )?;
        Ok(n)
    }

    pub fn get_by_uuid(conn: &Connection, uuid: &str) -> Result<Option<CatalogRepoRow>> {
        let mut stmt = conn.prepare(select_catalog_repos!("WHERE uuid = ?1"))?;
        Ok(stmt.query_row(params![uuid], read_row).optional()?)
    }

    pub fn find_live_by_name(conn: &Connection, name: &str) -> Result<Option<CatalogRepoRow>> {
        let mut stmt = conn.prepare(select_catalog_repos!(
            "WHERE name = ?1 AND removed_at IS NULL"
        ))?;
        Ok(stmt.query_row(params![name], read_row).optional()?)
    }

    /// Name lookup across live and removed entries; live rows win, most
    /// recently updated removed row breaks ties.
    pub fn find_by_name_any(conn: &Connection, name: &str) -> Result<Option<CatalogRepoRow>> {
        let mut stmt = conn.prepare(select_catalog_repos!(
            "WHERE name = ?1
             ORDER BY (removed_at IS NULL) DESC, updated_at DESC
             LIMIT 1"
        ))?;
        Ok(stmt.query_row(params![name], read_row).optional()?)
    }

    pub fn find_live_by_path(conn: &Connection, path: &str) -> Result<Option<CatalogRepoRow>> {
        let mut stmt = conn.prepare(select_catalog_repos!(
            "WHERE path = ?1 AND removed_at IS NULL"
        ))?;
        Ok(stmt.query_row(params![path], read_row).optional()?)
    }

    /// Path lookup across live and removed entries; live-first.
    pub fn find_by_path_any(conn: &Connection, path: &str) -> Result<Option<CatalogRepoRow>> {
        let mut stmt = conn.prepare(select_catalog_repos!(
            "WHERE path = ?1
             ORDER BY (removed_at IS NULL) DESC, updated_at DESC
             LIMIT 1"
        ))?;
        Ok(stmt.query_row(params![path], read_row).optional()?)
    }

    pub fn find_by_git_common_dir(
        conn: &Connection,
        git_common_dir: &str,
    ) -> Result<Option<CatalogRepoRow>> {
        let mut stmt = conn.prepare(select_catalog_repos!(
            "WHERE git_common_dir = ?1
             ORDER BY (removed_at IS NULL) DESC, updated_at DESC
             LIMIT 1"
        ))?;
        Ok(stmt
            .query_row(params![git_common_dir], read_row)
            .optional()?)
    }

    pub fn find_live_by_url_normalized(
        conn: &Connection,
        url_normalized: &str,
    ) -> Result<Option<CatalogRepoRow>> {
        let mut stmt = conn.prepare(select_catalog_repos!(
            "WHERE remote_url_normalized = ?1 AND removed_at IS NULL
             ORDER BY updated_at DESC
             LIMIT 1"
        ))?;
        Ok(stmt
            .query_row(params![url_normalized], read_row)
            .optional()?)
    }

    pub fn live_name_exists(conn: &Connection, name: &str) -> Result<bool> {
        let mut stmt =
            conn.prepare("SELECT 1 FROM catalog_repos WHERE name = ?1 AND removed_at IS NULL")?;
        Ok(stmt.exists(params![name])?)
    }

    pub fn list_live(conn: &Connection) -> Result<Vec<CatalogRepoRow>> {
        let mut stmt = conn.prepare(select_catalog_repos!(
            "WHERE removed_at IS NULL ORDER BY name ASC"
        ))?;
        collect_rows(&mut stmt, params![])
    }

    pub fn list_all(conn: &Connection) -> Result<Vec<CatalogRepoRow>> {
        let mut stmt = conn.prepare(select_catalog_repos!(
            "ORDER BY (removed_at IS NULL) DESC, name ASC"
        ))?;
        collect_rows(&mut stmt, params![])
    }
}

fn collect_rows(
    stmt: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<CatalogRepoRow>> {
    let rows = stmt.query_map(params, read_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogRepoRow> {
    let created_at_str: String = row.get(7)?;
    let updated_at_str: String = row.get(8)?;
    let removed_at_str: Option<String> = row.get(9)?;
    Ok(CatalogRepoRow {
        uuid: row.get(0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        git_common_dir: row.get(3)?,
        remote_url: row.get(4)?,
        remote_url_normalized: row.get(5)?,
        default_branch: row.get(6)?,
        created_at: parse_rfc3339(&created_at_str, "created_at")?,
        updated_at: parse_rfc3339(&updated_at_str, "updated_at")?,
        removed_at: removed_at_str
            .map(|s| parse_rfc3339(&s, "removed_at"))
            .transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{connection, migrate};
    use tempfile::TempDir;

    fn catalog_conn() -> (TempDir, Connection) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let is_fresh = !path.exists();
        let mut conn = Connection::open(&path).unwrap();
        connection::bring_up(
            &mut conn,
            &path,
            connection::WRITER_BUSY_TIMEOUT_MS,
            is_fresh,
            /* read_only */ false,
        )
        .unwrap();
        migrate::run_set(&migrate::catalog_set(), &mut conn, &path).unwrap();
        (tmp, conn)
    }

    fn row(uuid: &str, name: &str, path: &str) -> CatalogRepoRow {
        let now = chrono::Utc::now();
        CatalogRepoRow {
            uuid: uuid.into(),
            name: name.into(),
            path: path.into(),
            git_common_dir: [path, "/.git"].concat(),
            remote_url: Some(["git@example.com:org/", name, ".git"].concat()),
            remote_url_normalized: Some(["example.com/org/", name].concat()),
            default_branch: Some("main".into()),
            created_at: now,
            updated_at: now,
            removed_at: None,
        }
    }

    #[test]
    fn insert_and_get_roundtrip() {
        let (_tmp, conn) = catalog_conn();
        let r = row("u1", "api", "/w/api");
        CatalogReposRepo::insert(&conn, &r).unwrap();
        let got = CatalogReposRepo::get_by_uuid(&conn, "u1").unwrap().unwrap();
        assert_eq!(got.name, "api");
        assert_eq!(got.path, "/w/api");
        assert_eq!(got.git_common_dir, "/w/api/.git");
        assert_eq!(
            got.remote_url.as_deref(),
            Some("git@example.com:org/api.git")
        );
        assert_eq!(
            got.remote_url_normalized.as_deref(),
            Some("example.com/org/api")
        );
        assert_eq!(got.default_branch.as_deref(), Some("main"));
        assert!(got.removed_at.is_none());
    }

    #[test]
    fn update_registration_refreshes_facts_resurrects_and_keeps_name() {
        let (_tmp, conn) = catalog_conn();
        let r = row("u1", "custom-name", "/w/api");
        CatalogReposRepo::insert(&conn, &r).unwrap();
        CatalogReposRepo::mark_removed(&conn, "u1", chrono::Utc::now()).unwrap();

        CatalogReposRepo::update_registration(
            &conn,
            "u1",
            "/moved/api",
            "/moved/api/.git",
            Some("https://example.com/org/api"),
            Some("example.com/org/api"),
            Some("trunk"),
            chrono::Utc::now(),
        )
        .unwrap();

        let got = CatalogReposRepo::get_by_uuid(&conn, "u1").unwrap().unwrap();
        assert_eq!(
            got.name, "custom-name",
            "registration must not clobber name"
        );
        assert_eq!(got.path, "/moved/api");
        assert_eq!(got.default_branch.as_deref(), Some("trunk"));
        assert!(got.removed_at.is_none(), "re-registration resurrects");
    }

    #[test]
    fn live_wins_over_removed_on_name_lookup() {
        let (_tmp, conn) = catalog_conn();
        let mut old = row("u-old", "api", "/w/api");
        old.removed_at = Some(chrono::Utc::now());
        CatalogReposRepo::insert(&conn, &old).unwrap();
        CatalogReposRepo::insert(&conn, &row("u-new", "api", "/w/api")).unwrap();

        let live = CatalogReposRepo::find_live_by_name(&conn, "api")
            .unwrap()
            .unwrap();
        assert_eq!(live.uuid, "u-new");
        let any = CatalogReposRepo::find_by_name_any(&conn, "api")
            .unwrap()
            .unwrap();
        assert_eq!(any.uuid, "u-new", "live entry wins");

        CatalogReposRepo::mark_removed(&conn, "u-new", chrono::Utc::now()).unwrap();
        assert!(
            CatalogReposRepo::find_live_by_name(&conn, "api")
                .unwrap()
                .is_none()
        );
        assert!(
            CatalogReposRepo::find_by_name_any(&conn, "api")
                .unwrap()
                .is_some(),
            "removed entries stay addressable by name"
        );
    }

    #[test]
    fn retire_live_at_path_keeps_the_new_uuid() {
        let (_tmp, conn) = catalog_conn();
        CatalogReposRepo::insert(&conn, &row("u-old", "api", "/w/api")).unwrap();
        // Re-clone at the same path: new identity arrives...
        let retired =
            CatalogReposRepo::retire_live_at_path(&conn, "/w/api", "u-new", chrono::Utc::now())
                .unwrap();
        assert_eq!(retired, 1);
        CatalogReposRepo::insert(&conn, &row("u-new", "api", "/w/api")).unwrap();

        let live = CatalogReposRepo::find_live_by_path(&conn, "/w/api")
            .unwrap()
            .unwrap();
        assert_eq!(live.uuid, "u-new");
        let old = CatalogReposRepo::get_by_uuid(&conn, "u-old")
            .unwrap()
            .unwrap();
        assert!(
            old.removed_at.is_some(),
            "previous uuid retired, not deleted"
        );
    }

    #[test]
    fn live_name_unique_index_rejects_duplicates_but_allows_removed() {
        let (_tmp, conn) = catalog_conn();
        CatalogReposRepo::insert(&conn, &row("u1", "api", "/w/one")).unwrap();
        let err = CatalogReposRepo::insert(&conn, &row("u2", "api", "/w/two"));
        assert!(
            err.is_err(),
            "second live 'api' must violate the partial index"
        );

        let mut removed = row("u3", "api", "/w/three");
        removed.removed_at = Some(chrono::Utc::now());
        CatalogReposRepo::insert(&conn, &removed).expect("removed duplicate names are allowed");
    }

    #[test]
    fn live_path_unique_index_rejects_second_live_entry() {
        let (_tmp, conn) = catalog_conn();
        CatalogReposRepo::insert(&conn, &row("u1", "api", "/w/api")).unwrap();
        let err = CatalogReposRepo::insert(&conn, &row("u2", "api-2", "/w/api"));
        assert!(err.is_err(), "one live entry per path");
    }

    #[test]
    fn list_live_sorted_and_list_all_includes_removed() {
        let (_tmp, conn) = catalog_conn();
        CatalogReposRepo::insert(&conn, &row("u1", "zeta", "/w/zeta")).unwrap();
        CatalogReposRepo::insert(&conn, &row("u2", "alpha", "/w/alpha")).unwrap();
        let mut removed = row("u3", "gone", "/w/gone");
        removed.removed_at = Some(chrono::Utc::now());
        CatalogReposRepo::insert(&conn, &removed).unwrap();

        let live = CatalogReposRepo::list_live(&conn).unwrap();
        assert_eq!(
            live.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        let all = CatalogReposRepo::list_all(&conn).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all.last().unwrap().name, "gone", "removed rows sort last");
    }

    #[test]
    fn url_lookup_matches_live_only() {
        let (_tmp, conn) = catalog_conn();
        let mut removed = row("u1", "api", "/w/old");
        removed.removed_at = Some(chrono::Utc::now());
        CatalogReposRepo::insert(&conn, &removed).unwrap();
        assert!(
            CatalogReposRepo::find_live_by_url_normalized(&conn, "example.com/org/api")
                .unwrap()
                .is_none()
        );
        CatalogReposRepo::insert(&conn, &row("u2", "api", "/w/api")).unwrap();
        let hit = CatalogReposRepo::find_live_by_url_normalized(&conn, "example.com/org/api")
            .unwrap()
            .unwrap();
        assert_eq!(hit.uuid, "u2");
    }
}
