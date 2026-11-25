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

/// Most sign-ins waiting at once (they come from unauthenticated requests).
const MAX_PENDING_LINKS: i64 = 200;

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
        let code = random_hex(6);
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

    pub fn add_thread(
        &self,
        install: &str,
        thread: &pathfinder_schema::SharedThread,
    ) -> Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT OR REPLACE INTO threads (id, install, json, received_at) VALUES (?1, ?2, ?3, ?4)",
            params![thread.id, install, serde_json::to_string(thread)?, now()],
        )?;
        Ok(())
    }

    /// Erases everything from an install, the install itself included.
    pub fn erase(&self, install: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM installs WHERE install = ?1", params![install])?;
        Ok(())
    }

    /// Counts one gate request for today; false once over `cap`.
    pub fn count_gate_use(&self, install: &str, cap: u32) -> Result<bool> {
        let day = (now() / 86_400).to_string();
        let conn = self.0.lock().unwrap();
        let used: u32 = conn
            .query_row(
                "SELECT count FROM gate_use WHERE install = ?1 AND day = ?2",
                params![install, day],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if used >= cap {
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO gate_use (install, day, count) VALUES (?1, ?2, 1)
             ON CONFLICT(install, day) DO UPDATE SET count = count + 1",
            params![install, day],
        )?;
        Ok(true)
    }

    /// Deletes events past the retention period and old gate counts.
    pub fn prune(&self) -> Result<usize> {
        let cutoff = now() - RETENTION_DAYS * 86_400;
        let conn = self.0.lock().unwrap();
        let n = conn.execute("DELETE FROM events WHERE at < ?1", params![cutoff])?;
        conn.execute(
            "DELETE FROM gate_use WHERE CAST(day AS INTEGER) < ?1",
            params![cutoff / 86_400],
        )?;
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

    /// Writes `events.jsonl` and `threads.jsonl` into `dir`, each row with
    /// the install's label.
    pub fn export(&self, dir: &Path) -> Result<(usize, usize)> {
        use std::io::Write;
        std::fs::create_dir_all(dir)?;
        let conn = self.0.lock().unwrap();
        let out = |file: &str, sql: &str| -> Result<usize> {
            let mut f = std::io::BufWriter::new(std::fs::File::create(dir.join(file))?);
            let mut stmt = conn.prepare(sql)?;
            let mut rows = stmt.query([])?;
            let mut n = 0;
            while let Some(r) = rows.next()? {
                let label: String = r.get(0)?;
                let mut v: serde_json::Value = serde_json::from_str(&r.get::<_, String>(1)?)?;
                v["label"] = label.into();
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
        Ok((e, t))
    }
}
