//! The HTTP API. Everything under `/v1` except signing in needs an install
//! token (`Authorization: Bearer …`), checked before the body is read.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Form, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use pathfinder_schema::{
    BugReport, Consent, Enroll, Enrolled, Envelope, LinkPoll, LinkStart, LinkStarted,
    NOTICE_VERSION, SharedThread,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tower_http::services::ServeDir;

use crate::access::Access;
use crate::db::{Db, GateCheck, Install, LINK_SECS, LinkState};
use crate::limit::{RateLimiter, client_addr};

/// Most events one upload may carry.
const MAX_BATCH: usize = 1_000;

/// Body limits: the default, then per route.
const BODY_DEFAULT: usize = 64 << 10;
const BODY_EVENTS: usize = 2 << 20;
const BODY_THREAD: usize = 8 << 20;
const BODY_GATE: usize = 128 << 10;
const BODY_REPORT: usize = 256 << 10;

/// Requests handled at once; past it, 503 with Retry-After.
const MAX_IN_FLIGHT: usize = 64;
/// Gate checks forwarded at once.
const MAX_GATE_IN_FLIGHT: usize = 16;

/// Requests per address per minute on the endpoints anyone can reach.
const PER_MINUTE_ENROLL: u32 = 10;
const PER_MINUTE_LINK: u32 = 10;
/// A waiting app checks every 2 seconds.
const PER_MINUTE_POLL: u32 = 60;
const PER_MINUTE_JOIN: u32 = 30;

/// A gate request: a Jev question set about one piece of text.
const GATE_MAX_STATE: usize = 100 << 10;
const GATE_MAX_QUESTIONS: usize = 8;
const GATE_MAX_QUESTION: usize = 4 << 10;

pub struct Settings {
    /// TypeSafe's key; without it the gate proxy answers 503.
    pub typesafe_key: Option<String>,
    pub typesafe_base: String,
    /// The only model the gate proxy forwards to.
    pub gate_model: String,
    /// Gate checks per person per day, across all their installs.
    pub gate_cap: u32,
    /// Gate checks per day for everyone together.
    pub gate_global_cap: u32,
    /// How long to wait for TypeSafe; below the app's own 10 s.
    pub gate_timeout: Duration,
    /// This service's public address, for the sign-in form's origin check.
    pub public_url: String,
    /// Who runs this collector, as people know them ("Ask … to add you").
    pub operator: String,
    /// `install.sh`, `privacy`, `releases/…`.
    pub public_dir: PathBuf,
}

pub struct AppState {
    pub db: Arc<Db>,
    pub settings: Settings,
    pub http: reqwest::Client,
    /// Who is signed in on `/join`; without it, browser sign-in is off.
    pub access: Option<Access>,
    pub limiter: RateLimiter,
    in_flight: Semaphore,
    gate_in_flight: Semaphore,
}

impl AppState {
    pub fn new(db: Db, settings: Settings, http: reqwest::Client, access: Option<Access>) -> Self {
        Self {
            db: Arc::new(db),
            settings,
            http,
            access,
            limiter: RateLimiter::default(),
            in_flight: Semaphore::new(MAX_IN_FLIGHT),
            gate_in_flight: Semaphore::new(MAX_GATE_IN_FLIGHT),
        }
    }

