//! The collector's database: invites, installs, events, shared threads and
//! gate-proxy counts. Tokens are stored as SHA-256 hashes only.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

/// Events older than this are deleted.
pub const RETENTION_DAYS: i64 = 365;

/// How long a browser sign-in may take.
pub const LINK_SECS: i64 = 15 * 60;

/// Most sign-ins waiting at once (they come from unauthenticated requests;
/// each install has at most one, and each address a few per minute).
const MAX_PENDING_LINKS: i64 = 2_000;

/// Most threads one install may share.
pub const MAX_THREADS_PER_INSTALL: i64 = 50;

/// Most bug reports one install may send.
pub const MAX_REPORTS_PER_INSTALL: i64 = 100;

/// One day of gate use: date, forwarded, capped, upstream errors.
pub type GateDay = (String, u32, u32, u32);

/// Gate use per day, then today's use per person.
pub type GateStats = (Vec<GateDay>, Vec<(String, u32)>);

/// What the gate proxy may do with one request.
#[derive(Debug, PartialEq)]
pub enum GateCheck {
    Allowed,
    /// The operator switched the shared gate off.
    Off,
    /// This person's checks for today are used up.
    PersonCapped,
    /// All checks for today are used up.
    GlobalCapped,
}

/// A browser sign-in waiting for its person.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub code: String,
    pub events: bool,
    pub threads: bool,
    /// Set once someone on the list approved it.
    pub email: Option<String>,
}

/// What the app hears when it checks on a sign-in.
#[derive(Debug, PartialEq)]
pub enum LinkState {
    Waiting,
    /// Unknown secret, or the sign-in took too long.
    Gone,
    /// Approved: the install's new token.
    Done(String),
}

/// Upper-case letters without vowels or look-alikes: no words, no 0/O.
const CODE_LETTERS: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";

fn link_code() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let mut code = String::with_capacity(9);
    for i in 0..8 {
        if i == 4 {
            code.push('-');
        }
        code.push(CODE_LETTERS[rng.random_range(0..CODE_LETTERS.len())] as char);
    }
    code
}

pub fn normal_email(email: &str) -> String {
    email.trim().to_lowercase()
}

pub struct Db(Mutex<Connection>);

#[derive(Debug, Clone, PartialEq)]
pub struct Install {
    pub install: String,
    pub label: String,
    pub notice: u32,
    pub events: bool,
    pub threads: bool,
}

