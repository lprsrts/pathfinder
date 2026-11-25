//! Usage events, sent only with consent. The events themselves are
//! defined in `pathfinder-schema`: numbers, flags and closed sets of
//! names, with no message text, file names or paths.
//!
//! Events go to a local outbox first (a table in the app's database) and
//! are uploaded in batches, so a collector that is down loses nothing.
//! Nothing is written at all unless you said yes to usage events and the
//! install is enrolled; `HARNESS_TELEMETRY=off` always wins.

pub mod consent;
mod upload;

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use pathfinder_schema::{Arch, Envelope, Event, Os, VERSION};
use rusqlite::{Connection, params};

pub use consent::Consent;
pub use upload::{collector_url, delete_remote, enroll, flush, set_remote_consent, share};

/// The outbox keeps at most this many events; the oldest go first.
const MAX_QUEUED: i64 = 20_000;

struct Telemetry {
    db: Mutex<Connection>,
    consent: Mutex<Consent>,
    session: Mutex<String>,
    locale: Mutex<String>,
}

static GLOBAL: OnceLock<Telemetry> = OnceLock::new();

fn env_off() -> bool {
    matches!(
        std::env::var("HARNESS_TELEMETRY").as_deref().map(str::trim),
        Ok("off" | "0" | "false" | "no")
    )
}

/// Opens the outbox next to the app's database. Call once at start-up;
/// without it, `emit` does nothing.
pub fn init(db_path: &Path) -> Result<()> {
    let db = open_outbox(db_path)?;
    let _ = GLOBAL.set(Telemetry {
        db: Mutex::new(db),
        consent: Mutex::new(Consent::load()),
        session: Mutex::new(uuid::Uuid::new_v4().to_string()),
        locale: Mutex::new(String::new()),
    });
    Ok(())
}

fn open_outbox(db_path: &Path) -> Result<Connection> {
    if let Some(dir) = db_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let db = Connection::open(db_path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    create_table(&db)?;
    Ok(db)
}

fn create_table(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS telemetry_outbox (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            json TEXT NOT NULL
        );",
    )?;
    Ok(())
}

/// Whether events are being collected right now.
pub fn enabled() -> bool {
    GLOBAL
        .get()
        .is_some_and(|t| t.consent.lock().unwrap().collecting())
        && !env_off()
}

/// Reloads the consent file (after `harness data` or the consent screen
/// changed it).
pub fn reload() {
    if let Some(t) = GLOBAL.get() {
        *t.consent.lock().unwrap() = Consent::load();
    }
}

/// Starts a new session id: a new thread was opened.
pub fn new_session() {
    if let Some(t) = GLOBAL.get() {
        *t.session.lock().unwrap() = uuid::Uuid::new_v4().to_string();
    }
}

/// The conversation's language, e.g. `de`; dropped if it isn't a short tag.
pub fn set_locale(locale: &str) {
    if let Some(t) = GLOBAL.get() {
        let tag = if pathfinder_schema::is_token(locale, 16) {
            locale
        } else {
            ""
        };
        *t.locale.lock().unwrap() = tag.to_string();
    }
}

/// Queues an event if collecting; otherwise does nothing. Never fails the
/// caller: a broken outbox only logs.
pub fn emit(event: Event) {
    let Some(t) = GLOBAL.get() else { return };
    if !enabled() {
        return;
    }
    let env = envelope(t, event);
    if let Err(e) = env.validate() {
        tracing::warn!("not queueing an invalid usage event: {e}");
        return;
    }
    if let Err(e) = push(&t.db.lock().unwrap(), &env) {
        tracing::warn!("queueing a usage event: {e:#}");
    }
}

fn envelope(t: &Telemetry, event: Event) -> Envelope {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Envelope {
        v: VERSION,
        id: uuid::Uuid::new_v4().to_string(),
        session: t.session.lock().unwrap().clone(),
        at: now - now % 60,
        app: env!("CARGO_PKG_VERSION").to_string(),
        os: Os::current(),
        arch: Arch::current(),
        locale: t.locale.lock().unwrap().clone(),
        event,
    }
}

fn push(db: &Connection, env: &Envelope) -> Result<()> {
    db.execute(
        "INSERT INTO telemetry_outbox (json) VALUES (?1)",
        params![serde_json::to_string(env)?],
    )?;
    db.execute(
        "DELETE FROM telemetry_outbox WHERE seq <= (SELECT MAX(seq) FROM telemetry_outbox) - ?1",
        params![MAX_QUEUED],
    )?;
    Ok(())
}