    /// Runs a database call off the async workers.
    async fn db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Db) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || f(&db)).await?
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    let public = ServeDir::new(&state.settings.public_dir).append_index_html_on_directories(true);
    let authed = Router::new()
        .route(
            "/v1/events",
            post(events).layer(DefaultBodyLimit::max(BODY_EVENTS)),
        )
        .route("/v1/consent", put(consent))
        .route(
            "/v1/threads",
            post(threads).layer(DefaultBodyLimit::max(BODY_THREAD)),
        )
        .route(
            "/v1/reports",
            post(reports).layer(DefaultBodyLimit::max(BODY_REPORT)),
        )
        .route(
            "/v1/typesafe/systemone",
            post(gate_proxy).layer(DefaultBodyLimit::max(BODY_GATE)),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_install,
        ));
    let erasure =
        Router::new()
            .route("/v1/me", delete(erase))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                require_install_even_stopped,
            ));
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/version", get(version))
        .route("/v1/enroll", post(enroll))
        .route("/v1/link", post(link_start))
        .route("/v1/link/poll", post(link_poll))
        .route("/join", get(join_page).post(join_approve))
        .merge(authed)
        .merge(erasure)
        .fallback_service(public)
        .layer(middleware::from_fn(cache_headers))
        .layer(middleware::from_fn_with_state(state.clone(), count_fetches))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            limit_in_flight,
        ))
        .layer(DefaultBodyLimit::max(BODY_DEFAULT))
        .with_state(state)
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

fn internal(e: anyhow::Error) -> Response {
    tracing::error!("{e:#}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

fn busy(retry_after_secs: u32) -> Response {
    let mut res = err(StatusCode::SERVICE_UNAVAILABLE, "busy; try again shortly");
    res.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs));
    res
}

fn too_many() -> Response {
    let mut res = err(
        StatusCode::TOO_MANY_REQUESTS,
        "too many requests; slow down",
    );
    res.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(60u32));
    res
}

async fn limit_in_flight(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let Ok(_permit) = state.in_flight.try_acquire() else {
        return busy(2);
    };
    next.run(req).await
}

/// How long caches may keep the installer and the top-level release
/// manifest: a minute fresh, and a week while this service can't be reached,
/// so installs and updates keep working through an outage. (Cloudflare's
/// free plan may hold them longer; apps verify each version's own manifest,
/// so a slow cache delays an update but can't break one.)
const SHORT_LIVED: &str = "public, max-age=60, stale-if-error=604800";
const SITE_IMAGE: &str = "public, max-age=300, stale-if-error=604800";
/// Versioned release files never change.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

async fn cache_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut res = next.run(req).await;
    if !res.status().is_success() {
        return res;
    }
    let policy = match path.as_str() {
        "/install.sh" | "/releases/latest.json" | "/releases/latest.json.sig" => Some(SHORT_LIVED),
        // Everything under a version's folder: archives, its manifest.
        p if p
            .strip_prefix("/releases/")
            .is_some_and(|rest| rest.contains('/')) =>
        {
            Some(IMMUTABLE)
        }
        // The site's images: a change shows up within minutes.
        p if !p.starts_with("/releases/") && (p.ends_with(".svg") || p.ends_with(".png")) => {
            Some(SITE_IMAGE)
        }
        _ => None,
    };
    if let Some(policy) = policy {
        res.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    }
    res
}

/// Counts downloads of the installer and releases by app version (from the
/// user agent), so adoption shows without any install id.
async fn count_fetches(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let what = match path {
        "/install.sh" => Some("install.sh"),
        "/releases/latest.json" => Some("latest.json"),
        p if p.starts_with("/releases/") && p.ends_with(".tar.gz") => Some("release"),
        _ => None,
    };
    let version = app_version(req.headers());
    let res = next.run(req).await;
    if let Some(what) = what
        && res.status().is_success()
        && let Err(e) = state.db(move |db| db.count_fetch(what, &version)).await
    {
        tracing::warn!("counting a download: {e:#}");
    }
    res
}

/// The app version in a `pathfinder/<version>` user agent, else "other".
fn app_version(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .and_then(|ua| ua.strip_prefix("pathfinder/"))
        .filter(|v| {
            v.len() <= 16
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        })
        .unwrap_or("other")
        .to_string()
}

async fn healthz(State(state): State<Arc<AppState>>) -> Response {
    match state.db(|db| db.gate_enabled().map(|_| ())).await {
        Ok(()) => "ok".into_response(),
        Err(e) => {
            tracing::error!("health check: {e:#}");
            err(StatusCode::SERVICE_UNAVAILABLE, "database unavailable")
        }
    }
}

