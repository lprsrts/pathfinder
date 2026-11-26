//! Pathfinder's collector: usage events from consenting installs, shared
//! threads, the gate proxy, and the installer and releases.
//!
//! ```text
//! collector serve                 run the service
//! collector invite <name>         a single-use invite code for <name>
//! collector allow <email>         let <email> sign in with the browser
//! collector disallow <email>      take <email> off the list, stop its installs
//! collector people                who may sign in
//! collector stats                 one line per install
//! collector report                active people, retention, latency, errors
//! collector reports [n]           the newest bug reports (default 10)
//! collector gate-stats            gate checks per day and per person today
//! collector gate on|off           resume or pause the shared gate (no restart)
//! collector export [dir] [--with-emails]
//!                                 events, threads and installs as JSONL
//!                                 (pseudonymous unless --with-emails)
//! collector stop <name|install>   stop collecting from an install
//! collector prune                 delete events past retention
//! collector keygen                make the release signing key (keychain)
//! collector sign <file>           sign a release manifest
//! ```
//!
//! Settings come from the environment; see `collector/README.md`.

mod access;
mod api;
mod db;
mod limit;
mod release;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Where the service listens unless COLLECTOR_ADDR says otherwise.
const DEFAULT_ADDR: &str = "0.0.0.0:8080";

/// How long a request to TypeSafe or Cloudflare may take.
const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

const PRUNE_EVERY: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

fn db_path() -> PathBuf {
    PathBuf::from(env_or("COLLECTOR_DB", "/data/collector.sqlite"))
}

