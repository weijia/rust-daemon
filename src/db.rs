//! Local SQLite storage under `~/.my-tray-app/db.sqlite`.
//!
//! The connection is owned exclusively by the main thread, so every DB write
//! happens exactly once and there is no cross-thread contention.

use crate::commands::TaskRow;
use rusqlite::Connection;
use std::path::PathBuf;

/// Resolve `~/.my-tray-app/db.sqlite`, creating the directory if needed.
pub fn db_path() -> PathBuf {
    let mut dir = dirs::home_dir().expect("HOME directory is required");
    dir.push(".my-tray-app");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("failed to create {}: {e}", dir.display());
    }
    dir.push("db.sqlite");
    dir
}

#[derive(Debug)]
pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open() -> rusqlite::Result<Self> {
        let conn = Connection::open(db_path())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT NOT NULL,
                created_at  TEXT NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    pub fn insert_task(&self, name: &str) -> rusqlite::Result<()> {
        let created_at = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute("INSERT INTO tasks (name, created_at) VALUES (?1, ?2)", (name, created_at))?;
        Ok(())
    }

    pub fn count_tasks(&self) -> rusqlite::Result<usize> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    pub fn list_tasks(&self) -> rusqlite::Result<Vec<TaskRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, created_at FROM tasks ORDER BY id DESC LIMIT 100")?;
        let rows = stmt.query_map([], |r| {
            Ok(TaskRow {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    }
}