/// What this collector accepts and where its source is (AGPL §13).
async fn version() -> Response {
    Json(serde_json::json!({
        "schema": pathfinder_schema::VERSION,
        "notice": NOTICE_VERSION,
        "commit": std::env::var("COLLECTOR_COMMIT").unwrap_or_else(|_| "unknown".into()),
        "source": env!("CARGO_PKG_REPOSITORY"),
    }))
    .into_response()
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
}

/// The install behind the bearer token, before the body is read: 401 if
/// unknown, 410 if stopped (unless `even_stopped`).
async fn install_from(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    even_stopped: bool,
) -> Result<Install, Box<Response>> {
    let token =
        bearer(headers).ok_or_else(|| Box::new(err(StatusCode::UNAUTHORIZED, "no token")))?;
    match state.db(move |db| db.install_for(&token)).await {
        Ok(Some((install, stopped))) if even_stopped || !stopped => Ok(install),
        Ok(Some(_)) => Err(Box::new(err(
            StatusCode::GONE,
            "collection from this install has stopped",
        ))),
        Ok(None) => Err(Box::new(err(StatusCode::UNAUTHORIZED, "unknown token"))),
        Err(e) => Err(Box::new(internal(e))),
    }
}

async fn require_install(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    match install_from(&state, req.headers(), false).await {
        Ok(install) => {
            req.extensions_mut().insert(install);
            next.run(req).await
        }
        Err(res) => *res,
    }
}

async fn require_install_even_stopped(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    match install_from(&state, req.headers(), true).await {
        Ok(install) => {
            req.extensions_mut().insert(install);
            next.run(req).await
        }
        Err(res) => *res,
    }
}

fn is_install_id(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

async fn enroll(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    if !state
        .limiter
        .allow(&client_addr(&headers), "enroll", PER_MINUTE_ENROLL)
    {
        return too_many();
    }
    let Ok(req) = serde_json::from_slice::<Enroll>(&body) else {
        return err(StatusCode::BAD_REQUEST, "bad enrolment");
    };
    if !is_install_id(&req.install) || req.invite.len() > 64 || req.notice > NOTICE_VERSION {
        return err(StatusCode::BAD_REQUEST, "bad enrolment");
    }
    match state
        .db(move |db| {
            db.enroll(
                req.invite.trim(),
                &req.install,
                req.notice,
                req.events,
                req.threads,
            )
        })
        .await
    {
        Ok(Some(token)) => Json(Enrolled { token }).into_response(),
        Ok(None) => err(StatusCode::FORBIDDEN, "unknown or used invite code"),
        Err(e) => internal(e),
    }
}

/// Stores the valid events of a batch and names the ids it refused, so one
/// event an older collector doesn't know never costs the rest.
async fn events(
    State(state): State<Arc<AppState>>,
    Extension(install): Extension<Install>,
    body: Bytes,
) -> Response {
    // Consent withdrawn here (or never given): tell the app to stop.
    if !install.events {
        return err(StatusCode::GONE, "usage events are off for this install");
    }
    #[derive(Deserialize)]
    struct RawBatch {
        events: Vec<serde_json::Value>,
    }
    let batch: RawBatch = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, &format!("bad batch: {e}")),
    };
    if batch.events.len() > MAX_BATCH {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "too many events");
    }
    let mut valid = Vec::new();
    let mut rejected = Vec::new();
    for raw in batch.events {
        let id = raw["id"].as_str().unwrap_or("").to_string();
        match serde_json::from_value::<Envelope>(raw) {
            Ok(e) if e.validate().is_ok() => valid.push(e),
            _ => rejected.push(id),
        }
    }
    if !rejected.is_empty() {
        tracing::warn!(
            "refused {} event(s) this collector doesn't know",
            rejected.len()
        );
    }
    let id = install.install.clone();
    match state.db(move |db| db.add_events(&id, &valid)).await {
        Ok(n) => Json(serde_json::json!({ "stored": n, "rejected": rejected })).into_response(),
        Err(e) => internal(e),
    }
}

