//! Pathfinder's collector: usage events from consenting installs, shared
//! threads, the gate proxy, and the installer and releases.
//!
//! ```text
//! collector serve                 run the service
//! collector invite <name>         a single-use invite code for <name>
//! collector stats                 one line per install
//! collector export <dir>          events.jsonl and threads.jsonl
//! collector stop <name|install>   stop collecting from an install
//! collector prune                 delete events past retention
//! ```
//!
//! Settings come from the environment: `COLLECTOR_DB`, `COLLECTOR_PUBLIC`,
//! `COLLECTOR_ADDR`, `TYPESAFE_KEY_FILE`, `TYPESAFE_BASE_URL`,
//! `GATE_DAILY_CAP`.

mod api;
mod db;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn open_db() -> Result<db::Db> {
    let path = PathBuf::from(env_or("COLLECTOR_DB", "/data/collector.sqlite"));
    db::Db::open(&path).with_context(|| format!("opening {}", path.display()))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_ansi(false)
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => serve().await,
        Some("invite") => {
            let name = args.get(1).context("usage: collector invite <name>")?;
            let code = open_db()?.new_invite(name)?;
            println!("{code}");
            Ok(())
        }
        Some("stats") => {
            for line in open_db()?.stats()? {
                println!("{line}");
            }
            Ok(())
        }
        Some("export") => {
            let dir = PathBuf::from(args.get(1).context("usage: collector export <dir>")?);
            let (e, t) = open_db()?.export(&dir)?;
            println!("{e} events and {t} threads written to {}", dir.display());
            Ok(())
        }
        Some("stop") => {
            let who = args
                .get(1)
                .context("usage: collector stop <name|install>")?;
            let n = open_db()?.stop(who)?;
            println!("stopped {n} install(s)");
            Ok(())
        }
        Some("prune") => {
            let n = open_db()?.prune()?;
            println!("deleted {n} events past retention");
            Ok(())
        }
        _ => bail!(
            "usage: collector serve | invite <name> | stats | export <dir> | stop <name|install> | prune"
        ),
    }
}

