use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::Session;
use crate::secrets::Secret;
use crate::understanding::{Concept, Event, EventKind, Misconception, NewEvent};
use crate::validate::Violation;

fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

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
        let mut conn = self.conn.lock().unwrap();
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
            -- The removed Zed agent server's replay log.
            DROP TABLE IF EXISTS transcript;
            -- The understanding model (theory A4). `events` is strictly append-only.
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
            CREATE INDEX IF NOT EXISTS idx_events_concept_global ON events(concept, id);
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

        // User-global concepts and prerequisites (migrated from per-workspace
        // tables without ever touching append-only `events`).
        let legacy_concepts: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('concepts') WHERE name = 'workspace'")?
            .exists([])?;
        if legacy_concepts {
            Self::migrate_concepts_to_global(&mut conn)?;
        } else {
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS concepts (
                    id TEXT PRIMARY KEY,
                    label TEXT NOT NULL,
                    aliases_json TEXT NOT NULL DEFAULT '[]',
                    domain TEXT NOT NULL DEFAULT '',
                    created_by TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS prerequisites (
                    concept TEXT NOT NULL,
                    needs TEXT NOT NULL,
                    source TEXT NOT NULL,
                    PRIMARY KEY (concept, needs)
                );
                CREATE TABLE IF NOT EXISTS workspace_concepts (
                    workspace TEXT NOT NULL,
                    concept_id TEXT NOT NULL,
                    PRIMARY KEY (workspace, concept_id)
                );
                CREATE INDEX IF NOT EXISTS idx_workspace_concepts_concept
                    ON workspace_concepts(concept_id);
                -- Maps `(workspace, legacy_id)` to its disambiguated canonical
                -- global `concept_id` when migration renamed a colliding id.
                -- `events` is strictly append-only and never mutated; event
                -- queries join against this table to resolve legacy rows.
                CREATE TABLE IF NOT EXISTS concept_id_map (
                    workspace TEXT NOT NULL,
                    legacy_id TEXT NOT NULL,
                    concept_id TEXT NOT NULL,
                    PRIMARY KEY (workspace, legacy_id)
                );
                CREATE INDEX IF NOT EXISTS idx_concept_id_map_concept
                    ON concept_id_map(concept_id);
                "#,
            )?;
            let has_domain: bool = conn
                .prepare("SELECT 1 FROM pragma_table_info('concepts') WHERE name = 'domain'")?
                .exists([])?;
            if !has_domain {
                conn.execute(
                    "ALTER TABLE concepts ADD COLUMN domain TEXT NOT NULL DEFAULT ''",
                    [],
                )?;
            }
        }
        Ok(())
    }

    /// Migrates legacy per-workspace `concepts` and `prerequisites` tables to
    /// user-global tables in a single transaction, leaving `events` untouched:
    /// - Exact `(id, normalized_label)` matches across workspaces merge into
    ///   one global concept (preserving and deduplicating aliases).
    /// - Same `id` with a different normalized label is disambiguated with a
    ///   numeric suffix (`-2`, `-3`, ...), avoiding collisions with any
    ///   existing id, and recorded in `concept_id_map(workspace, legacy_id,
    ///   concept_id)` so append-only `events` rows continue to resolve cleanly.
    fn migrate_concepts_to_global(conn: &mut Connection) -> Result<()> {
        let tx = conn.transaction()?;

        let legacy_rows: Vec<(String, String, String, String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT workspace, id, label, aliases_json, created_by \
                 FROM concepts ORDER BY workspace, id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let legacy_prereqs_has_ws: bool = tx
            .prepare("SELECT 1 FROM pragma_table_info('prerequisites') WHERE name = 'workspace'")?
            .exists([])?;
        let legacy_prereqs: Vec<(Option<String>, String, String, String)> = if legacy_prereqs_has_ws
        {
            let mut stmt = tx.prepare(
                "SELECT workspace, concept, needs, source \
                 FROM prerequisites ORDER BY workspace, concept, needs",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((Some(row.get(0)?), row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            let mut stmt = tx.prepare(
                "SELECT concept, needs, source FROM prerequisites ORDER BY concept, needs",
            )?;
            let rows =
                stmt.query_map([], |row| Ok((None, row.get(0)?, row.get(1)?, row.get(2)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        // Reserve all existing legacy ids so suffixing never steals an id
        // already claimed by another workspace.
        let mut taken_ids: HashSet<String> = legacy_rows.iter().map(|r| r.1.clone()).collect();
        // (legacy_id, normalized_label) -> canonical_id
        let mut by_id_and_label: HashMap<(String, String), String> = HashMap::new();
        // canonical_id -> Concept
        let mut global_concepts: HashMap<String, Concept> = HashMap::new();
        // (workspace, legacy_id) -> canonical_id (only when renamed)
        let mut remaps: HashMap<(String, String), String> = HashMap::new();
        // (workspace, canonical_id)
        let mut ws_links: HashSet<(String, String)> = HashSet::new();

        for (workspace, legacy_id, label, aliases_json, created_by) in legacy_rows {
            let aliases: Vec<String> = serde_json::from_str(&aliases_json).unwrap_or_default();
            let norm = normalize_label(&label);
            let key = (legacy_id.clone(), norm);

            let canonical_id = if let Some(existing_id) = by_id_and_label.get(&key) {
                existing_id.clone()
            } else if !global_concepts.contains_key(&legacy_id) {
                by_id_and_label.insert(key, legacy_id.clone());
                legacy_id.clone()
            } else {
                let disambiguated = (2..)
                    .map(|n| format!("{legacy_id}-{n}"))
                    .find(|cand| !taken_ids.contains(cand))
                    .unwrap();
                taken_ids.insert(disambiguated.clone());
                by_id_and_label.insert(key, disambiguated.clone());
                disambiguated
            };

            match global_concepts.get_mut(&canonical_id) {
                Some(existing) => {
                    for a in aliases {
                        if !existing.aliases.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
                            existing.aliases.push(a);
                        }
                    }
                }
                None => {
                    global_concepts.insert(
                        canonical_id.clone(),
                        Concept {
                            id: canonical_id.clone(),
                            label,
                            aliases,
                            domain: String::new(),
                            created_by,
                        },
                    );
                }
            }

            if canonical_id != legacy_id {
                remaps.insert((workspace.clone(), legacy_id), canonical_id.clone());
            }
            ws_links.insert((workspace, canonical_id));
        }

        tx.execute_batch(
            r#"
            DROP TABLE concepts;
            DROP TABLE IF EXISTS prerequisites;
            CREATE TABLE concepts (
                id TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                aliases_json TEXT NOT NULL DEFAULT '[]',
                domain TEXT NOT NULL DEFAULT '',
                created_by TEXT NOT NULL
            );
            CREATE TABLE prerequisites (
                concept TEXT NOT NULL,
                needs TEXT NOT NULL,
                source TEXT NOT NULL,
                PRIMARY KEY (concept, needs)
            );
            CREATE TABLE IF NOT EXISTS workspace_concepts (
                workspace TEXT NOT NULL,
                concept_id TEXT NOT NULL,
                PRIMARY KEY (workspace, concept_id)
            );
            CREATE INDEX IF NOT EXISTS idx_workspace_concepts_concept
                ON workspace_concepts(concept_id);
            CREATE TABLE IF NOT EXISTS concept_id_map (
                workspace TEXT NOT NULL,
                legacy_id TEXT NOT NULL,
                concept_id TEXT NOT NULL,
                PRIMARY KEY (workspace, legacy_id)
            );
            CREATE INDEX IF NOT EXISTS idx_concept_id_map_concept
                ON concept_id_map(concept_id);
            "#,
        )?;

        let mut sorted_concepts: Vec<Concept> = global_concepts.into_values().collect();
        sorted_concepts.sort_by(|a, b| a.id.cmp(&b.id));
        for c in &sorted_concepts {
            tx.execute(
                "INSERT INTO concepts (id, label, aliases_json, domain, created_by) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    c.id,
                    c.label,
                    serde_json::to_string(&c.aliases)?,
                    c.domain,
                    c.created_by
                ],
            )?;
        }

        for (workspace, concept_id) in &ws_links {
            tx.execute(
                "INSERT OR IGNORE INTO workspace_concepts (workspace, concept_id) VALUES (?1, ?2)",
                params![workspace, concept_id],
            )?;
        }

        for ((workspace, legacy_id), concept_id) in &remaps {
            tx.execute(
                "INSERT INTO concept_id_map (workspace, legacy_id, concept_id) VALUES (?1, ?2, ?3)",
                params![workspace, legacy_id, concept_id],
            )?;
            tx.execute(
                "UPDATE misconceptions SET concept = ?3 WHERE workspace = ?1 AND concept = ?2",
                params![workspace, legacy_id, concept_id],
            )?;
        }

        for (ws, concept, needs, source) in legacy_prereqs {
            let resolved_concept = ws
                .as_ref()
                .and_then(|w| remaps.get(&(w.clone(), concept.clone())))
                .cloned()
                .unwrap_or(concept);
            let resolved_needs = ws
                .as_ref()
                .and_then(|w| remaps.get(&(w.clone(), needs.clone())))
                .cloned()
                .unwrap_or(needs);
            if resolved_concept == resolved_needs {
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO prerequisites (concept, needs, source) VALUES (?1, ?2, ?3)",
                params![resolved_concept, resolved_needs, source],
            )?;
        }

        tx.commit()?;
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

    /// All user-global concepts, ordered by id.
    pub fn all_concepts(&self) -> Result<Vec<Concept>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, label, aliases_json, domain, created_by FROM concepts ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            let aliases: String = row.get(2)?;
            Ok(Concept {
                id: row.get(0)?,
                label: row.get(1)?,
                aliases: serde_json::from_str(&aliases).unwrap_or_default(),
                domain: row.get(3)?,
                created_by: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Global concepts associated with `workspace` (linked directly or
    /// referenced by events in `workspace`, honoring `concept_id_map`).
    pub fn workspace_concepts(&self, workspace: &str) -> Result<Vec<Concept>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT c.id, c.label, c.aliases_json, c.domain, c.created_by \
             FROM concepts c \
             WHERE EXISTS ( \
                 SELECT 1 FROM workspace_concepts wc \
                 WHERE wc.workspace = ?1 AND wc.concept_id = c.id \
             ) OR EXISTS ( \
                 SELECT 1 FROM events e \
                 WHERE e.workspace = ?1 \
                   AND ( \
                       (e.concept = c.id AND NOT EXISTS ( \
                           SELECT 1 FROM concept_id_map m \
                           WHERE m.workspace = e.workspace \
                             AND m.legacy_id = e.concept \
                             AND m.concept_id != c.id \
                       )) \
                       OR EXISTS ( \
                           SELECT 1 FROM concept_id_map m \
                           WHERE m.workspace = e.workspace \
                             AND m.legacy_id = e.concept \
                             AND m.concept_id = c.id \
                       ) \
                   ) \
             ) \
             ORDER BY c.id",
        )?;
        let rows = stmt.query_map(params![workspace], |row| {
            let aliases: String = row.get(2)?;
            Ok(Concept {
                id: row.get(0)?,
                label: row.get(1)?,
                aliases: serde_json::from_str(&aliases).unwrap_or_default(),
                domain: row.get(3)?,
                created_by: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Alias for [`Self::workspace_concepts`].
    pub fn concepts(&self, workspace: &str) -> Result<Vec<Concept>> {
        self.workspace_concepts(workspace)
    }

    /// Inserts the global concept, or merges new aliases (case-insensitively)
    /// and fills in `domain` if currently empty.
    pub fn upsert_concept(&self, concept: &Concept) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::upsert_concept_conn(&conn, concept)
    }

    /// Records that `workspace` references the global `concept_id`.
    pub fn link_workspace_concept(&self, workspace: &str, concept_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO workspace_concepts (workspace, concept_id) VALUES (?1, ?2)",
            params![workspace, concept_id],
        )?;
        Ok(())
    }

    /// Upserts `concept` globally and links it to `workspace` in one call.
    pub fn upsert_workspace_concept(&self, workspace: &str, concept: &Concept) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::upsert_concept_conn(&conn, concept)?;
        conn.execute(
            "INSERT OR IGNORE INTO workspace_concepts (workspace, concept_id) VALUES (?1, ?2)",
            params![workspace, concept.id],
        )?;
        Ok(())
    }

    fn upsert_concept_conn(conn: &Connection, concept: &Concept) -> Result<()> {
        let existing: Option<(String, String)> = conn
            .query_row(
                "SELECT aliases_json, domain FROM concepts WHERE id = ?1",
                params![concept.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match existing {
            None => {
                conn.execute(
                    "INSERT INTO concepts (id, label, aliases_json, domain, created_by) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        concept.id,
                        concept.label,
                        serde_json::to_string(&concept.aliases)?,
                        concept.domain,
                        concept.created_by
                    ],
                )?;
            }
            Some((raw_aliases, existing_domain)) => {
                let mut aliases: Vec<String> =
                    serde_json::from_str(&raw_aliases).unwrap_or_default();
                let before = aliases.len();
                for a in &concept.aliases {
                    if !aliases.iter().any(|x| x.eq_ignore_ascii_case(a)) {
                        aliases.push(a.clone());
                    }
                }
                let new_domain = if existing_domain.is_empty() && !concept.domain.is_empty() {
                    Some(concept.domain.as_str())
                } else {
                    None
                };
                if aliases.len() != before || new_domain.is_some() {
                    let domain = new_domain.unwrap_or(existing_domain.as_str());
                    conn.execute(
                        "UPDATE concepts SET aliases_json = ?2, domain = ?3 WHERE id = ?1",
                        params![concept.id, serde_json::to_string(&aliases)?, domain],
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn add_prerequisite(&self, concept: &str, needs: &str, source: &str) -> Result<()> {
        if concept == needs {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO prerequisites (concept, needs, source) VALUES (?1, ?2, ?3)",
            params![concept, needs, source],
        )?;
        Ok(())
    }

    /// Global prerequisite graph: concept → the concepts it needs first.
    pub fn prerequisites(&self) -> Result<HashMap<String, Vec<String>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT concept, needs FROM prerequisites ORDER BY concept, needs")?;
        let rows = stmt.query_map([], |row| {
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
            tx.execute(
                "INSERT OR IGNORE INTO workspace_concepts (workspace, concept_id) VALUES (?1, ?2)",
                params![workspace, e.concept],
            )?;
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Every event for `concepts` across ALL workspaces, oldest first.
    /// Legacy concept ids disambiguated during migration are resolved to their
    /// canonical `concept` id transparently without mutating `events`.
    pub fn events_for_concepts(&self, concepts: &[String]) -> Result<Vec<Event>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.workspace, e.kind, e.ref, e.detail_json, e.at \
             FROM events e \
             WHERE ( \
                 e.concept = ?1 \
                 AND NOT EXISTS ( \
                     SELECT 1 FROM concept_id_map m \
                     WHERE m.workspace = e.workspace \
                       AND m.legacy_id = e.concept \
                       AND m.concept_id != ?1 \
                 ) \
             ) OR EXISTS ( \
                 SELECT 1 FROM concept_id_map m \
                 WHERE m.workspace = e.workspace \
                   AND m.legacy_id = e.concept \
                   AND m.concept_id = ?1 \
             ) \
             ORDER BY e.id",
        )?;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for c in concepts {
            if !seen.insert(c.as_str()) {
                continue;
            }
            let rows = stmt.query_map(params![c], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?;
            for row in rows {
                let (id, workspace, kind, reference, detail, at) = row?;
                let Some(kind) = EventKind::parse(&kind) else {
                    continue;
                };
                out.push(Event {
                    id,
                    workspace,
                    concept: c.clone(),
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

    /// Every event for `concepts` in `workspace`, oldest first.
    /// Honors `concept_id_map` for legacy disambiguated ids.
    pub fn events(&self, workspace: &str, concepts: &[String]) -> Result<Vec<Event>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.workspace, e.kind, e.ref, e.detail_json, e.at \
             FROM events e \
             WHERE e.workspace = ?1 \
               AND ( \
                   (e.concept = ?2 AND NOT EXISTS ( \
                       SELECT 1 FROM concept_id_map m \
                       WHERE m.workspace = e.workspace \
                         AND m.legacy_id = e.concept \
                         AND m.concept_id != ?2 \
                   )) \
                   OR EXISTS ( \
                       SELECT 1 FROM concept_id_map m \
                       WHERE m.workspace = e.workspace \
                         AND m.legacy_id = e.concept \
                         AND m.concept_id = ?2 \
                   ) \
               ) \
             ORDER BY e.id",
        )?;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for c in concepts {
            if !seen.insert(c.as_str()) {
                continue;
            }
            let rows = stmt.query_map(params![workspace, c], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?;
            for row in rows {
                let (id, ws, kind, reference, detail, at) = row?;
                let Some(kind) = EventKind::parse(&kind) else {
                    continue;
                };
                out.push(Event {
                    id,
                    workspace: ws,
                    concept: c.clone(),
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

    /// Every session with the seconds since it was last saved, the oldest
    /// first.
    pub fn all_sessions_aged(&self) -> Result<Vec<(Session, u64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT state_json, CAST(strftime('%s','now') - strftime('%s', updated_at) AS INTEGER)
             FROM sessions ORDER BY updated_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (json, age) = row?;
            if let Ok(s) = serde_json::from_str(&json) {
                out.push((s, age.unwrap_or(0).max(0) as u64));
            }
        }
        Ok(out)
    }

    /// (concept, kind) → count of a session's events.
    pub fn event_counts(&self, session_id: &str) -> Result<HashMap<(String, String), usize>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            // A claim withdrawn by `/undo` no longer counts (§15).
            "SELECT concept, kind, COUNT(*) FROM events WHERE session_id = ?1 AND NOT (kind = 'claim' AND EXISTS (SELECT 1 FROM events r WHERE r.kind = 'retracted' AND r.workspace = events.workspace AND r.ref = CAST(events.id AS TEXT))) GROUP BY concept, kind",
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

    #[test]
    fn event_counts_skip_retracted_claims() {
        let store = Store::open_in_memory().unwrap();
        let ids = store
            .append_events(
                "/w",
                "s",
                &[
                    NewEvent::new("c", EventKind::Claim),
                    NewEvent::new("d", EventKind::Claim),
                    NewEvent::new("c", EventKind::Pass),
                ],
                5,
            )
            .unwrap();
        // One claim withdrawn; a retraction aimed at the pass does nothing.
        store
            .append_events(
                "/w",
                "s",
                &[
                    NewEvent::new("c", EventKind::Retracted).reference(ids[0].to_string()),
                    NewEvent::new("c", EventKind::Retracted).reference(ids[2].to_string()),
                ],
                6,
            )
            .unwrap();
        let counts = store.event_counts("s").unwrap();
        assert_eq!(counts.get(&("c".into(), "claim".into())), None);
        assert_eq!(counts.get(&("d".into(), "claim".into())), Some(&1));
        assert_eq!(counts.get(&("c".into(), "pass".into())), Some(&1));
    }

    #[test]
    fn global_concepts_domain_workspace_links_and_cross_workspace_events() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_workspace_concept(
                "/w1",
                &Concept {
                    id: "maxwell".into(),
                    label: "Maxwell's equations".into(),
                    aliases: vec!["Maxwell".into()],
                    domain: String::new(),
                    created_by: "map".into(),
                },
            )
            .unwrap();
        // Upserting globally merges new aliases (case-insensitive) and fills empty domain.
        store
            .upsert_concept(&Concept {
                id: "maxwell".into(),
                label: "Maxwell's equations".into(),
                aliases: vec!["maxwell".into(), "EM equations".into()],
                domain: "electromagnetism".into(),
                created_by: "extractor".into(),
            })
            .unwrap();
        // A later upsert with a different domain does not clobber the existing domain.
        store
            .upsert_concept(&Concept {
                id: "maxwell".into(),
                label: "Maxwell's equations".into(),
                aliases: vec![],
                domain: "other".into(),
                created_by: "extractor".into(),
            })
            .unwrap();

        let all = store.all_concepts().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].domain, "electromagnetism");
        assert_eq!(all[0].aliases, vec!["Maxwell", "EM equations"]);

        assert_eq!(store.workspace_concepts("/w1").unwrap().len(), 1);
        assert!(store.workspace_concepts("/w2").unwrap().is_empty());

        store
            .append_events(
                "/w1",
                "s1",
                &[NewEvent::new("maxwell", EventKind::Pass)],
                10,
            )
            .unwrap();
        store
            .append_events(
                "/w2",
                "s2",
                &[NewEvent::new("maxwell", EventKind::Revisit)],
                20,
            )
            .unwrap();

        // Appending events in /w2 links the concept to /w2.
        assert_eq!(store.workspace_concepts("/w2").unwrap().len(), 1);

        let global_events = store.events_for_concepts(&["maxwell".into()]).unwrap();
        assert_eq!(global_events.len(), 2);
        assert_eq!(global_events[0].workspace, "/w1");
        assert_eq!(global_events[0].kind, EventKind::Pass);
        assert_eq!(global_events[1].workspace, "/w2");
        assert_eq!(global_events[1].kind, EventKind::Revisit);

        let w2_events = store.events("/w2", &["maxwell".into()]).unwrap();
        assert_eq!(w2_events.len(), 1);
        assert_eq!(w2_events[0].workspace, "/w2");
        assert_eq!(w2_events[0].kind, EventKind::Revisit);
    }

    #[test]
    fn global_prerequisites_dedup_and_ignore_self_loops() {
        let store = Store::open_in_memory().unwrap();
        store.add_prerequisite("b", "a", "map").unwrap();
        store.add_prerequisite("b", "a", "extractor").unwrap();
        store.add_prerequisite("b", "b", "map").unwrap();
        let prereqs = store.prerequisites().unwrap();
        assert_eq!(prereqs.get("b"), Some(&vec!["a".to_string()]));
    }

    #[test]
    fn migrates_legacy_workspace_concepts_and_resolves_collisions_without_mutating_events() {
        let dir = std::env::temp_dir().join(format!(
            "pathfinder-migrate-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("state.db");

        // Build a legacy database with per-workspace concepts & prerequisites
        // and append-only triggers already installed on `events`.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE concepts (
                    workspace TEXT NOT NULL,
                    id TEXT NOT NULL,
                    label TEXT NOT NULL,
                    aliases_json TEXT NOT NULL DEFAULT '[]',
                    created_by TEXT NOT NULL,
                    PRIMARY KEY (workspace, id)
                );
                CREATE TABLE prerequisites (
                    workspace TEXT NOT NULL,
                    concept TEXT NOT NULL,
                    needs TEXT NOT NULL,
                    source TEXT NOT NULL,
                    PRIMARY KEY (workspace, concept, needs)
                );
                CREATE TABLE events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    workspace TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    concept TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    ref TEXT,
                    detail_json TEXT NOT NULL DEFAULT 'null',
                    at INTEGER NOT NULL
                );
                CREATE TRIGGER events_append_only_update BEFORE UPDATE ON events
                    BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
                CREATE TRIGGER events_append_only_delete BEFORE DELETE ON events
                    BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
                CREATE TABLE misconceptions (
                    id TEXT PRIMARY KEY,
                    workspace TEXT NOT NULL,
                    concept TEXT NOT NULL,
                    statement TEXT NOT NULL,
                    source TEXT NOT NULL
                );
                "#,
            )
            .unwrap();

            // Exact (id, normalized label) match across /w1 and /w2 -> merges.
            conn.execute(
                "INSERT INTO concepts VALUES ('/w1', 'phasors', 'Phasors', '[\"phasor\"]', 'map')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO concepts VALUES ('/w2', 'phasors', '  phasors  ', '[\"Phasor\", \"complex amplitude\"]', 'map')",
                [],
            )
            .unwrap();

            // Same id, different label across /w1 and /w2; /w3 already owns `kernel-2`;
            // /w4 matches /w2's normalized label and should merge with /w2's disambiguated id (`kernel-3`).
            conn.execute(
                "INSERT INTO concepts VALUES ('/w1', 'kernel', 'Linux Kernel', '[]', 'map')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO concepts VALUES ('/w2', 'kernel', 'Kernel of a Linear Map', '[\"nullspace\"]', 'map')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO concepts VALUES ('/w3', 'kernel-2', 'Existing Kernel Two', '[]', 'map')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO concepts VALUES ('/w4', 'kernel', '  kernel of a linear map ', '[\"null space\"]', 'map')",
                [],
            )
            .unwrap();

            conn.execute(
                "INSERT INTO prerequisites VALUES ('/w2', 'kernel', 'phasors', 'map')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO misconceptions VALUES ('m-lin', '/w2', 'kernel', 'confuses kernel with image', 'judge')",
                [],
            )
            .unwrap();

            // Append-only events under legacy concept ids.
            conn.execute(
                "INSERT INTO events (workspace, session_id, concept, kind, detail_json, at) \
                 VALUES ('/w1', 's1', 'kernel', 'pass', '{\"flawed\":false}', 100)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO events (workspace, session_id, concept, kind, detail_json, at) \
                 VALUES ('/w2', 's2', 'kernel', 'pass', '{\"flawed\":true}', 200)",
                [],
            )
            .unwrap();
        }

        // Open with Store -> triggers migration without touching `events`.
        let store = Store::open(&db_path).unwrap();

        let all = store.all_concepts().unwrap();
        let ids: Vec<&str> = all.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["kernel", "kernel-2", "kernel-3", "phasors"]);

        let phasors = all.iter().find(|c| c.id == "phasors").unwrap();
        assert_eq!(phasors.aliases, vec!["phasor", "complex amplitude"]);
        assert_eq!(phasors.domain, "");

        let lin_kernel = all.iter().find(|c| c.id == "kernel-3").unwrap();
        assert_eq!(lin_kernel.label, "Kernel of a Linear Map");
        assert_eq!(lin_kernel.aliases, vec!["nullspace", "null space"]);

        // Raw `events` table still has the original unmutated concept column,
        // and append-only triggers are intact.
        {
            let conn = store.conn.lock().unwrap();
            let raw: Vec<(String, String)> = conn
                .prepare("SELECT workspace, concept FROM events ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            assert_eq!(
                raw,
                vec![
                    ("/w1".into(), "kernel".into()),
                    ("/w2".into(), "kernel".into()),
                ]
            );
            assert!(conn.execute("DELETE FROM events", []).is_err());
            assert!(conn.execute("UPDATE events SET concept = 'x'", []).is_err());
        }

        // Global event queries cleanly separate `/w1`'s `kernel` from `/w2`'s `kernel-3`.
        let os_events = store.events_for_concepts(&["kernel".into()]).unwrap();
        assert_eq!(os_events.len(), 1);
        assert_eq!(os_events[0].workspace, "/w1");
        assert_eq!(os_events[0].concept, "kernel");

        let lin_events = store.events_for_concepts(&["kernel-3".into()]).unwrap();
        assert_eq!(lin_events.len(), 1);
        assert_eq!(lin_events[0].workspace, "/w2");
        assert_eq!(lin_events[0].concept, "kernel-3");

        // Appending a new event under canonical `kernel-3` in `/w2` joins cleanly
        // with the legacy event in both global and workspace-filtered queries.
        store
            .append_events(
                "/w2",
                "s2",
                &[NewEvent::new("kernel-3", EventKind::ReviewOk)],
                300,
            )
            .unwrap();
        let lin_w2 = store.events("/w2", &["kernel-3".into()]).unwrap();
        assert_eq!(lin_w2.len(), 2);
        assert_eq!(lin_w2[0].kind, EventKind::Pass);
        assert_eq!(lin_w2[1].kind, EventKind::ReviewOk);

        // Prerequisites and misconceptions were remapped to `kernel-3`.
        let prereqs = store.prerequisites().unwrap();
        assert_eq!(prereqs.get("kernel-3"), Some(&vec!["phasors".to_string()]));
        assert_eq!(
            store.misconception("m-lin").unwrap().unwrap().concept,
            "kernel-3"
        );

        // Re-opening the migrated DB is idempotent.
        drop(store);
        let reopened = Store::open(&db_path).unwrap();
        assert_eq!(reopened.all_concepts().unwrap().len(), 4);

        let _ = fs::remove_dir_all(&dir);
    }
}