async fn consent(
    State(state): State<Arc<AppState>>,
    Extension(install): Extension<Install>,
    body: Bytes,
) -> Response {
    let Ok(c) = serde_json::from_slice::<Consent>(&body) else {
        return err(StatusCode::BAD_REQUEST, "bad consent");
    };
    if c.notice > NOTICE_VERSION {
        return err(StatusCode::BAD_REQUEST, "unknown notice");
    }
    match state
        .db(move |db| db.set_consent(&install.install, c.notice, c.events, c.threads))
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

async fn erase(
    State(state): State<Arc<AppState>>,
    Extension(install): Extension<Install>,
) -> Response {
    match state.db(move |db| db.erase(&install.install)).await {
        Ok(()) => {
            tracing::info!("erased an install on request");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => internal(e),
    }
}

async fn threads(
    State(state): State<Arc<AppState>>,
    Extension(install): Extension<Install>,
    body: Bytes,
) -> Response {
    if !install.threads {
        return err(
            StatusCode::FORBIDDEN,
            "sharing threads is off for this install",
        );
    }
    let thread: SharedThread = match serde_json::from_slice(&body) {
        Ok(t) => t,
        Err(e) => return err(StatusCode::BAD_REQUEST, &format!("bad thread: {e}")),
    };
    if !is_install_id(&thread.id) || thread.note.len() > 2_000 {
        return err(StatusCode::BAD_REQUEST, "bad thread");
    }
    match state
        .db(move |db| db.add_thread(&install.install, &thread))
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(
            StatusCode::INSUFFICIENT_STORAGE,
            "this install has shared the most threads it can; delete some with `pathfinder data delete`",
        ),
        Err(e) => internal(e),
    }
}

/// A bug report needs no data consent: sending it, after seeing it in
/// full, is the consent.
async fn reports(
    State(state): State<Arc<AppState>>,
    Extension(install): Extension<Install>,
    body: Bytes,
) -> Response {
    let report: BugReport = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_REQUEST, &format!("bad report: {e}")),
    };
    if !is_install_id(&report.id)
        || report.note.len() > BugReport::MAX_NOTE
        || report.details.len() > BugReport::MAX_DETAILS
    {
        return err(StatusCode::BAD_REQUEST, "bad report");
    }
    match state
        .db(move |db| db.add_report(&install.install, &report))
        .await
    {
        Ok(true) => {
            tracing::info!("a bug report arrived");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => err(
            StatusCode::INSUFFICIENT_STORAGE,
            "too many reports from this install",
        ),
        Err(e) => internal(e),
    }
}

/// What the gate proxy forwards: exactly a Jev request, nothing else.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GateRequest {
    model: String,
    state: String,
    /// Questions by name.
    questions: serde_json::Map<String, serde_json::Value>,
}

impl GateRequest {
    fn check(&self, model: &str) -> Result<(), &'static str> {
        if self.model != model {
            return Err("unsupported model");
        }
        if self.state.len() > GATE_MAX_STATE {
            return Err("text too long");
        }
        if self.questions.is_empty() || self.questions.len() > GATE_MAX_QUESTIONS {
            return Err("bad number of questions");
        }
        if self.questions.iter().any(|(name, q)| {
            name.len() > 64 || !q.is_object() || q.to_string().len() > GATE_MAX_QUESTION
        }) {
            return Err("bad question");
        }
        Ok(())
    }
}

