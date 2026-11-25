//! The HTTP API. Everything under `/v1` except enrolment needs an install
//! token (`Authorization: Bearer …`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Form, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use pathfinder_schema::{
    Batch, Consent, Enroll, Enrolled, LinkPoll, LinkStart, LinkStarted, NOTICE_VERSION,
    SharedThread,
};
use serde::Deserialize;
use tower_http::services::ServeDir;

use crate::access::Access;
use crate::db::{Db, Install, LINK_SECS, LinkState};

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
    /// Who is signed in on `/join`; without it, browser sign-in is off.
    pub access: Option<Access>,
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
        .route("/v1/link", post(link_start))
        .route("/v1/link/poll", post(link_poll))
        .route("/join", get(join_page).post(join_approve))
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

async fn link_start(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
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
    match state
        .db
        .start_link(&req.install, req.notice, req.events, req.threads)
    {
        Ok(Some((code, secret))) => Json(LinkStarted {
            code,
            secret,
            expires_in: LINK_SECS as u64,
        })
        .into_response(),
        Ok(None) => err(
            StatusCode::TOO_MANY_REQUESTS,
            "too many sign-ins at once; try again in a few minutes",
        ),
        Err(e) => internal(e),
    }
}

async fn link_poll(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<LinkPoll>(&body) else {
        return err(StatusCode::BAD_REQUEST, "bad poll");
    };
    match state.db.poll_link(req.secret.trim()) {
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

async fn join_check(state: &AppState, headers: &HeaderMap, code: &str) -> Join {
    let Some(access) = &state.access else {
        return Join::Page(
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "<p>Signing in with the browser isn’t set up yet. Ask Alper for an invite code.</p>"
                .into(),
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
    match state.db.is_allowed(&email) {
        Ok(true) => {}
        Ok(false) => {
            tracing::info!("a sign-in from an address that isn't on the list");
            return Join::Page(
                StatusCode::FORBIDDEN,
                "Not on the list yet",
                format!(
                    "{you}<p>Pathfinder is invite-only for now, and this address isn’t on the list. \
                     Ask Alper to add it, then reload this page.</p>"
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
                "{you}<p>You’re on the list. Now run <code>harness tui</code> in a terminal: \
                 it opens this page again with a code to link your computer.</p>\
                 <p>Not installed yet? <code>curl -fsSL https://pathfinder.lprsrts.com/install.sh | sh</code></p>"
            ),
        );
    }
    match state.db.link(code) {
        Ok(Some(link)) => Join::Ready { email, link },
        Ok(None) => Join::Page(
            StatusCode::GONE,
            "This code has expired",
            format!(
                "{you}<p>Codes last 15 minutes. Start again in Pathfinder and it opens a new one.</p>"
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
         (<code>harness data</code>). Linking also turns on the shared gate. \
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
    let (email, link) = match join_check(&state, &headers, &form.code).await {
        Join::Page(status, title, body) => return page(status, title, &body),
        Join::Ready { email, link } => (email, link),
    };
    match state.db.approve_link(&link.code, &email) {
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
</main></body></html>"#,
        title = esc(title),
    );
    (status, [(header::CACHE_CONTROL, "no-store")], Html(html)).into_response()
}
