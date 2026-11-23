use std::fs;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::{ItemStatus, Session};

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open sqlite database {}", path.display()))?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                cwd TEXT NOT NULL,
                goal TEXT,
                state_json TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS outcomes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                title TEXT NOT NULL,
                outcome TEXT NOT NULL,
                notes TEXT,
                recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_outcomes_session ON outcomes(session_id);
            "#,
        )?;
        Ok(())
    }

    pub fn save_session(&self, session: &Session) -> Result<()> {
        let json = serde_json::to_string(session)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO sessions (id, cwd, goal, state_json, updated_at)
            VALUES (?1, ?2, ?3, ?4, datetime('now'))
            ON CONFLICT(id) DO UPDATE SET
                cwd = excluded.cwd,
                goal = excluded.goal,
                state_json = excluded.state_json,
                updated_at = datetime('now')
            "#,
            params![
                session.id,
                session.cwd.to_string_lossy().as_ref(),
                session.goal,
                json,
            ],
        )?;
        Ok(())
    }

    pub fn load_session(&self, id: &str) -> Result<Option<Session>> {
        let conn = self.conn.lock().unwrap();
        let raw: Option<String> = conn
            .query_row(
                "SELECT state_json FROM sessions WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        match raw {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    pub fn record_outcome(
        &self,
        session_id: &str,
        item_id: &str,
        title: &str,
        status: ItemStatus,
        notes: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO outcomes (session_id, item_id, title, outcome, notes)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![session_id, item_id, title, status.label(), notes],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn round_trips_session_and_outcomes() {
        let store = Store::open_in_memory().unwrap();
        let mut s = Session::new("s-1".into(), PathBuf::from("/tmp"));
        s.goal = Some("Understand FFT".into());
        store.save_session(&s).unwrap();

        let loaded = store.load_session("s-1").unwrap().unwrap();
        assert_eq!(loaded.id, "s-1");
        assert_eq!(loaded.goal.as_deref(), Some("Understand FFT"));

        store
            .record_outcome("s-1", "n1", "Complex roots", ItemStatus::Understood, Some("solid"))
            .unwrap();
    }
}