/// Forwards a gate check to TypeSafe with the collector's key, within the
/// person's and everyone's daily budget. Nothing of the request is stored
/// or logged, only counts.
async fn gate_proxy(
    State(state): State<Arc<AppState>>,
    Extension(install): Extension<Install>,
    body: Bytes,
) -> Response {
    let s = &state.settings;
    let Some(key) = s.typesafe_key.clone() else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "the gate service isn't set up",
        );
    };
    let req: GateRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return err(StatusCode::BAD_REQUEST, "not a gate request"),
    };
    if let Err(why) = req.check(&s.gate_model) {
        return err(StatusCode::BAD_REQUEST, why);
    }
    let (per_person, global) = (s.gate_cap, s.gate_global_cap);
    let id = install.install.clone();
    match state
        .db(move |db| db.gate_check(&id, per_person, global))
        .await
    {
        Ok(GateCheck::Allowed) => {}
        Ok(GateCheck::Off) => {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "the shared gate is paused; your own TypeSafe key still works",
            );
        }
        Ok(GateCheck::PersonCapped) => {
            return err(
                StatusCode::TOO_MANY_REQUESTS,
                "today's gate checks are used up; it resets at midnight UTC",
            );
        }
        Ok(GateCheck::GlobalCapped) => {
            tracing::warn!("the shared gate reached its daily budget ({global})");
            return err(
                StatusCode::TOO_MANY_REQUESTS,
                "today's shared gate checks are used up; it resets at midnight UTC",
            );
        }
        Err(e) => return internal(e),
    }
    if let Ok(today) = state.db(|db| db.gate_today()).await
        && (today == global / 2 || today == global * 8 / 10)
    {
        tracing::warn!("the shared gate has used {today} of its {global} daily checks");
    }
    let Ok(_permit) = state.gate_in_flight.try_acquire() else {
        return busy(1);
    };
    let started = Instant::now();
    let res = state
        .http
        .post(format!("{}/systemone", s.typesafe_base))
        .bearer_auth(key)
        .timeout(s.gate_timeout)
        .json(&req)
        .send()
        .await;
    let ms = started.elapsed().as_millis();
    match res {
        Ok(r) => {
            let status =
                StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            tracing::info!(status = status.as_u16(), ms, "gate check");
            if !status.is_success() {
                state.db(|db| db.gate_upstream_error()).await.ok();
            }
            let bytes = r.bytes().await.unwrap_or_default();
            (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
        }
        Err(e) => {
            tracing::warn!(ms, "forwarding a gate check: {e}");
            state.db(|db| db.gate_upstream_error()).await.ok();
            err(StatusCode::BAD_GATEWAY, "the gate service didn't answer")
        }
    }
}

async fn link_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !state
        .limiter
        .allow(&client_addr(&headers), "link", PER_MINUTE_LINK)
    {
        return too_many();
    }
    let Ok(req) = serde_json::from_slice::<LinkStart>(&body) else {
        return err(StatusCode::BAD_REQUEST, "bad sign-in request");
    };
    if !is_install_id(&req.install) || req.notice > NOTICE_VERSION {
        return err(StatusCode::BAD_REQUEST, "bad sign-in request");
    }
    if state.access.is_none() {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "signing in with the browser isn't set up; use an invite code",
        );
    }
    let version = app_version(&headers);
    match state
        .db(move |db| {
            let started = db.start_link(&req.install, req.notice, req.events, req.threads)?;
            if started.is_some() {
                db.count_fetch("sign-in started", &version)?;
            }
            Ok(started)
        })
        .await
    {
        Ok(Some((code, secret))) => Json(LinkStarted {
            code,
            secret,
            expires_in: LINK_SECS as u64,
        })
        .into_response(),
        Ok(None) => busy(60),
        Err(e) => internal(e),
    }
}

async fn link_poll(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !state
        .limiter
        .allow(&client_addr(&headers), "poll", PER_MINUTE_POLL)
    {
        return too_many();
    }
    let Ok(req) = serde_json::from_slice::<LinkPoll>(&body) else {
        return err(StatusCode::BAD_REQUEST, "bad poll");
    };
    match state.db(move |db| db.poll_link(req.secret.trim())).await {
        Ok(LinkState::Waiting) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "status": "waiting" })),
        )
            .into_response(),
        Ok(LinkState::Gone) => err(StatusCode::GONE, "this sign-in expired; start again"),
        Ok(LinkState::Done(token)) => Json(Enrolled { token }).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct JoinForm {
    #[serde(default)]
    code: String,
}