pub fn hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn random_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS invites (
                 code TEXT PRIMARY KEY,
                 label TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 used_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS installs (
                 install TEXT PRIMARY KEY,
                 token_hash TEXT NOT NULL UNIQUE,
                 label TEXT NOT NULL,
                 notice INTEGER NOT NULL,
                 events INTEGER NOT NULL,
                 threads INTEGER NOT NULL,
                 enrolled_at INTEGER NOT NULL,
                 consent_at INTEGER NOT NULL,
                 stopped INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS events (
                 id TEXT PRIMARY KEY,
                 install TEXT NOT NULL REFERENCES installs(install) ON DELETE CASCADE,
                 at INTEGER NOT NULL,
                 kind TEXT NOT NULL,
                 json TEXT NOT NULL,
                 received_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_events_install ON events(install, at);
             CREATE INDEX IF NOT EXISTS idx_events_kind ON events(kind, at);
             CREATE TABLE IF NOT EXISTS threads (
                 id TEXT NOT NULL,
                 install TEXT NOT NULL REFERENCES installs(install) ON DELETE CASCADE,
                 json TEXT NOT NULL,
                 received_at INTEGER NOT NULL,
                 PRIMARY KEY (install, id)
             );
             CREATE TABLE IF NOT EXISTS people (
                 email TEXT PRIMARY KEY,
                 added_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS links (
                 code TEXT PRIMARY KEY,
                 secret_hash TEXT NOT NULL UNIQUE,
                 install TEXT NOT NULL,
                 notice INTEGER NOT NULL,
                 events INTEGER NOT NULL,
                 threads INTEGER NOT NULL,
                 created_at INTEGER NOT NULL,
                 email TEXT
             );
             CREATE TABLE IF NOT EXISTS reports (
                 id TEXT NOT NULL,
                 install TEXT NOT NULL REFERENCES installs(install) ON DELETE CASCADE,
                 json TEXT NOT NULL,
                 received_at INTEGER NOT NULL,
                 PRIMARY KEY (install, id)
             );
             CREATE TABLE IF NOT EXISTS settings (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS gate_daily (
                 day TEXT PRIMARY KEY,
                 forwarded INTEGER NOT NULL DEFAULT 0,
                 capped INTEGER NOT NULL DEFAULT 0,
                 upstream_errors INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS fetches (
                 day TEXT NOT NULL,
                 what TEXT NOT NULL,
                 version TEXT NOT NULL,
                 count INTEGER NOT NULL,
                 PRIMARY KEY (day, what, version)
             );
             CREATE TABLE IF NOT EXISTS gate_use (
                 install TEXT NOT NULL REFERENCES installs(install) ON DELETE CASCADE,
                 day TEXT NOT NULL,
                 count INTEGER NOT NULL,
                 PRIMARY KEY (install, day)
             );",
        )?;
        Ok(Self(Mutex::new(conn)))
    }

    /// A new single-use invite for `label` (who it's for; only you see it).
    pub fn new_invite(&self, label: &str) -> Result<String> {
        let code = random_hex(16);
        self.0.lock().unwrap().execute(
            "INSERT INTO invites (code, label, created_at) VALUES (?1, ?2, ?3)",
            params![code, label, now()],
        )?;
        Ok(code)
    }

    /// Uses an invite: a new token for `install`. None if the code is
    /// unknown or used. Re-enrolling the same install with a fresh code
    /// replaces its token.
    pub fn enroll(
        &self,
        code: &str,
        install: &str,
        notice: u32,
        events: bool,
        threads: bool,
    ) -> Result<Option<String>> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let label: Option<String> = tx
            .query_row(
                "SELECT label FROM invites WHERE code = ?1 AND used_at IS NULL",
                params![code],
                |r| r.get(0),
            )
            .optional()?;
        let Some(label) = label else {
            return Ok(None);
        };
        if held_by_other(&tx, install, &label)? {
            return Ok(None);
        }
        let token = random_hex(32);
        let t = now();
        tx.execute(
            "UPDATE invites SET used_at = ?2 WHERE code = ?1",
            params![code, t],
        )?;
        tx.execute(
            "INSERT INTO installs (install, token_hash, label, notice, events, threads, enrolled_at, consent_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(install) DO UPDATE SET token_hash = ?2, label = ?3, notice = ?4,
                 events = ?5, threads = ?6, consent_at = ?7, stopped = 0",
            params![install, hash(&token), label, notice, events, threads, t],
        )?;
        tx.commit()?;
        Ok(Some(token))
    }

    /// Lets `email` sign in.
    pub fn allow(&self, email: &str) -> Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT OR IGNORE INTO people (email, added_at) VALUES (?1, ?2)",
            params![normal_email(email), now()],
        )?;
        Ok(())
    }

    /// Takes `email` off the list and stops its installs. Returns how many
    /// installs were stopped.
    pub fn disallow(&self, email: &str) -> Result<usize> {
        let email = normal_email(email);
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM people WHERE email = ?1", params![email])?;
        self.stop(&email)
    }

    pub fn is_allowed(&self, email: &str) -> Result<bool> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT 1 FROM people WHERE email = ?1",
                params![normal_email(email)],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn people(&self) -> Result<Vec<String>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT email FROM people ORDER BY added_at")?;
        let rows = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(rows)
    }

    /// A new browser sign-in for `install`: (code, secret). None when too
    /// many are waiting.
    pub fn start_link(
        &self,
        install: &str,
        notice: u32,
        events: bool,
        threads: bool,
    ) -> Result<Option<(String, String)>> {
        let conn = self.0.lock().unwrap();
        conn.execute(
            "DELETE FROM links WHERE created_at < ?1",
            params![now() - LINK_SECS],
        )?;
        // A new sign-in replaces this install's earlier one.
        conn.execute("DELETE FROM links WHERE install = ?1", params![install])?;
        let pending: i64 = conn.query_row("SELECT COUNT(*) FROM links", [], |r| r.get(0))?;
        if pending >= MAX_PENDING_LINKS {
            return Ok(None);
        }
        let secret = random_hex(32);
        let code = link_code();
        conn.execute(
            "INSERT INTO links (code, secret_hash, install, notice, events, threads, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![code, hash(&secret), install, notice, events, threads, now()],
        )?;
        Ok(Some((code, secret)))
    }

    /// The sign-in behind a code, if it hasn't expired.
    pub fn link(&self, code: &str) -> Result<Option<Link>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT code, events, threads, email FROM links WHERE code = ?1 AND created_at >= ?2",
                params![code.trim().to_uppercase(), now() - LINK_SECS],
                |r| {
                    Ok(Link {
                        code: r.get(0)?,
                        events: r.get(1)?,
                        threads: r.get(2)?,
                        email: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// Approves a sign-in for `email` (already checked to be on the list).
    /// False if the code expired or someone else approved it.
    pub fn approve_link(&self, code: &str, email: &str) -> Result<bool> {
        let email = normal_email(email);
        Ok(self.0.lock().unwrap().execute(
            "UPDATE links SET email = ?2
             WHERE code = ?1 AND created_at >= ?3 AND (email IS NULL OR email = ?2)",
            params![code.trim().to_uppercase(), email, now() - LINK_SECS],
        )? == 1)
    }

    /// Checks on a sign-in. Once approved, enrols the install under the
    /// email that approved it and forgets the sign-in.
    pub fn poll_link(&self, secret: &str) -> Result<LinkState> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        type Row = (String, String, u32, bool, bool, i64, Option<String>);
        let row: Option<Row> = tx
            .query_row(
                "SELECT code, install, notice, events, threads, created_at, email
                 FROM links WHERE secret_hash = ?1",
                params![hash(secret)],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((code, install, notice, events, threads, created_at, email)) = row else {
            return Ok(LinkState::Gone);
        };
        let Some(email) = email else {
            return Ok(if created_at < now() - LINK_SECS {
                LinkState::Gone
            } else {
                LinkState::Waiting
            });
        };
        if held_by_other(&tx, &install, &email)? {
            tx.execute("DELETE FROM links WHERE code = ?1", params![code])?;
            tx.commit()?;
            return Ok(LinkState::Gone);
        }
        let token = random_hex(32);
        let t = now();
        tx.execute(
            "INSERT INTO installs (install, token_hash, label, notice, events, threads, enrolled_at, consent_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(install) DO UPDATE SET token_hash = ?2, label = ?3, notice = ?4,
                 events = ?5, threads = ?6, consent_at = ?7, stopped = 0",
            params![install, hash(&token), email, notice, events, threads, t],
        )?;
        tx.execute("DELETE FROM links WHERE code = ?1", params![code])?;
        tx.commit()?;
        Ok(LinkState::Done(token))
    }

    /// The install a token belongs to, unless stopped.
    pub fn install_for(&self, token: &str) -> Result<Option<(Install, bool)>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT install, label, notice, events, threads, stopped FROM installs WHERE token_hash = ?1",
                params![hash(token)],
                |r| {
                    Ok((
                        Install {
                            install: r.get(0)?,
                            label: r.get(1)?,
                            notice: r.get(2)?,
                            events: r.get(3)?,
                            threads: r.get(4)?,
                        },
                        r.get::<_, bool>(5)?,
                    ))
                },
            )
            .optional()?)
    }

    pub fn set_consent(
        &self,
        install: &str,
        notice: u32,
        events: bool,
        threads: bool,
    ) -> Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE installs SET notice = ?2, events = ?3, threads = ?4, consent_at = ?5 WHERE install = ?1",
            params![install, notice, events, threads, now()],
        )?;
        Ok(())
    }

    /// Stops collecting from an install (its next upload gets 410).
    pub fn stop(&self, label_or_install: &str) -> Result<usize> {
        Ok(self.0.lock().unwrap().execute(
            "UPDATE installs SET stopped = 1, events = 0, threads = 0 WHERE install = ?1 OR label = ?1",
            params![label_or_install],
        )?)
    }

    /// Stores events; ones already stored (a retried upload) are skipped.
    /// Returns how many were new.
    pub fn add_events(
        &self,
        install: &str,
        events: &[pathfinder_schema::Envelope],
    ) -> Result<usize> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let t = now();
        let mut added = 0;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO events (id, install, at, kind, json, received_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for e in events {
                let json = serde_json::to_value(e)?;
                let kind = json["event"]["kind"].as_str().unwrap_or("").to_string();
                added += stmt.execute(params![e.id, install, e.at, kind, json.to_string(), t])?;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// Stores a shared thread. False when the install already shared
    /// [`MAX_THREADS_PER_INSTALL`] (sharing the same thread again replaces it).
    pub fn add_thread(
        &self,
        install: &str,
        thread: &pathfinder_schema::SharedThread,
    ) -> Result<bool> {
        let conn = self.0.lock().unwrap();
        let (count, again): (i64, bool) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(id = ?2), 0) > 0 FROM threads WHERE install = ?1",
            params![install, thread.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count >= MAX_THREADS_PER_INSTALL && !again {
            return Ok(false);
        }
        conn.execute(
            "INSERT OR REPLACE INTO threads (id, install, json, received_at) VALUES (?1, ?2, ?3, ?4)",
            params![thread.id, install, serde_json::to_string(thread)?, now()],
        )?;
        Ok(true)
    }

    /// Erases everything from an install, the install itself included.
    pub fn erase(&self, install: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM installs WHERE install = ?1", params![install])?;
        Ok(())
    }

    /// Stores a bug report. False past [`MAX_REPORTS_PER_INSTALL`].
    pub fn add_report(&self, install: &str, report: &pathfinder_schema::BugReport) -> Result<bool> {
        let conn = self.0.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM reports WHERE install = ?1",
            params![install],
            |r| r.get(0),
        )?;
        if count >= MAX_REPORTS_PER_INSTALL {
            return Ok(false);
        }
        conn.execute(
            "INSERT OR REPLACE INTO reports (id, install, json, received_at) VALUES (?1, ?2, ?3, ?4)",
            params![report.id, install, serde_json::to_string(report)?, now()],
        )?;
        Ok(true)
    }

    /// The newest `n` bug reports: (date, person, report).
    pub fn reports(&self, n: i64) -> Result<Vec<(String, String, pathfinder_schema::BugReport)>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT r.received_at, i.label, r.json FROM reports r JOIN installs i USING (install)
             ORDER BY r.received_at DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![n], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(t, who, json)| Ok((date(t), who, serde_json::from_str(&json)?)))
            .collect()
    }

    /// Whether the shared gate is on (`collector gate on|off`).
    pub fn gate_enabled(&self) -> Result<bool> {
        Ok(self.setting("gate")?.as_deref() != Some("off"))
    }

    pub fn set_gate_enabled(&self, on: bool) -> Result<()> {
        self.set_setting("gate", if on { "on" } else { "off" })
    }

    fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    /// Counts one gate request for today, if allowed: within `per_person`
    /// for everyone with this install's label (all their installs together)
    /// and within `global` for everyone.
    pub fn gate_check(&self, install: &str, per_person: u32, global: u32) -> Result<GateCheck> {
        if !self.gate_enabled()? {
            return Ok(GateCheck::Off);
        }
        let day = (now() / 86_400).to_string();
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let person: u32 = tx.query_row(
            "SELECT COALESCE(SUM(g.count), 0) FROM gate_use g JOIN installs i USING (install)
             WHERE g.day = ?2 AND i.label = (SELECT label FROM installs WHERE install = ?1)",
            params![install, day],
            |r| r.get(0),
        )?;
        let all: u32 = tx
            .query_row(
                "SELECT forwarded FROM gate_daily WHERE day = ?1",
                params![day],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let check = if all >= global {
            GateCheck::GlobalCapped
        } else if person >= per_person {
            GateCheck::PersonCapped
        } else {
            GateCheck::Allowed
        };
        if check == GateCheck::Allowed {
            tx.execute(
                "INSERT INTO gate_use (install, day, count) VALUES (?1, ?2, 1)
                 ON CONFLICT(install, day) DO UPDATE SET count = count + 1",
                params![install, day],
            )?;
            tx.execute(
                "INSERT INTO gate_daily (day, forwarded) VALUES (?1, 1)
                 ON CONFLICT(day) DO UPDATE SET forwarded = forwarded + 1",
                params![day],
            )?;
        } else {
            tx.execute(
                "INSERT INTO gate_daily (day, capped) VALUES (?1, 1)
                 ON CONFLICT(day) DO UPDATE SET capped = capped + 1",
                params![day],
            )?;
        }
        tx.commit()?;
        Ok(check)
    }

    /// Today's forwarded gate checks, for the budget alarm.
    pub fn gate_today(&self) -> Result<u32> {
        let day = (now() / 86_400).to_string();
        Ok(self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT forwarded FROM gate_daily WHERE day = ?1",
                params![day],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn gate_upstream_error(&self) -> Result<()> {
        let day = (now() / 86_400).to_string();
        self.0.lock().unwrap().execute(
            "INSERT INTO gate_daily (day, upstream_errors) VALUES (?1, 1)
             ON CONFLICT(day) DO UPDATE SET upstream_errors = upstream_errors + 1",
            params![day],
        )?;
        Ok(())
    }

    /// The last `days` days of gate use: date, forwarded, capped, upstream
    /// errors; then today's use per person.
    pub fn gate_stats(&self, days: i64) -> Result<GateStats> {
        let today = now() / 86_400;
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT day, forwarded, capped, upstream_errors FROM gate_daily
             WHERE CAST(day AS INTEGER) > ?1 ORDER BY CAST(day AS INTEGER)",
        )?;
        let daily = stmt
            .query_map(params![today - days], |r| {
                let day: String = r.get(0)?;
                Ok((
                    date(day.parse::<i64>().unwrap_or(0) * 86_400),
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = conn.prepare(
            "SELECT i.label, SUM(g.count) FROM gate_use g JOIN installs i USING (install)
             WHERE g.day = ?1 GROUP BY i.label ORDER BY 2 DESC",
        )?;
        let people = stmt
            .query_map(params![today.to_string()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((daily, people))
    }

    /// Counts a download of `what` (`latest.json`, `install.sh`, a release)
    /// by the app version in its user agent: no install, no address.
    pub fn count_fetch(&self, what: &str, version: &str) -> Result<()> {
        let day = (now() / 86_400).to_string();
        self.0.lock().unwrap().execute(
            "INSERT INTO fetches (day, what, version, count) VALUES (?1, ?2, ?3, 1)
             ON CONFLICT(day, what, version) DO UPDATE SET count = count + 1",
            params![day, what, version],
        )?;
        Ok(())
    }

    /// Deletes events past the retention period and old gate counts.
    pub fn prune(&self) -> Result<usize> {
        let cutoff = now() - RETENTION_DAYS * 86_400;
        let conn = self.0.lock().unwrap();
        let n = conn.execute("DELETE FROM events WHERE at < ?1", params![cutoff])?;
        for table in ["gate_use", "gate_daily", "fetches"] {
            conn.execute(
                &format!("DELETE FROM {table} WHERE CAST(day AS INTEGER) < ?1"),
                params![cutoff / 86_400],
            )?;
        }
        Ok(n)
    }

    /// One line per install: label, consent, events, threads, last seen.
    pub fn stats(&self) -> Result<Vec<String>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT i.label, i.events, i.threads, i.stopped,
                    (SELECT COUNT(*) FROM events e WHERE e.install = i.install),
                    (SELECT COUNT(*) FROM threads t WHERE t.install = i.install),
                    (SELECT MAX(at) FROM events e WHERE e.install = i.install),
                    (SELECT COALESCE(SUM(count), 0) FROM gate_use g WHERE g.install = i.install)
             FROM installs i ORDER BY i.enrolled_at",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let yes = |b: bool| if b { "yes" } else { "no" };
                let last: Option<i64> = r.get(6)?;
                Ok(format!(
                    "{:<20} events:{:<3} threads:{:<3}{} {:>7} events {:>4} threads {:>6} gate checks  last {}",
                    r.get::<_, String>(0)?,
                    yes(r.get(1)?),
                    yes(r.get(2)?),
                    if r.get::<_, bool>(3)? { " STOPPED" } else { "" },
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(7)?,
                    last.map(|t| format!("{} days ago", (now() - t) / 86_400))
                        .unwrap_or_else(|| "never".into()),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Writes `events.jsonl`, `threads.jsonl` and `installs.jsonl` into
    /// `dir`. Each row names its person by a pseudonym (a hash of the
    /// label, stable across exports) unless `with_labels`.
    pub fn export(&self, dir: &Path, with_labels: bool) -> Result<(usize, usize)> {
        use std::io::Write;
        std::fs::create_dir_all(dir)?;
        let who = |label: &str| -> String {
            if with_labels {
                label.to_string()
            } else {
                format!("p-{}", &hash(label)[..12])
            }
        };
        let conn = self.0.lock().unwrap();
        let out = |file: &str, sql: &str| -> Result<usize> {
            let mut f = std::io::BufWriter::new(std::fs::File::create(dir.join(file))?);
            let mut stmt = conn.prepare(sql)?;
            let mut rows = stmt.query([])?;
            let mut n = 0;
            while let Some(r) = rows.next()? {
                let label: String = r.get(0)?;
                let mut v: serde_json::Value = serde_json::from_str(&r.get::<_, String>(1)?)?;
                v["person"] = who(&label).into();
                writeln!(f, "{v}")?;
                n += 1;
            }
            Ok(n)
        };
        let e = out(
            "events.jsonl",
            "SELECT i.label, e.json FROM events e JOIN installs i USING (install) ORDER BY e.at",
        )?;
        let t = out(
            "threads.jsonl",
            "SELECT i.label, t.json FROM threads t JOIN installs i USING (install) ORDER BY t.received_at",
        )?;
        out(
            "reports.jsonl",
            "SELECT i.label, r.json FROM reports r JOIN installs i USING (install) ORDER BY r.received_at",
        )?;
        out(
            "installs.jsonl",
            "SELECT label, json_object('install', install, 'enrolled_at', enrolled_at,
                 'consent_at', consent_at, 'notice', notice, 'events', events,
                 'threads', threads, 'stopped', stopped) FROM installs ORDER BY enrolled_at",
        )?;
        Ok((e, t))
    }
}

impl Db {
    /// A plain-text summary for analysis: active people, retention, turn
    /// latency, gate outcomes, errors, versions, models and downloads.
    pub fn report(&self) -> Result<String> {
        use std::fmt::Write;
        let conn = self.0.lock().unwrap();
        let t = now();
        let mut out = String::new();
        let one = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> Result<i64> {
            Ok(conn
                .query_row(sql, p, |r| r.get::<_, Option<i64>>(0))?
                .unwrap_or(0))
        };
        let people = "SELECT COUNT(DISTINCT i.label) FROM events e JOIN installs i USING (install) WHERE e.at >= ?1";
        writeln!(
            out,
            "Active people: {} today, {} in 7 days, {} in 28 days (of {} who joined)",
            one(people, &[&(t - 86_400)])?,
            one(people, &[&(t - 7 * 86_400)])?,
            one(people, &[&(t - 28 * 86_400)])?,
            one("SELECT COUNT(DISTINCT label) FROM installs", &[])?
        )?;

        // Day-N retention: of the people who joined at least N days ago,
        // how many used Pathfinder on day N or later.
        for n in [1i64, 7, 28] {
            let cohort = one(
                "SELECT COUNT(DISTINCT label) FROM installs WHERE enrolled_at <= ?1",
                &[&(t - n * 86_400)],
            )?;
            let kept = one(
                "SELECT COUNT(DISTINCT i.label) FROM installs i JOIN events e USING (install)
                 WHERE i.enrolled_at <= ?1 AND e.at >= i.enrolled_at + ?2",
                &[&(t - n * 86_400), &(n * 86_400)],
            )?;
            writeln!(out, "Day-{n} retention: {kept} of {cohort}")?;
        }

        let mut ms: Vec<i64> = conn
            .prepare("SELECT json_extract(json, '$.event.ms') FROM events WHERE kind = 'turn' AND at >= ?1")?
            .query_map(params![t - 7 * 86_400], |r| r.get::<_, Option<i64>>(0))?
            .filter_map(|r| r.ok().flatten())
            .collect();
        ms.sort_unstable();
        let pct = |p: usize| {
            ms.get(ms.len().saturating_sub(1) * p / 100)
                .copied()
                .unwrap_or(0)
        };
        writeln!(
            out,
            "Turns in 7 days: {}, reply time p50 {} ms, p95 {} ms",
            ms.len(),
            pct(50),
            pct(95)
        )?;

        let mut tally = |title: &str, sql: &str| -> Result<()> {
            let rows: Vec<(String, i64)> = conn
                .prepare(sql)?
                .query_map(params![t - 7 * 86_400], |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?.unwrap_or_else(|| "?".into()),
                        r.get(1)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let line: Vec<String> = rows.iter().map(|(k, n)| format!("{k} {n}")).collect();
            writeln!(
                out,
                "{title}: {}",
                if line.is_empty() {
                    "none".into()
                } else {
                    line.join(", ")
                }
            )?;
            Ok(())
        };
        tally(
            "Gate outcomes (7 days)",
            "SELECT json_extract(json, '$.event.outcome'), COUNT(*) FROM events WHERE kind = 'gate' AND at >= ?1 GROUP BY 1 ORDER BY 2 DESC",
        )?;
        tally(
            "Check outcomes (7 days)",
            "SELECT json_extract(json, '$.event.outcome'), COUNT(*) FROM events WHERE kind = 'check' AND at >= ?1 GROUP BY 1 ORDER BY 2 DESC",
        )?;
        tally(
            "Errors (7 days)",
            "SELECT json_extract(json, '$.event.code'), COUNT(*) FROM events WHERE kind = 'error' AND at >= ?1 GROUP BY 1 ORDER BY 2 DESC",
        )?;
        tally(
            "Crashes (7 days)",
            "SELECT json_extract(json, '$.app'), COUNT(*) FROM events WHERE kind = 'crash' AND at >= ?1 GROUP BY 1 ORDER BY 2 DESC",
        )?;
        tally(
            "Models (7 days)",
            "SELECT json_extract(json, '$.event.model'), COUNT(*) FROM events WHERE kind = 'turn' AND at >= ?1 GROUP BY 1 ORDER BY 2 DESC",
        )?;
        tally(
            "Features (7 days)",
            "SELECT json_extract(json, '$.event.feature'), COUNT(*) FROM events WHERE kind = 'ui' AND at >= ?1 GROUP BY 1 ORDER BY 2 DESC",
        )?;
        tally(
            "App versions in use (7 days)",
            "SELECT json_extract(json, '$.app'), COUNT(DISTINCT install) FROM events WHERE at >= ?1 GROUP BY 1 ORDER BY 1 DESC",
        )?;
        tally(
            "Downloads by app version (7 days)",
            "SELECT what || ' from ' || version, SUM(count) FROM fetches WHERE CAST(day AS INTEGER) * 86400 >= ?1 GROUP BY 1 ORDER BY 2 DESC",
        )?;
        Ok(out)
    }
}

/// `held_by_other`: whether `install` is already enrolled under another
/// label (a guessed or copied install id must not take an install over).
fn held_by_other(conn: &Connection, install: &str, label: &str) -> Result<bool> {
    let held: Option<String> = conn
        .query_row(
            "SELECT label FROM installs WHERE install = ?1",
            params![install],
            |r| r.get(0),
        )
        .optional()?;
    Ok(held.is_some_and(|l| l != label))
}

/// A Unix time as `YYYY-MM-DD` (UTC).
pub fn date(t: i64) -> String {
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = t.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}
