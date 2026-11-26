//! Usage events, sent only with consent. The events themselves are
//! defined in `pathfinder-schema`: numbers, flags and closed sets of
//! names, with no message text, file names or paths.
//!
//! Events go to a local outbox first (a table in the app's database) and
//! are uploaded in batches, so a collector that is down loses nothing.
//! Nothing is written at all unless you said yes to usage events and the
//! install is enrolled; `PATHFINDER_TELEMETRY=off` always wins.

pub mod consent;
pub mod share;
mod upload;

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use pathfinder_schema::{Arch, Envelope, Event, Os, VERSION};
use rusqlite::{Connection, params};

pub use consent::Consent;
pub use upload::{
    SignIn, collector_url, delete_remote, enroll, finish_sign_in, flush, report_bug,
    set_remote_consent, share, start_sign_in,
};

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
        crate::envvar::get("PATHFINDER_TELEMETRY")
            .as_deref()
            .map(str::trim),
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

/// Reloads the consent file (after `pathfinder data` or the consent screen
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
/// Like [`emit`], but gives up instead of waiting for the queue (from a
/// panic hook, where the queue's lock may be held or poisoned).
pub fn try_emit(event: Event) {
    let Some(t) = GLOBAL.get() else { return };
    let collecting = t.consent.try_lock().is_ok_and(|c| c.collecting());
    if !collecting || env_off() {
        return;
    }
    let env = envelope(t, event);
    if env.validate().is_err() {
        return;
    }
    if let Ok(db) = t.db.try_lock() {
        push(&db, &env).ok();
    }
}

/// The first-time step an event marks, if any.
fn milestone_of(event: &Event) -> Option<pathfinder_schema::Milestone> {
    use pathfinder_schema::{Feature, Milestone, Step};
    match event {
        Event::Turn {
            step: Step::Map, ..
        } => Some(Milestone::FirstMap),
        Event::Turn {
            step: Step::Candidate,
            ..
        } => Some(Milestone::FirstCandidate),
        Event::Turn { .. } => Some(Milestone::FirstTurn),
        Event::Check { .. } => Some(Milestone::FirstCheck),
        Event::Ui {
            feature: Feature::Share,
        } => Some(Milestone::FirstShare),
        _ => None,
    }
}

/// Records `step` as reached; true the first time (per install).
fn first_time(step: pathfinder_schema::Milestone) -> bool {
    let path = crate::paths::state_dir().join("milestones.json");
    let mut seen: Vec<pathfinder_schema::Milestone> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    if seen.contains(&step) {
        return false;
    }
    seen.push(step);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(&path, serde_json::to_string(&seen).unwrap_or_default()).is_ok()
}

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
    let milestone = milestone_of(&env.event);
    if let Err(e) = push(&t.db.lock().unwrap(), &env) {
        tracing::warn!("queueing a usage event: {e:#}");
        return;
    }
    if let Some(step) = milestone
        && first_time(step)
    {
        let env = envelope(t, Event::Milestone { step });
        push(&t.db.lock().unwrap(), &env).ok();
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

/// The outbox, for `pathfinder data` (which runs without `init`).
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
/// A failed turn's error as a usage-event code; None when you stopped it.
pub fn error_code(err: &anyhow::Error) -> Option<pathfinder_schema::ErrorCode> {
    use pathfinder_schema::ErrorCode as C;
    if let Some(e) = err.downcast_ref::<crate::llm::LlmError>() {
        return match e {
            crate::llm::LlmError::Cancelled => None,
            e => Some(llm_error_code(e)),
        };
    }
    if let Some(e) = err.downcast_ref::<reqwest::Error>() {
        return Some(if e.is_timeout() {
            C::Timeout
        } else {
            C::Network
        });
    }
    let text = format!("{err:#}");
    if text == "cancelled" || text == "stopped" {
        return None;
    }
    Some(if text.contains("timed out") {
        C::Timeout
    } else {
        C::Other
    })
}

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

/// How often queued usage events are uploaded while the app runs.
pub const UPLOAD_EVERY: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// How long quitting may wait for the last upload.
pub const FLUSH_ON_EXIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Starts the background uploader, every [`UPLOAD_EVERY`] while the app runs.
pub fn spawn_uploader() {
    if GLOBAL.get().is_none() {
        return;
    }
    tokio::spawn(async {
        let mut tick = tokio::time::interval(UPLOAD_EVERY);
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