/// Up to `limit` queued events, oldest first: (seq, json).
fn queued(db: &Connection, limit: usize) -> Result<Vec<(i64, String)>> {
    let mut stmt = db.prepare("SELECT seq, json FROM telemetry_outbox ORDER BY seq LIMIT ?1")?;
    let rows = stmt
        .query_map(params![limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn sent_up_to(db: &Connection, seq: i64) -> Result<()> {
    db.execute("DELETE FROM telemetry_outbox WHERE seq <= ?1", params![seq])?;
    Ok(())
}

fn clear(db: &Connection) -> Result<()> {
    db.execute("DELETE FROM telemetry_outbox", [])?;
    Ok(())
}

/// The outbox, for `harness data` (which runs without `init`).
pub struct Outbox(Connection);

impl Outbox {
    pub fn open(db_path: &Path) -> Result<Self> {
        Ok(Self(open_outbox(db_path)?))
    }

    pub fn count(&self) -> Result<i64> {
        Ok(self
            .0
            .query_row("SELECT COUNT(*) FROM telemetry_outbox", [], |r| r.get(0))?)
    }

    pub fn peek(&self, limit: usize) -> Result<Vec<String>> {
        Ok(queued(&self.0, limit)?
            .into_iter()
            .map(|(_, j)| j)
            .collect())
    }

    pub fn clear(&self) -> Result<()> {
        clear(&self.0)
    }
}

/// Counts within one turn: model calls and retries, from wherever the
/// turn's engine runs (it stays on the turn's task).
#[derive(Default, Debug)]
pub struct TurnCounters {
    pub model_calls: std::sync::atomic::AtomicU32,
    pub retries: std::sync::atomic::AtomicU32,
}

tokio::task_local! {
    static TURN: std::sync::Arc<TurnCounters>;
}

/// Runs `fut` with fresh turn counters, returning them with its output.
pub async fn with_turn_counters<F: std::future::Future>(
    fut: F,
) -> (F::Output, std::sync::Arc<TurnCounters>) {
    let counters = std::sync::Arc::new(TurnCounters::default());
    let out = TURN.scope(counters.clone(), fut).await;
    (out, counters)
}

/// One model call made in this turn.
pub fn count_model_call() {
    let _ = TURN.try_with(|c| {
        c.model_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    });
}

/// One retry made in this turn.
pub fn count_retry() {
    let _ = TURN.try_with(|c| c.retries.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
}

/// The error code for a provider error.
pub fn llm_error_code(err: &crate::llm::LlmError) -> pathfinder_schema::ErrorCode {
    use crate::llm::LlmError as E;
    use pathfinder_schema::ErrorCode as C;
    match err {
        E::Auth(_) => C::Auth,
        E::RateLimited { .. } => C::RateLimited,
        E::UsageLimit(_) => C::UsageLimit,
        E::Network(_) => C::Network,
        E::Protocol(_) => C::BadOutput,
        E::Overloaded(_) | E::Server { .. } | E::Request { .. } | E::Cancelled => C::Other,
    }
}

/// Starts the background uploader: every five minutes while the app runs.
pub fn spawn_uploader() {
    if GLOBAL.get().is_none() {
        return;
    }
    tokio::spawn(async {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
        tick.tick().await; // the first tick is immediate; skip it
        loop {
            tick.tick().await;
            if let Err(e) = flush().await {
                tracing::debug!("uploading usage events: {e:#}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pathfinder_schema::{Feature, Surface};

    fn test_telemetry(consent: Consent) -> Telemetry {
        let db = Connection::open_in_memory().unwrap();
        create_table(&db).unwrap();
        Telemetry {
            db: Mutex::new(db),
            consent: Mutex::new(consent),
            session: Mutex::new(uuid::Uuid::new_v4().to_string()),
            locale: Mutex::new("de".into()),
        }
    }

    #[test]
    fn envelopes_are_valid_and_to_the_minute() {
        let t = test_telemetry(Consent::default());
        let e = envelope(
            &t,
            Event::Ui {
                feature: Feature::Copy,
            },
        );
        e.validate().unwrap();
        assert_eq!(e.at % 60, 0);
        assert_eq!(e.locale, "de");
    }

    #[test]
    fn the_outbox_keeps_order_and_forgets_what_was_sent() {
        let t = test_telemetry(Consent::default());
        let db = t.db.lock().unwrap();
        for _ in 0..3 {
            push(
                &db,
                &envelope(
                    &t,
                    Event::SessionStart {
                        surface: Surface::Tui,
                    },
                ),
            )
            .unwrap();
        }
        let q = queued(&db, 2).unwrap();
        assert_eq!(q.len(), 2);
        sent_up_to(&db, q[1].0).unwrap();
        assert_eq!(queued(&db, 10).unwrap().len(), 1);
    }

    #[test]
    fn collecting_needs_consent_enrolment_and_the_current_notice() {
        let mut c = Consent::default();
        assert!(!c.collecting());
        c.events = true;
        assert!(!c.collecting(), "not enrolled");
        c.enrolled = true;
        assert!(c.collecting());
        c.notice = pathfinder_schema::NOTICE_VERSION - 1;
        assert!(!c.collecting(), "consent was for an older notice");
    }
}
