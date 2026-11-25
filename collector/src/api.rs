//! The HTTP API. Everything under `/v1` except enrolment needs an install
//! token (`Authorization: Bearer …`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use pathfinder_schema::{Batch, Consent, Enroll, Enrolled, NOTICE_VERSION, SharedThread};
use tower_http::services::ServeDir;

use crate::db::{Db, Install};

/// Most events one upload may carry.
const MAX_BATCH: usize = 1_000;

pub struct AppState {
    pub db: Db,
    /// TypeSafe's key; without it the gate proxy answers 503.
    pub typesafe_key: Option<String>,
    pub typesafe_base: String,
    /// Gate checks per install per day.
    pub gate_cap: u32,
    pub http: reqwest::Client,
    /// `install.sh`, `privacy`, `releases/…`.
    pub public_dir: PathBuf,
}

pub fn router(state: Arc<AppState>) -> Router {
    let public = ServeDir::new(&state.public_dir).append_index_html_on_directories(true);
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/enroll", post(enroll))
        .route(
            "/v1/events",
            post(events).layer(DefaultBodyLimit::max(2 << 20)),
        )
        .route("/v1/consent", put(consent))
        .route("/v1/me", delete(erase))
        .route(
            "/v1/threads",
            post(threads).layer(DefaultBodyLimit::max(8 << 20)),
        )
        .route(
            "/v1/typesafe/systemone",
            post(gate_proxy).layer(DefaultBodyLimit::max(1 << 20)),
        )
        .fallback_service(public)
        .layer(DefaultBodyLimit::max(64 << 10))
        .with_state(state)
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

/// The install behind the bearer token: 401 if unknown, 410 if stopped.
fn authed(state: &AppState, headers: &HeaderMap) -> Result<Install, Response> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "no token"))?;
    match state.db.install_for(token.trim()) {
        Ok(Some((_, true))) => Err(err(
            StatusCode::GONE,
            "collection from this install has stopped",
        )),
        Ok(Some((install, false))) => Ok(install),
        Ok(None) => Err(err(StatusCode::UNAUTHORIZED, "unknown token")),
        Err(e) => {
            tracing::error!("looking up a token: {e:#}");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "database error"))
        }
    }
}

fn internal(e: anyhow::Error) -> Response {
    tracing::error!("{e:#}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

fn is_install_id(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

async fn enroll(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<Enroll>(&body) else {
        return err(StatusCode::BAD_REQUEST, "bad enrolment");
    };
    if !is_install_id(&req.install) || req.invite.len() > 64 || req.notice > NOTICE_VERSION {
        return err(StatusCode::BAD_REQUEST, "bad enrolment");
    }
    match state.db.enroll(
        req.invite.trim(),
        &req.install,
        req.notice,
        req.events,
        req.threads,
    ) {
        Ok(Some(token)) => Json(Enrolled { token }).into_response(),
        Ok(None) => err(StatusCode::FORBIDDEN, "unknown or used invite code"),
        Err(e) => internal(e),
    }
}

async fn events(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    let install = match authed(&state, &headers) {
        Ok(i) => i,
        Err(r) => return r,
    };
    // Consent withdrawn here (or never given): tell the app to stop.
    if !install.events {
        return err(StatusCode::GONE, "usage events are off for this install");
    }
    let batch: Batch = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, &format!("bad batch: {e}")),
    };
    if batch.events.len() > MAX_BATCH {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "too many events");
    }
    if let Some(bad) = batch.events.iter().find_map(|e| e.validate().err()) {
        return err(StatusCode::BAD_REQUEST, &bad.to_string());
    }
    match state.db.add_events(&install.install, &batch.events) {
        Ok(n) => Json(serde_json::json!({ "stored": n })).into_response(),
        Err(e) => internal(e),
    }
}

async fn consent(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    let install = match authed(&state, &headers) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let Ok(c) = serde_json::from_slice::<Consent>(&body) else {
        return err(StatusCode::BAD_REQUEST, "bad consent");
    };
    if c.notice > NOTICE_VERSION {
        return err(StatusCode::BAD_REQUEST, "unknown notice");
    }
    match state
        .db
        .set_consent(&install.install, c.notice, c.events, c.threads)
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

async fn erase(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let install = match authed(&state, &headers) {
        Ok(i) => i,
        // A stopped install may still ask for erasure.
        Err(r) if r.status() == StatusCode::GONE => {
            let token = headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .unwrap_or("");
            match state.db.install_for(token.trim()) {
                Ok(Some((i, _))) => i,
                _ => return r,
            }
        }
        Err(r) => return r,
    };
    match state.db.erase(&install.install) {
        Ok(()) => {
            tracing::info!("erased an install on request");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => internal(e),
    }
}

async fn threads(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    let install = match authed(&state, &headers) {
        Ok(i) => i,
        Err(r) => return r,
    };
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
    match state.db.add_thread(&install.install, &thread) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

/// Forwards a gate check to TypeSafe with the collector's key, within the
/// install's daily cap. Nothing of the request is stored, only the count.
async fn gate_proxy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let install = match authed(&state, &headers) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let Some(key) = &state.typesafe_key else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "the gate service isn't set up",
        );
    };
    match state.db.count_gate_use(&install.install, state.gate_cap) {
        Ok(true) => {}
        Ok(false) => {
            return err(
                StatusCode::TOO_MANY_REQUESTS,
                "today's gate checks are used up; it resets at midnight UTC",
            );
        }
        Err(e) => return internal(e),
    }
    let res = state
        .http
        .post(format!("{}/systemone", state.typesafe_base))
        .bearer_auth(key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await;
    match res {
        Ok(r) => {
            let status =
                StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let bytes = r.bytes().await.unwrap_or_default();
            (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
        }
        Err(e) => {
            tracing::warn!("forwarding a gate check: {e}");
            err(StatusCode::BAD_GATEWAY, "the gate service didn't answer")
        }
    }
}
