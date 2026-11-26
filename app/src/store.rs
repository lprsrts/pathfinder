use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::Session;
use crate::secrets::Secret;
use crate::understanding::{Concept, Event, EventKind, Misconception, NewEvent};
use crate::validate::Violation;

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
            CREATE TABLE IF NOT EXISTS critic_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                tier TEXT NOT NULL,
                accepted INTEGER NOT NULL,
                escalated INTEGER NOT NULL,
                violations_json TEXT NOT NULL,
                recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_critic_session ON critic_events(session_id);
            -- What Zed was shown, one ACP session update per row, replayed on load.
            CREATE TABLE IF NOT EXISTS transcript (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                update_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_transcript_session ON transcript(session_id, seq);
            -- The understanding model (theory A4). `events` is append-only.
            CREATE TABLE IF NOT EXISTS concepts (
                workspace TEXT NOT NULL,
                id TEXT NOT NULL,
                label TEXT NOT NULL,
                aliases_json TEXT NOT NULL DEFAULT '[]',
                created_by TEXT NOT NULL,
                PRIMARY KEY (workspace, id)
            );
            CREATE TABLE IF NOT EXISTS prerequisites (
                workspace TEXT NOT NULL,
                concept TEXT NOT NULL,
                needs TEXT NOT NULL,
                source TEXT NOT NULL,
                PRIMARY KEY (workspace, concept, needs)
            );
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workspace TEXT NOT NULL,
                session_id TEXT NOT NULL,
                concept TEXT NOT NULL,
                kind TEXT NOT NULL,
                ref TEXT,
                detail_json TEXT NOT NULL DEFAULT 'null',
                at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_events_concept ON events(workspace, concept, id);
            CREATE TRIGGER IF NOT EXISTS events_append_only_update BEFORE UPDATE ON events
                BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
            CREATE TRIGGER IF NOT EXISTS events_append_only_delete BEFORE DELETE ON events
                BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
            -- What is kept from you until your verdict (theory A6).
            CREATE TABLE IF NOT EXISTS secrets (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                probe TEXT NOT NULL,
                opened_at_turn INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_secrets_session ON secrets(session_id, status);
            -- Provenance (theory A7): hash-chained per workspace, append-only.
            CREATE TABLE IF NOT EXISTS ledger (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                workspace TEXT NOT NULL,
                id TEXT NOT NULL UNIQUE,
                entry_json TEXT NOT NULL,
                hash TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_ledger_workspace ON ledger(workspace, seq);
            CREATE TRIGGER IF NOT EXISTS ledger_append_only_update BEFORE UPDATE ON ledger
                BEGIN SELECT RAISE(ABORT, 'the ledger is append-only'); END;
            CREATE TRIGGER IF NOT EXISTS ledger_append_only_delete BEFORE DELETE ON ledger
                BEGIN SELECT RAISE(ABORT, 'the ledger is append-only'); END;
            CREATE TABLE IF NOT EXISTS misconceptions (
                id TEXT PRIMARY KEY,
                workspace TEXT NOT NULL,
                concept TEXT NOT NULL,
                statement TEXT NOT NULL,
                source TEXT NOT NULL
            );
            "#,
        )?;
        // Outcomes gained a `status` column next to the §8 record in `outcome`.
        let has_status: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('outcomes') WHERE name = 'status'")?
            .exists([])?;
        if !has_status {
            conn.execute("ALTER TABLE outcomes ADD COLUMN status TEXT", [])?;
        }
        Ok(())
    }

    pub fn append_transcript(&self, session_id: &str, updates: &[String]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for update in updates {
            tx.execute(
                "INSERT INTO transcript (session_id, update_json) VALUES (?1, ?2)",
                params![session_id, update],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_transcript(&self, session_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT update_json FROM transcript WHERE session_id = ?1 ORDER BY seq")?;
        let rows = stmt.query_map(params![session_id], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
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
            Some(json) => match serde_json::from_str(&json) {
                Ok(session) => Ok(Some(session)),
                // Sessions saved by an older state format can't be resumed;
                // treat them as new rather than failing the thread.
                Err(err) => {
                    tracing::warn!("discarding unreadable session `{id}`: {err}");
                    Ok(None)
                }
            },
            None => Ok(None),
        }
    }

    pub fn record_outcome(
        &self,
        session_id: &str,
        item_id: &str,
        title: &str,
        record: &str,
        status: &str,
        notes: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO outcomes (session_id, item_id, title, outcome, status, notes)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![session_id, item_id, title, record, status, notes],
        )?;
        Ok(())
    }

    pub fn record_critic_event(
        &self,
        session_id: &str,
        kind: &str,
        tier: &str,
        accepted: bool,
        escalated: bool,
        violations: &[Violation],
    ) -> Result<()> {
        let json = serde_json::to_string(violations)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO critic_events (session_id, kind, tier, accepted, escalated, violations_json)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                session_id,
                kind,
                tier,
                i32::from(accepted),
                i32::from(escalated),
                json,
            ],
        )?;
        Ok(())
    }

    // The understanding model (theory A4).

    pub fn concepts(&self, workspace: &str) -> Result<Vec<Concept>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, label, aliases_json, created_by FROM concepts WHERE workspace = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![workspace], |row| {
            let aliases: String = row.get(2)?;
            Ok(Concept {
                id: row.get(0)?,
                label: row.get(1)?,
                aliases: serde_json::from_str(&aliases).unwrap_or_default(),
                created_by: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Inserts the concept, or adds new aliases to the one already there.
    pub fn upsert_concept(&self, workspace: &str, concept: &Concept) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let existing: Option<String> = conn
            .query_row(
                "SELECT aliases_json FROM concepts WHERE workspace = ?1 AND id = ?2",
                params![workspace, concept.id],
                |row| row.get(0),
            )
            .optional()?;
        match existing {
            None => {
                conn.execute(
                    "INSERT INTO concepts (workspace, id, label, aliases_json, created_by) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        workspace,
                        concept.id,
                        concept.label,
                        serde_json::to_string(&concept.aliases)?,
                        concept.created_by
                    ],
                )?;
            }
            Some(raw) => {
                let mut aliases: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
                let before = aliases.len();
                for a in &concept.aliases {
                    if !aliases.iter().any(|x| x.eq_ignore_ascii_case(a)) {
                        aliases.push(a.clone());
                    }
                }
                if aliases.len() != before {
                    conn.execute(
                        "UPDATE concepts SET aliases_json = ?3 WHERE workspace = ?1 AND id = ?2",
                        params![workspace, concept.id, serde_json::to_string(&aliases)?],
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn add_prerequisite(
        &self,
        workspace: &str,
        concept: &str,
        needs: &str,
        source: &str,
    ) -> Result<()> {
        if concept == needs {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO prerequisites (workspace, concept, needs, source) VALUES (?1, ?2, ?3, ?4)",
            params![workspace, concept, needs, source],
        )?;
        Ok(())
    }

    /// concept → the concepts it needs first.
    pub fn prerequisites(&self, workspace: &str) -> Result<HashMap<String, Vec<String>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT concept, needs FROM prerequisites WHERE workspace = ?1 ORDER BY concept, needs",
        )?;
        let rows = stmt.query_map(params![workspace], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows {
            let (c, n) = row?;
            out.entry(c).or_default().push(n);
        }
        Ok(out)
    }

    /// Appends events in one transaction, stamped with `at`; returns their ids.
    pub fn append_events(
        &self,
        workspace: &str,
        session_id: &str,
        events: &[NewEvent],
        at: i64,
    ) -> Result<Vec<i64>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut ids = Vec::new();
        for e in events {
            tx.execute(
                "INSERT INTO events (workspace, session_id, concept, kind, ref, detail_json, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    workspace,
                    session_id,
                    e.concept,
                    e.kind.as_str(),
                    e.reference,
                    serde_json::to_string(&e.detail)?,
                    at
                ],
            )?;
            ids.push(tx.last_insert_rowid());
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Every event for `concepts` in this workspace, oldest first.
    pub fn events(&self, workspace: &str, concepts: &[String]) -> Result<Vec<Event>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, concept, kind, ref, detail_json, at FROM events WHERE workspace = ?1 AND concept = ?2 ORDER BY id",
        )?;
        let mut out = Vec::new();
        for c in concepts {
            let rows = stmt.query_map(params![workspace, c], |row| {
                let kind: String = row.get(2)?;
                let detail: String = row.get(4)?;
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    kind,
                    row.get::<_, Option<String>>(3)?,
                    detail,
                    row.get::<_, i64>(5)?,
                ))
            })?;
            for row in rows {
                let (id, concept, kind, reference, detail, at) = row?;
                let Some(kind) = EventKind::parse(&kind) else {
                    continue;
                };
                out.push(Event {
                    id,
                    concept,
                    kind,
                    reference,
                    detail: serde_json::from_str(&detail).unwrap_or_default(),
                    at,
                });
            }
        }
        out.sort_by_key(|e| e.id);
        Ok(out)
    }

    // The ledger (theory A7).

    /// Chains `entry` onto the workspace's ledger and appends it.
    pub fn append_ledger(
        &self,
        workspace: &str,
        mut entry: crate::ledger::Entry,
    ) -> Result<crate::ledger::Entry> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let prev: Option<String> = tx
            .query_row(
                "SELECT hash FROM ledger WHERE workspace = ?1 ORDER BY seq DESC LIMIT 1",
                params![workspace],
                |row| row.get(0),
            )
            .optional()?;
        entry.prev_hash = prev.unwrap_or_else(|| crate::ledger::GENESIS.to_string());
        entry.hash = entry.compute_hash();
        tx.execute(
            "INSERT INTO ledger (workspace, id, entry_json, hash) VALUES (?1, ?2, ?3, ?4)",
            params![
                workspace,
                entry.id,
                serde_json::to_string(&entry)?,
                entry.hash
            ],
        )?;
        tx.commit()?;
        Ok(entry)
    }

    pub fn ledger(&self, workspace: &str) -> Result<Vec<crate::ledger::Entry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT entry_json FROM ledger WHERE workspace = ?1 ORDER BY seq")?;
        let rows = stmt.query_map(params![workspace], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Every saved session (for the evals' evidence report).
    pub fn all_sessions(&self) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT state_json FROM sessions ORDER BY updated_at")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            if let Ok(s) = serde_json::from_str(&row?) {
                out.push(s);
            }
        }
        Ok(out)
    }

    /// (concept, kind) → count of a session's events.
    pub fn event_counts(&self, session_id: &str) -> Result<HashMap<(String, String), usize>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT concept, kind, COUNT(*) FROM events WHERE session_id = ?1 GROUP BY concept, kind",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok((
                (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                row.get::<_, i64>(2)? as usize,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    pub fn ledger_workspaces(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT workspace FROM ledger ORDER BY workspace")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // Secrets (theory A6).

    pub fn add_secret(&self, session_id: &str, secret: &Secret) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO secrets (id, session_id, kind, payload_json, probe, opened_at_turn) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                secret.id,
                session_id,
                secret.kind,
                serde_json::to_string(&secret.payload)?,
                secret.probe,
                secret.opened_at_turn as i64
            ],
        )?;
        Ok(())
    }

    fn secrets_where(&self, clause: &str, arg: &str) -> Result<Vec<Secret>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, kind, payload_json, probe, opened_at_turn, status FROM secrets WHERE {clause} ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map(params![arg], |row| {
            let payload: String = row.get(2)?;
            Ok(Secret {
                id: row.get(0)?,
                kind: row.get(1)?,
                payload: serde_json::from_str(&payload).unwrap_or_default(),
                probe: row.get(3)?,
                opened_at_turn: row.get::<_, i64>(4)? as usize,
                active: row.get::<_, String>(5)? == "active",
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn secret(&self, id: &str) -> Result<Option<Secret>> {
        Ok(self.secrets_where("id = ?1", id)?.into_iter().next())
    }

    pub fn active_secrets(&self, session_id: &str) -> Result<Vec<Secret>> {
        self.secrets_where("session_id = ?1 AND status = 'active'", session_id)
    }

    pub fn retire_secret(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE secrets SET status = 'retired' WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn add_misconception(&self, workspace: &str, m: &Misconception) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO misconceptions (id, workspace, concept, statement, source) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![m.id, workspace, m.concept, m.statement, m.source],
        )?;
        Ok(())
    }

    pub fn misconception(&self, id: &str) -> Result<Option<Misconception>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT id, concept, statement, source FROM misconceptions WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Misconception {
                        id: row.get(0)?,
                        concept: row.get(1)?,
                        statement: row.get(2)?,
                        source: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn round_trips_session_outcomes_and_critic_events() {
        let store = Store::open_in_memory().unwrap();
        let mut s = Session::new("s-1".into(), PathBuf::from("/tmp"));
        s.goal = Some("Understand FFT".into());
        store.save_session(&s).unwrap();

        let loaded = store.load_session("s-1").unwrap().unwrap();
        assert_eq!(loaded.id, "s-1");
        assert_eq!(loaded.goal.as_deref(), Some("Understand FFT"));

        store
            .record_outcome(
                "s-1",
                "n1",
                "Complex roots",
                "understood",
                "understood",
                Some("solid"),
            )
            .unwrap();
        store
            .record_critic_event(
                "s-1",
                "onboarding",
                "system1",
                false,
                true,
                &[Violation::new("empty_field", "`reply` must not be empty")],
            )
            .unwrap();
    }
}

#[cfg(test)]
mod understanding_tests {
    use super::*;

    #[test]
    fn events_are_append_only() {
        let store = Store::open_in_memory().unwrap();
        store
            .append_events("/w", "s", &[NewEvent::new("c", EventKind::Claim)], 5)
            .unwrap();
        let events = store.events("/w", &["c".into()]).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Claim);
        let conn = store.conn.lock().unwrap();
        assert!(conn.execute("DELETE FROM events", []).is_err());
        assert!(conn.execute("UPDATE events SET kind = 'pass'", []).is_err());
    }
}