/// The page's state for one request: who is signed in, and their sign-in.
enum Join {
    Page(StatusCode, &'static str, String),
    Ready {
        email: String,
        link: crate::db::Link,
    },
}

async fn join_check(state: &Arc<AppState>, headers: &HeaderMap, code: &str) -> Join {
    let operator = esc(&state.settings.operator);
    if !state
        .limiter
        .allow(&client_addr(headers), "join", PER_MINUTE_JOIN)
    {
        return Join::Page(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many tries",
            "<p>Wait a minute, then reload this page.</p>".into(),
        );
    }
    let Some(access) = &state.access else {
        return Join::Page(
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            format!(
                "<p>Signing in with the browser isn’t set up yet. Ask {operator} for an invite code.</p>"
            ),
        );
    };
    let email = match access.email(headers).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("a /join request without a valid sign-in: {e:#}");
            return Join::Page(
                StatusCode::UNAUTHORIZED,
                "Not signed in",
                "<p>Your sign-in couldn’t be confirmed. Reload this page to sign in again.</p>"
                    .into(),
            );
        }
    };
    let you = format!("<p class=\"meta\">Signed in as {}</p>", esc(&email));
    let checked = email.clone();
    match state.db(move |db| db.is_allowed(&checked)).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::info!("a sign-in from an address that isn't on the list");
            return Join::Page(
                StatusCode::FORBIDDEN,
                "Not on the list yet",
                format!(
                    "{you}<p>Pathfinder is invite-only for now, and this address isn’t on the list. \
                     Ask {operator} to add it, then reload this page.</p>"
                ),
            );
        }
        Err(e) => {
            tracing::error!("checking the list: {e:#}");
            return Join::Page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong",
                format!("{you}<p>Please try again in a minute.</p>"),
            );
        }
    }
    if code.trim().is_empty() {
        return Join::Page(
            StatusCode::OK,
            "Start from Pathfinder",
            format!(
                "{you}<p>You’re on the list. Now run <code>pathfinder</code> in a terminal: \
                 it opens this page again with a code to link your computer.</p>\
                 <p>Not installed yet? <code>curl -fsSL {base}/install.sh | sh</code></p>",
                base = esc(&state.settings.public_url),
            ),
        );
    }
    let code = code.to_string();
    match state.db(move |db| db.link(&code)).await {
        Ok(Some(link)) => Join::Ready { email, link },
        Ok(None) => Join::Page(
            StatusCode::GONE,
            "This code has expired",
            format!(
                "{you}<p>Codes last {} minutes. Start again in Pathfinder and it opens a new one.</p>",
                LINK_SECS / 60
            ),
        ),
        Err(e) => {
            tracing::error!("looking up a sign-in: {e:#}");
            Join::Page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong",
                format!("{you}<p>Please try again in a minute.</p>"),
            )
        }
    }
}

async fn join_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<JoinForm>,
) -> Response {
    let (email, link) = match join_check(&state, &headers, &q.code).await {
        Join::Page(status, title, body) => return page(status, title, &body),
        Join::Ready { email, link } => (email, link),
    };
    let yes = |b: bool| if b { "yes" } else { "no" };
    let body = format!(
        "<p class=\"meta\">Signed in as {email}</p>\
         <p>Your terminal should show this code:</p>\
         <p class=\"code\">{code}</p>\
         <p>Only continue if it matches. Then Pathfinder on that computer is linked to {email}.</p>\
         <table><tr><td>Usage events</td><td>{events}</td></tr>\
         <tr><td>Sharing threads you choose</td><td>{threads}</td></tr></table>\
         <p class=\"meta\">You chose these in Pathfinder, and can change them there any time \
         (<code>pathfinder data</code>). Linking also turns on the shared gate. \
         <a href=\"/privacy/\">Privacy notice</a></p>\
         <form method=\"post\" action=\"/join\"><input type=\"hidden\" name=\"code\" value=\"{code}\">\
         <button type=\"submit\">Link this computer</button></form>",
        email = esc(&email),
        code = esc(&link.code),
        events = yes(link.events),
        threads = yes(link.threads),
    );
    page(StatusCode::OK, "Link Pathfinder", &body)
}