async fn serve() -> Result<()> {
    let typesafe_key = std::env::var("TYPESAFE_KEY_FILE")
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty());
    if typesafe_key.is_none() {
        tracing::warn!("no TypeSafe key: the gate proxy answers 503");
    }
    let state = Arc::new(api::AppState {
        db: open_db()?,
        typesafe_key,
        typesafe_base: env_or("TYPESAFE_BASE_URL", "https://api.typesafe.ai/v1")
            .trim_end_matches('/')
            .to_string(),
        gate_cap: env_or("GATE_DAILY_CAP", "400").parse().unwrap_or(400),
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?,
        public_dir: PathBuf::from(env_or("COLLECTOR_PUBLIC", "/public")),
    });
    // Retention, daily.
    let pruner = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(86_400));
        loop {
            tick.tick().await;
            match pruner.db.prune() {
                Ok(n) if n > 0 => tracing::info!("pruned {n} events past retention"),
                Ok(_) => {}
                Err(e) => tracing::error!("pruning: {e:#}"),
            }
        }
    });
    let addr = env_or("COLLECTOR_ADDR", "0.0.0.0:8080");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("listening on {addr}");
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use pathfinder_schema::*;
    use tower::ServiceExt;

    const INSTALL: &str = "0b5d0f7e-6c2a-4b8e-9f00-1a2b3c4d5e6f";

    fn state(cap: u32) -> Arc<api::AppState> {
        Arc::new(api::AppState {
            db: db::Db::open_in_memory().unwrap(),
            typesafe_key: None,
            typesafe_base: "http://127.0.0.1:9".into(),
            gate_cap: cap,
            http: reqwest::Client::new(),
            public_dir: std::env::temp_dir(),
        })
    }

    async fn call(
        st: &Arc<api::AppState>,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let res = api::router(st.clone())
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    async fn enrolled(st: &Arc<api::AppState>, events: bool, threads: bool) -> String {
        let code = st.db.new_invite("tester").unwrap();
        let (s, body) = call(
            st,
            "POST",
            "/v1/enroll",
            None,
            serde_json::json!({"invite": code, "install": INSTALL, "notice": NOTICE_VERSION, "events": events, "threads": threads}),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        body["token"].as_str().unwrap().to_string()
    }

    fn event(id: &str) -> serde_json::Value {
        serde_json::to_value(Envelope {
            v: VERSION,
            id: id.into(),
            session: "7c1e2d3f-0000-4000-8000-123456789abc".into(),
            at: 1_790_000_040,
            app: "0.1.0".into(),
            os: Os::Macos,
            arch: Arch::Aarch64,
            locale: "de".into(),
            event: Event::Ui {
                feature: Feature::Copy,
            },
        })
        .unwrap()
    }

    #[tokio::test]
    async fn invites_work_once() {
        let st = state(10);
        let code = st.db.new_invite("a").unwrap();
        let body = serde_json::json!({"invite": code, "install": INSTALL, "notice": 1, "events": true, "threads": false});
        assert_eq!(
            call(&st, "POST", "/v1/enroll", None, body.clone()).await.0,
            StatusCode::OK
        );
        assert_eq!(
            call(&st, "POST", "/v1/enroll", None, body).await.0,
            StatusCode::FORBIDDEN
        );
        let wrong = serde_json::json!({"invite": "nope", "install": INSTALL, "notice": 1, "events": true, "threads": false});
        assert_eq!(
            call(&st, "POST", "/v1/enroll", None, wrong).await.0,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn events_are_stored_once_and_unknown_fields_refused() {
        let st = state(10);
        let token = enrolled(&st, true, false).await;
        let id = "11111111-2222-4333-8444-555555555555";
        let batch = serde_json::json!({"events": [event(id), event(id)]});
        let (s, body) = call(&st, "POST", "/v1/events", Some(&token), batch).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["stored"], 1);
        let mut bad = event("22222222-2222-4333-8444-555555555555");
        bad["event"]["text"] = "my notes".into();
        let (s, _) = call(
            &st,
            "POST",
            "/v1/events",
            Some(&token),
            serde_json::json!({"events": [bad]}),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = call(
            &st,
            "POST",
            "/v1/events",
            Some("forged"),
            serde_json::json!({"events": []}),
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn withdrawn_or_stopped_installs_get_410() {
        let st = state(10);
        let token = enrolled(&st, true, true).await;
        let off = serde_json::json!({"notice": NOTICE_VERSION, "events": false, "threads": false});
        assert_eq!(
            call(&st, "PUT", "/v1/consent", Some(&token), off).await.0,
            StatusCode::NO_CONTENT
        );
        let (s, _) = call(
            &st,
            "POST",
            "/v1/events",
            Some(&token),
            serde_json::json!({"events": []}),
        )
        .await;
        assert_eq!(s, StatusCode::GONE);
        let token2 = {
            let st2 = &st;
            let code = st2.db.new_invite("b").unwrap();
            let (_, b) = call(st2, "POST", "/v1/enroll", None, serde_json::json!({"invite": code, "install": "aaaaaaaa-2222-4333-8444-555555555555", "notice": 1, "events": true, "threads": false})).await;
            b["token"].as_str().unwrap().to_string()
        };
        st.db.stop("b").unwrap();
        let (s, _) = call(
            &st,
            "POST",
            "/v1/events",
            Some(&token2),
            serde_json::json!({"events": []}),
        )
        .await;
        assert_eq!(s, StatusCode::GONE);
    }

    #[tokio::test]
    async fn erasure_removes_everything() {
        let st = state(10);
        let token = enrolled(&st, true, true).await;
        let batch = serde_json::json!({"events": [event("33333333-2222-4333-8444-555555555555")]});
        call(&st, "POST", "/v1/events", Some(&token), batch).await;
        let thread = serde_json::json!({"id": "44444444-2222-4333-8444-555555555555", "app": "0.1.0", "transcript": []});
        assert_eq!(
            call(&st, "POST", "/v1/threads", Some(&token), thread)
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            call(
                &st,
                "DELETE",
                "/v1/me",
                Some(&token),
                serde_json::Value::Null
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        assert!(st.db.stats().unwrap().is_empty());
        let dir = std::env::temp_dir().join(format!("collector-test-{}", std::process::id()));
        assert_eq!(st.db.export(&dir).unwrap(), (0, 0));
        std::fs::remove_dir_all(dir).ok();
        let (s, _) = call(
            &st,
            "POST",
            "/v1/events",
            Some(&token),
            serde_json::json!({"events": []}),
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn threads_need_their_own_consent() {
        let st = state(10);
        let token = enrolled(&st, true, false).await;
        let thread = serde_json::json!({"id": "44444444-2222-4333-8444-555555555555", "app": "0.1.0", "transcript": []});
        assert_eq!(
            call(&st, "POST", "/v1/threads", Some(&token), thread)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn the_gate_cap_holds_and_works_without_data_consent() {
        let st = state(2);
        let token = enrolled(&st, false, false).await;
        let install = st.db.install_for(&token).unwrap().unwrap().0.install;
        assert!(st.db.count_gate_use(&install, 2).unwrap());
        assert!(st.db.count_gate_use(&install, 2).unwrap());
        assert!(!st.db.count_gate_use(&install, 2).unwrap());
        // No key configured: 503, not a crash.
        let (s, _) = call(
            &st,
            "POST",
            "/v1/typesafe/systemone",
            Some(&token),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    }
}
