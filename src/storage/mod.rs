//! SQLite-backed cache for `.desktop` parsing and shared-mime-info
//! descriptions, plus stateless readers for `mimeapps.list`. Schema lives here;
//! parsing/refresh logic lives in the sub-modules.

use std::fs;

use eyre::{Context, Result};
use rusqlite::Connection;

pub mod desktop;
pub mod mime_info;
pub mod mimeapps;

const SCHEMA_VERSION: i32 = 2;

pub struct Storage {
    pub conn: Connection,
}

impl Storage {
    /// Open (or create) the mime-tui sqlite database under XDG_DATA_HOME.
    pub fn open() -> Result<Self> {
        let data_root = dirs::data_dir()
            .ok_or_else(|| eyre::eyre!("could not resolve XDG_DATA_HOME"))?;
        let dir = data_root.join("mime-tui");
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

        let path = dir.join("mime-tui.sqlite");
        let conn = Connection::open(&path)
            .with_context(|| format!("opening {}", path.display()))?;

        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();

        let mut storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        let conn = Connection::open_in_memory().unwrap();
        let mut s = Self { conn };
        s.migrate().unwrap();
        s
    }

    fn migrate(&mut self) -> Result<()> {
        let current: i32 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if current >= SCHEMA_VERSION {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        if current < 1 {
            tx.execute_batch(
                r#"
                CREATE TABLE apps (
                    id              TEXT PRIMARY KEY,
                    name            TEXT NOT NULL,
                    exec            TEXT NOT NULL,
                    terminal        INTEGER NOT NULL,
                    comment         TEXT NOT NULL DEFAULT '',
                    raw_mime_types  TEXT NOT NULL DEFAULT '',
                    source_path     TEXT NOT NULL,
                    file_mtime      INTEGER NOT NULL
                );
                CREATE INDEX apps_source ON apps(source_path);

                CREATE TABLE mime_types (
                    id              TEXT PRIMARY KEY,
                    description     TEXT NOT NULL DEFAULT '',
                    source_path     TEXT NOT NULL DEFAULT '',
                    file_mtime      INTEGER NOT NULL DEFAULT 0
                );

                CREATE TABLE scan_meta (
                    directory       TEXT PRIMARY KEY,
                    dir_mtime       INTEGER NOT NULL,
                    scanned_at      INTEGER NOT NULL
                );
                "#,
            )?;
        }
        if current < 2 {
            // v2 adds a `raw_categories` column on `apps`, used to derive the
            // per-app category icon. Easiest migration is to rebuild the
            // cache — the data is all derivable from .desktop files which
            // are re-walked on next startup by `refresh_app_cache`.
            tx.execute_batch(
                r#"
                DROP TABLE IF EXISTS apps;
                DROP TABLE IF EXISTS scan_meta;
                CREATE TABLE apps (
                    id              TEXT PRIMARY KEY,
                    name            TEXT NOT NULL,
                    exec            TEXT NOT NULL,
                    terminal        INTEGER NOT NULL,
                    comment         TEXT NOT NULL DEFAULT '',
                    raw_mime_types  TEXT NOT NULL DEFAULT '',
                    raw_categories  TEXT NOT NULL DEFAULT '',
                    source_path     TEXT NOT NULL,
                    file_mtime      INTEGER NOT NULL
                );
                CREATE INDEX apps_source ON apps(source_path);
                CREATE TABLE scan_meta (
                    directory       TEXT PRIMARY KEY,
                    dir_mtime       INTEGER NOT NULL,
                    scanned_at      INTEGER NOT NULL
                );
                "#,
            )?;
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_migrates_to_current() {
        let s = Storage::in_memory();
        let v: i32 = s
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }
}