async fn join_approve(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<JoinForm>,
) -> Response {
    // The form only counts when posted from this service's own page.
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin != Some(state.settings.public_url.as_str()) {
        tracing::warn!("a sign-in form posted from another origin");
        return page(
            StatusCode::FORBIDDEN,
            "Start again",
            "<p>That form didn’t come from this page. Start again in Pathfinder.</p>",
        );
    }
    let (email, link) = match join_check(&state, &headers, &form.code).await {
        Join::Page(status, title, body) => return page(status, title, &body),
        Join::Ready { email, link } => (email, link),
    };
    match state
        .db(move |db| {
            let approved = db.approve_link(&link.code, &email)?;
            if approved {
                db.count_fetch("sign-in approved", "browser")?;
            }
            Ok(approved)
        })
        .await
    {
        Ok(true) => page(
            StatusCode::OK,
            "Linked",
            "<p>Done. Go back to your terminal: Pathfinder carries on there.</p>\
             <p class=\"meta\">You can close this page.</p>",
        ),
        Ok(false) => page(
            StatusCode::GONE,
            "This code has expired",
            "<p>Start again in Pathfinder and it opens a new one.</p>",
        ),
        Err(e) => internal(e),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn page(status: StatusCode, title: &str, body: &str) -> Response {
    let html = format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<link rel="icon" type="image/png" sizes="32x32" href="/icon-32.png">
<link rel="apple-touch-icon" href="/icon-180.png">
<title>{title} · Pathfinder</title>
<style>
  :root {{ color-scheme: light dark; --fg: #1d1d1f; --bg: #fff; --muted: #6e6e73; --rule: #e5e5ea; --accent: #0a66d8; }}
  @media (prefers-color-scheme: dark) {{ :root {{ --fg: #f5f5f7; --bg: #111; --muted: #a1a1a6; --rule: #333; --accent: #4c9bff; }} }}
  body {{ margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
  main {{ max-width: 34rem; margin: 0 auto; padding: 3rem 1rem 4rem; }}
  h1 {{ font-size: 1.5rem; margin: 0 0 1rem; }}
  .meta {{ color: var(--muted); font-size: .95rem; }}
  .code {{ font: 600 2rem/1.2 ui-monospace, SFMono-Regular, Menlo, monospace; letter-spacing: .08em; margin: .5rem 0 1rem; }}
  table {{ border-collapse: collapse; width: 100%; margin: 1rem 0; }}
  td {{ padding: .4rem .75rem .4rem 0; border-bottom: 1px solid var(--rule); }}
  td:last-child {{ text-align: right; }}
  code {{ font: .9em ui-monospace, SFMono-Regular, Menlo, monospace; overflow-wrap: anywhere; }}
  button {{ font: inherit; font-weight: 600; color: #fff; background: var(--accent); border: 0; border-radius: 8px; padding: .7rem 1.2rem; margin-top: 1rem; cursor: pointer; width: 100%; }}
  a {{ color: var(--accent); }}
</style></head>
<body><main><h1>{title}</h1>
{body}
<p class="meta"><a href="/">Pathfinder</a> · <a href="/privacy/">Privacy</a> · <a href="{source}">Source</a></p>
</main></body></html>"#,
        title = esc(title),
        source = env!("CARGO_PKG_REPOSITORY"),
    );
    (status, [(header::CACHE_CONTROL, "no-store")], Html(html)).into_response()
}
