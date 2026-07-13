//! [`ProfileStore`] over the per-repo coordinator database.
//!
//! Opens the same `coordinator.db` the hooks/jobs subsystem uses (pool +
//! `bring_up` security defaults come for free) and delegates to the
//! `hook_profiles` / `governor_events` repos. Every method is best-effort
//! per the port contract: a storage failure degrades the governor to
//! cold-start behavior and is deliberately silent — during a sync push the
//! terminal belongs to the TUI.

use crate::governor::ports::{ProfileKey, ProfileStore};
use crate::store::models::{GovernorEventRow, HookProfileRow};
use crate::store::pool::Pool;
use crate::store::repos::{GovernorEventsRepo, HookProfilesRepo};

pub struct SqliteProfileStore {
    pool: Pool,
}

impl SqliteProfileStore {
    /// Open (creating/migrating if needed) the coordinator DB for a repo.
    pub fn open_for_repo(repo_hash: &str) -> Option<Self> {
        let db_path = crate::store::paths::for_repo(repo_hash).ok()?;
        let pool = Pool::open(&db_path).ok()?;
        Some(Self { pool })
    }
}

impl ProfileStore for SqliteProfileStore {
    fn load(&self, key: &ProfileKey) -> Option<HookProfileRow> {
        let conn = self.pool.reader().ok()?;
        HookProfilesRepo::get(&conn, &key.repo_hash, &key.stage, &key.hook_hash)
            .ok()
            .flatten()
    }

    fn save(&self, row: &HookProfileRow) {
        if let Ok(conn) = self.pool.writer() {
            let _ = HookProfilesRepo::upsert(&conn, row);
        }
    }

    fn record_events(&self, events: &[GovernorEventRow]) {
        if events.is_empty() {
            return;
        }
        if let Ok(conn) = self.pool.writer() {
            for event in events {
                let _ = GovernorEventsRepo::insert(&conn, event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serial_test::serial;

    #[test]
    #[serial]
    fn profile_roundtrip_through_real_pool() {
        // Never touch the developer's real state dir (#697 tripwire).
        let _isolated = crate::store::paths::IsolatedStateDir::new();
        let store = SqliteProfileStore::open_for_repo("governor-test-repo")
            .expect("pool opens in isolated state dir");
        let key = ProfileKey {
            repo_hash: "governor-test-repo".into(),
            stage: "pre-push".into(),
            hook_hash: "abc123".into(),
        };
        assert!(store.load(&key).is_none(), "no profile before first save");

        let row = HookProfileRow {
            repo_hash: key.repo_hash.clone(),
            stage: key.stage.clone(),
            hook_hash: key.hook_hash.clone(),
            peak_rss_bytes: 6 << 30,
            wall_ms: 240_000,
            runs: 1,
            updated_at: Utc::now(),
        };
        store.save(&row);
        let loaded = store.load(&key).expect("profile persisted");
        assert_eq!(loaded.peak_rss_bytes, 6 << 30);
        assert_eq!(loaded.wall_ms, 240_000);
        assert_eq!(loaded.runs, 1);

        // Upsert replaces.
        store.save(&HookProfileRow {
            peak_rss_bytes: 5 << 30,
            runs: 2,
            ..row.clone()
        });
        let updated = store.load(&key).expect("profile still there");
        assert_eq!(updated.peak_rss_bytes, 5 << 30);
        assert_eq!(updated.runs, 2);

        // A different hook hash is a different profile.
        assert!(
            store
                .load(&ProfileKey {
                    hook_hash: "other".into(),
                    ..key.clone()
                })
                .is_none()
        );

        // Events append without erroring (best-effort surface).
        store.record_events(&[GovernorEventRow {
            id: None,
            repo_hash: key.repo_hash.clone(),
            occurred_at: Utc::now(),
            kind: "throttle".into(),
            branch: Some("feat/a".into()),
            detail_ms: Some(1_400),
            rss_bytes: None,
        }]);
        let conn = store.pool.reader().unwrap();
        let events = GovernorEventsRepo::list_by_repo(&conn, &key.repo_hash).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "throttle");
        assert_eq!(events[0].detail_ms, Some(1_400));
    }
}
