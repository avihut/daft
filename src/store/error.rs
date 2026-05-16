//! Error type for the store layer.

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("store path is outside daft state dir: {0}")]
    PathOutsideStateDir(PathBuf),

    #[error("store file permissions are too open at {path}: mode 0o{mode:o}")]
    PermissionsTooOpen { path: PathBuf, mode: u32 },

    #[error("{path}: not a daft store (application_id 0x{found:08x} != expected 0x{expected:08x})")]
    AppIdMismatch {
        path: PathBuf,
        found: i32,
        expected: i32,
    },

    #[error("{path}: schema version {found} is newer than this binary's {expected} — upgrade daft")]
    SchemaTooNew {
        path: PathBuf,
        found: i64,
        expected: i64,
    },

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] rusqlite_migration::Error),

    #[error("connection pool error: {0}")]
    Pool(#[from] r2d2::Error),

    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, StoreError>;