fn open_db() -> Result<db::Db> {
    let path = db_path();
    db::Db::open(&path).with_context(|| format!("opening {}", path.display()))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn env_num<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
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
        // For the container's healthcheck: the image has no curl.
        Some("health") => {
            let addr = env_or("COLLECTOR_ADDR", DEFAULT_ADDR).replace("0.0.0.0", "127.0.0.1");
            let ok = reqwest::get(format!("http://{addr}/healthz"))
                .await
                .is_ok_and(|r| r.status().is_success());
            if !ok {
                bail!("not healthy");
            }
            Ok(())
        }
        Some("invite") => {
            let name = args.get(1).context("usage: collector invite <name>")?;
            let code = open_db()?.new_invite(name)?;
            println!("{code}");
            Ok(())
        }
        Some("allow") => {
            let email = args
                .get(1)
                .filter(|e| e.contains('@'))
                .context("usage: collector allow <email>")?;
            open_db()?.allow(email)?;
            println!(
                "{} may sign in now: send them {}",
                db::normal_email(email),
                env_or("COLLECTOR_PUBLIC_URL", "this service's address")
            );
            Ok(())
        }
        Some("disallow") => {
            let email = args.get(1).context("usage: collector disallow <email>")?;
            let n = open_db()?.disallow(email)?;
            println!("removed; stopped {n} install(s)");
            Ok(())
        }
        Some("people") => {
            for email in open_db()?.people()? {
                println!("{email}");
            }
            Ok(())
        }
        Some("stats") => {
            for line in open_db()?.stats()? {
                println!("{line}");
            }
            Ok(())
        }
        Some("export") => {
            let with_labels = args.iter().any(|a| a == "--with-emails");
            let db = open_db()?;
            // Into the data volume by default: the container's /tmp is small.
            let dir = match args.get(1).filter(|a| !a.starts_with("--")) {
                Some(d) => PathBuf::from(d),
                None => db_path()
                    .parent()
                    .map(|p| p.join("exports").join(db::date(now())))
                    .context("no data folder")?,
            };
            let (e, t) = db.export(&dir, with_labels)?;
            println!("{e} events and {t} threads written to {}", dir.display());
            Ok(())
        }
        Some("reports") => {
            let n = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(10);
            for (day, who, r) in open_db()?.reports(n)? {
                println!("== {day} · {who} · {} on {:?} {:?}", r.app, r.os, r.arch);
                println!("{}\n", r.note);
                println!("{}\n", r.details);
            }
            Ok(())
        }
        Some("report") => {
            print!("{}", open_db()?.report()?);
            Ok(())
        }
        Some("gate-stats") => {
            let (daily, people) = open_db()?.gate_stats(14)?;
            println!(
                "{:<12}{:>10}{:>8}{:>8}",
                "day", "forwarded", "capped", "errors"
            );
            for (day, f, c, e) in daily {
                println!("{day:<12}{f:>10}{c:>8}{e:>8}");
            }
            println!("\nToday by person:");
            for (who, n) in people {
                println!("  {who:<32}{n:>6}");
            }
            Ok(())
        }
        Some("gate") => {
            let db = open_db()?;
            match args.get(1).map(String::as_str) {
                Some("on") => db.set_gate_enabled(true)?,
                Some("off") => db.set_gate_enabled(false)?,
                Some(_) => bail!("usage: collector gate on|off"),
                None => {}
            }
            println!(
                "The shared gate is {}.",
                if db.gate_enabled()? { "on" } else { "off" }
            );
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
        Some("keygen") => release::keygen(),
        Some("sign") => {
            let file = args.get(1).context("usage: collector sign <file>")?;
            release::sign(std::path::Path::new(file))
        }
        Some("prune") => {
            let n = open_db()?.prune()?;
            println!("deleted {n} events past retention");
            Ok(())
        }
        _ => bail!(
            "usage: collector serve | invite <name> | allow <email> | disallow <email> | people | stats | export <dir> | stop <name|install> | prune"
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
    let http = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(concat!("pathfinder-collector/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let access = match (
        std::env::var("ACCESS_TEAM_DOMAIN")
            .ok()
            .filter(|v| !v.trim().is_empty()),
        std::env::var("ACCESS_AUD")
            .ok()
            .filter(|v| !v.trim().is_empty()),
    ) {
        (Some(team), Some(aud)) => Some(access::Access::cloudflare(&team, &aud, http.clone())),
        _ => {
            tracing::warn!("no ACCESS_TEAM_DOMAIN/ACCESS_AUD: browser sign-in is off");
            None
        }
    };
    let settings = api::Settings {
        typesafe_key,
        typesafe_base: env_or("TYPESAFE_BASE_URL", "https://api.typesafe.ai/v1")
            .trim_end_matches('/')
            .to_string(),
        gate_model: env_or("GATE_MODEL", "jev-1.14.0"),
        gate_cap: env_num("GATE_DAILY_CAP", 400),
        gate_global_cap: env_num("GATE_GLOBAL_DAILY_CAP", 5_000),
        gate_timeout: std::time::Duration::from_secs(env_num("GATE_TIMEOUT_SECS", 8)),
        public_url: env_or("COLLECTOR_PUBLIC_URL", "http://127.0.0.1:8080")
            .trim_end_matches('/')
            .to_string(),
        operator: env_or("COLLECTOR_OPERATOR", "the person who invited you"),
        public_dir: PathBuf::from(env_or("COLLECTOR_PUBLIC", "/public")),
    };
    let state = Arc::new(api::AppState::new(open_db()?, settings, http, access));
    // Delete what is past retention, once a day.
    let pruner = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PRUNE_EVERY);
        loop {
            tick.tick().await;
            let db = pruner.db.clone();
            match tokio::task::spawn_blocking(move || db.prune()).await {
                Ok(Ok(n)) if n > 0 => tracing::info!("pruned {n} events past retention"),
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::error!("pruning: {e:#}"),
                Err(e) => tracing::error!("pruning: {e}"),
            }
        }
    });
    let addr = env_or("COLLECTOR_ADDR", DEFAULT_ADDR);
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
    const PUBLIC: &str = "https://collector.test";

    fn state(cap: u32) -> Arc<api::AppState> {
        Arc::new(api::AppState::new(
            db::Db::open_in_memory().unwrap(),
            api::Settings {
                typesafe_key: None,
                typesafe_base: "http://127.0.0.1:9".into(),
                gate_model: "jev-1.14.0".into(),
                gate_cap: cap,
                gate_global_cap: 1_000,
                gate_timeout: std::time::Duration::from_secs(1),
                public_url: PUBLIC.into(),
                operator: "the operator".into(),
                public_dir: std::env::temp_dir(),
            },
            reqwest::Client::new(),
            Some(access::Access::Fake),
        ))
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
        // An event with a field the schema doesn't have is refused on its
        // own; the rest of the batch is kept.
        let mut bad = event("22222222-2222-4333-8444-555555555555");
        bad["event"]["text"] = "my notes".into();
        let good = event("33333333-2222-4333-8444-555555555555");
        let (s, body) = call(
            &st,
            "POST",
            "/v1/events",
            Some(&token),
            serde_json::json!({"events": [bad, good]}),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["stored"], 1);
        assert_eq!(
            body["rejected"],
            serde_json::json!(["22222222-2222-4333-8444-555555555555"])
        );
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
        assert_eq!(st.db.export(&dir, false).unwrap(), (0, 0));
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
        use db::GateCheck;
        assert_eq!(
            st.db.gate_check(&install, 2, 10).unwrap(),
            GateCheck::Allowed
        );
        assert_eq!(
            st.db.gate_check(&install, 2, 10).unwrap(),
            GateCheck::Allowed
        );
        assert_eq!(
            st.db.gate_check(&install, 2, 10).unwrap(),
            GateCheck::PersonCapped
        );
        st.db.set_gate_enabled(false).unwrap();
        assert_eq!(st.db.gate_check(&install, 9, 10).unwrap(), GateCheck::Off);
        st.db.set_gate_enabled(true).unwrap();
        assert_eq!(
            st.db.gate_check(&install, 9, 2).unwrap(),
            GateCheck::GlobalCapped
        );
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

    async fn join(
        st: &Arc<api::AppState>,
        method: &str,
        code: &str,
        email: Option<&str>,
    ) -> (StatusCode, String) {
        let (uri, body) = match method {
            "GET" => (format!("/join?code={code}"), String::new()),
            _ => ("/join".to_string(), format!("code={code}")),
        };
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("origin", PUBLIC)
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(e) = email {
            req = req.header("x-test-email", e);
        }
        let res = api::router(st.clone())
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn signing_in_with_the_browser_links_an_install_to_its_email() {
        let st = state(10);
        st.db.allow("Friend@Example.com").unwrap();
        let start = serde_json::json!({"install": INSTALL, "notice": NOTICE_VERSION, "events": true, "threads": false});
        let (s, started) = call(&st, "POST", "/v1/link", None, start).await;
        assert_eq!(s, StatusCode::OK);
        let code = started["code"].as_str().unwrap().to_string();
        let secret = started["secret"].as_str().unwrap().to_string();
        assert_eq!(code.len(), 9);
        let poll = serde_json::json!({ "secret": secret });
        assert_eq!(
            call(&st, "POST", "/v1/link/poll", None, poll.clone())
                .await
                .0,
            StatusCode::ACCEPTED
        );

        // Not signed in, or not on the list: no.
        assert_eq!(
            join(&st, "GET", &code, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (s, html) = join(&st, "POST", &code, Some("stranger@example.com")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert!(html.contains("isn’t on the list"));
        assert_eq!(
            call(&st, "POST", "/v1/link/poll", None, poll.clone())
                .await
                .0,
            StatusCode::ACCEPTED
        );

        // The page shows the code and the choices; approving links it.
        let (s, html) = join(&st, "GET", &code.to_lowercase(), Some("friend@example.com")).await;
        assert_eq!(s, StatusCode::OK);
        assert!(html.contains(&code) && html.contains("Link this computer"));
        let (s, html) = join(&st, "POST", &code, Some("friend@example.com")).await;
        assert_eq!(s, StatusCode::OK, "{html}");

        let (s, body) = call(&st, "POST", "/v1/link/poll", None, poll.clone()).await;
        assert_eq!(s, StatusCode::OK);
        let token = body["token"].as_str().unwrap();
        let (install, stopped) = st.db.install_for(token).unwrap().unwrap();
        assert!(!stopped);
        assert_eq!(install.label, "friend@example.com");
        assert!(install.events && !install.threads);
        // Collected once; the code is gone.
        assert_eq!(
            call(&st, "POST", "/v1/link/poll", None, poll).await.0,
            StatusCode::GONE
        );
        assert_eq!(
            join(&st, "GET", &code, Some("friend@example.com")).await.0,
            StatusCode::GONE
        );

        // Taking them off the list stops their installs.
        assert_eq!(st.db.disallow("friend@example.com").unwrap(), 1);
        assert!(st.db.install_for(token).unwrap().unwrap().1);
    }

    #[tokio::test]
    async fn sign_in_pages_escape_what_they_show() {
        let st = state(10);
        st.db.allow("a<b>@example.com").unwrap();
        let (s, html) = join(&st, "GET", "", Some("a<b>@example.com")).await;
        assert_eq!(s, StatusCode::OK);
        assert!(html.contains("a&lt;b&gt;@example.com") && !html.contains("a<b>"));
    }

    #[tokio::test]
    async fn the_token_is_checked_before_a_body_is_read() {
        let st = state(10);
        let big = serde_json::json!({"events": [], "pad": "x".repeat(3 << 20)});
        let (s, _) = call(&st, "POST", "/v1/events", None, big).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_gate_proxy_forwards_only_jev_requests() {
        let st = state(10);
        let token = enrolled(&st, false, false).await;
        // A key, so the request gets as far as being checked.
        let mut st2 = Arc::try_unwrap(st).ok().unwrap();
        st2.settings.typesafe_key = Some("k".into());
        let st = Arc::new(st2);
        let q = serde_json::json!({"s0": {"type": "choice", "instructions": "?", "criteria": {"yes": "y", "no": "n"}}});
        for bad in [
            serde_json::json!({"model": "gpt-4o", "state": "x", "questions": q}),
            serde_json::json!({"model": "jev-1.14.0", "state": "x", "questions": {}}),
            serde_json::json!({"model": "jev-1.14.0", "state": "x", "questions": q, "extra": 1}),
            serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}),
        ] {
            let (s, _) = call(&st, "POST", "/v1/typesafe/systemone", Some(&token), bad).await;
            assert_eq!(s, StatusCode::BAD_REQUEST);
        }
        // A real gate request gets past the checks: it is counted and
        // forwarded (the test upstream isn't there, so 502).
        let good =
            serde_json::json!({"model": "jev-1.14.0", "state": "Draft reply: …", "questions": q});
        let (s, _) = call(&st, "POST", "/v1/typesafe/systemone", Some(&token), good).await;
        assert_eq!(s, StatusCode::BAD_GATEWAY);
        assert_eq!(st.db.gate_today().unwrap(), 1);
    }

    #[tokio::test]
    async fn sign_in_forms_from_other_origins_are_refused() {
        let st = state(10);
        st.db.allow("friend@example.com").unwrap();
        let (code, _) = st
            .db
            .start_link(INSTALL, NOTICE_VERSION, false, false)
            .unwrap()
            .unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/join")
            .header("origin", "https://evil.test")
            .header("x-test-email", "friend@example.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("code={code}")))
            .unwrap();
        let res = api::router(st.clone()).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert_eq!(st.db.link(&code).unwrap().unwrap().email, None);
    }

    #[tokio::test]
    async fn an_install_cant_be_taken_over_by_someone_else() {
        let st = state(10);
        st.db.allow("a@example.com").unwrap();
        st.db.allow("b@example.com").unwrap();
        let link = |who: &str| {
            let (code, secret) = st
                .db
                .start_link(INSTALL, NOTICE_VERSION, false, false)
                .unwrap()
                .unwrap();
            assert!(st.db.approve_link(&code, who).unwrap());
            st.db.poll_link(&secret).unwrap()
        };
        assert!(matches!(link("a@example.com"), db::LinkState::Done(_)));
        assert_eq!(link("b@example.com"), db::LinkState::Gone);
        // The same person linking again is fine.
        assert!(matches!(link("a@example.com"), db::LinkState::Done(_)));
    }

    #[tokio::test]
    async fn each_install_has_one_pending_sign_in() {
        let st = state(10);
        let (first, _) = st
            .db
            .start_link(INSTALL, NOTICE_VERSION, false, false)
            .unwrap()
            .unwrap();
        let (second, _) = st
            .db
            .start_link(INSTALL, NOTICE_VERSION, false, false)
            .unwrap()
            .unwrap();
        assert!(st.db.link(&first).unwrap().is_none());
        assert!(st.db.link(&second).unwrap().is_some());
    }
}
