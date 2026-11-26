//! Talking to the collector.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use pathfinder_schema::{
    Consent as RemoteConsent, Enroll, Enrolled, LinkPoll, LinkStart, LinkStarted, NOTICE_VERSION,
    SharedThread,
};
use reqwest::StatusCode;

use super::{Consent, GLOBAL, clear, queued, sent_up_to};

/// Events per upload.
const BATCH: usize = 500;

/// How long one request to the collector may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// How often a waiting browser sign-in checks back.
const POLL_EVERY: Duration = Duration::from_secs(2);

pub fn collector_url() -> String {
    crate::envvar::get("PATHFINDER_COLLECTOR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| crate::operator::COLLECTOR.into())
        .trim_end_matches('/')
        .to_string()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("pathfinder/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

/// Trades an invite code for an upload token and saves both answers.
pub async fn enroll(invite: &str, consent: &mut Consent) -> Result<()> {
    consent.stamp();
    let body = Enroll {
        invite: invite.trim().to_string(),
        install: consent.install.clone(),
        notice: NOTICE_VERSION,
        events: consent.events,
        threads: consent.threads,
    };
    let res = client()
        .post(format!("{}/v1/enroll", collector_url()))
        .json(&body)
        .send()
        .await
        .context("reaching the collector")?;
    match res.status() {
        s if s.is_success() => {}
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
            bail!("that invite code isn't valid (or was already used)")
        }
        s => bail!("the collector answered {s}"),
    }
    let Enrolled { token } = res.json().await.context("reading the enrolment")?;
    consent.set_token(&token)?;
    consent.save()?;
    super::reload();
    Ok(())
}

/// A browser sign-in in progress: the person opens `url`, signs in with
/// their email, and checks the page shows `code`.
#[derive(Debug, Clone)]
pub struct SignIn {
    pub code: String,
    pub url: String,
    secret: String,
    expires_in: Duration,
}

/// Starts signing in with the browser, with the answers in `consent`.
pub async fn start_sign_in(consent: &Consent) -> Result<SignIn> {
    let res = client()
        .post(format!("{}/v1/link", collector_url()))
        .json(&LinkStart {
            install: consent.install.clone(),
            notice: NOTICE_VERSION,
            events: consent.events,
            threads: consent.threads,
        })
        .send()
        .await
        .context("reaching the collector")?;
    match res.status() {
        s if s.is_success() => {}
        StatusCode::SERVICE_UNAVAILABLE => {
            bail!("signing in with the browser isn't available; use an invite code")
        }
        StatusCode::TOO_MANY_REQUESTS => {
            bail!("too many sign-ins at once; try again in a few minutes")
        }
        s => bail!("the collector answered {s}"),
    }
    let started: LinkStarted = res.json().await.context("reading the sign-in")?;
    Ok(SignIn {
        url: format!("{}/join?code={}", collector_url(), started.code),
        code: started.code,
        secret: started.secret,
        expires_in: Duration::from_secs(started.expires_in),
    })
}

/// Waits for the person to approve the sign-in in the browser, then saves
/// the token and the answers in `consent`.
pub async fn finish_sign_in(sign_in: &SignIn, consent: &mut Consent) -> Result<()> {
    let deadline = std::time::Instant::now() + sign_in.expires_in + Duration::from_secs(30);
    let http = client();
    let token = loop {
        if std::time::Instant::now() > deadline {
            bail!("the sign-in expired (codes last 15 minutes); start again");
        }
        tokio::time::sleep(POLL_EVERY).await;
        let res = http
            .post(format!("{}/v1/link/poll", collector_url()))
            .json(&LinkPoll {
                secret: sign_in.secret.clone(),
            })
            .send()
            .await;
        // A dropped connection isn't the end: try again until the deadline.
        let Ok(res) = res else { continue };
        match res.status() {
            StatusCode::OK => {
                let Enrolled { token } = res.json().await.context("reading the sign-in")?;
                break token;
            }
            StatusCode::ACCEPTED => {}
            StatusCode::GONE => {
                bail!("the sign-in expired (codes last 15 minutes); start again")
            }
            s if s.is_server_error() => {}
            s => bail!("the collector answered {s}"),
        }
    };
    consent.stamp();
    consent.set_token(&token)?;
    consent.save()?;
    super::reload();
    Ok(())
}

/// Tells the collector about a changed answer.
pub async fn set_remote_consent(consent: &Consent) -> Result<()> {
    let Some(token) = consent.token() else {
        return Ok(());
    };
    let res = client()
        .put(format!("{}/v1/consent", collector_url()))
        .bearer_auth(token)
        .json(&RemoteConsent {
            notice: consent.notice,
            events: consent.events,
            threads: consent.threads,
        })
        .send()
        .await
        .context("reaching the collector")?;
    if !res.status().is_success() {
        bail!("the collector answered {}", res.status());
    }
    Ok(())
}

/// Asks the collector to erase everything from this install.
pub async fn delete_remote(consent: &Consent) -> Result<()> {
    let Some(token) = consent.token() else {
        bail!("this install isn't enrolled, so the collector has nothing from it");
    };
    let res = client()
        .delete(format!("{}/v1/me", collector_url()))
        .bearer_auth(token)
        .send()
        .await
        .context("reaching the collector")?;
    if !(res.status().is_success() || res.status() == StatusCode::GONE) {
        bail!("the collector answered {}", res.status());
    }
    Ok(())
}

/// Sends a bug report you chose to send. It needs a link to the collector
/// (joined), not data consent: sending it is the consent.
pub async fn report_bug(consent: &Consent, report: &pathfinder_schema::BugReport) -> Result<()> {
    let token = consent
        .token()
        .context("this computer hasn't joined, so the report can't be sent from here")?;
    let res = client()
        .post(format!("{}/v1/reports", collector_url()))
        .bearer_auth(token)
        .json(report)
        .send()
        .await
        .context("reaching Pathfinder's server")?;
    if !res.status().is_success() {
        bail!("Pathfinder's server answered {}", res.status());
    }
    Ok(())
}

/// Uploads a thread you chose to share.
pub async fn share(consent: &Consent, thread: &SharedThread) -> Result<()> {
    if !consent.may_share() {
        bail!("sharing threads is off; turn it on with `pathfinder data on --threads`");
    }
    let token = consent.token().context("this install isn't enrolled")?;
    let res = client()
        .post(format!("{}/v1/threads", collector_url()))
        .bearer_auth(token)
        .json(thread)
        .send()
        .await
        .context("reaching the collector")?;
    if !res.status().is_success() {
        bail!("the collector answered {}", res.status());
    }
    Ok(())
}

/// Uploads what is queued, a batch at a time. A 410 means the collector
/// has stopped collecting from this install: collection switches off and
/// the queue is dropped.
pub async fn flush() -> Result<()> {
    let Some(t) = GLOBAL.get() else {
        return Ok(());
    };
    if !super::enabled() {
        return Ok(());
    }
    let Some(token) = t.consent.lock().unwrap().token() else {
        return Ok(());
    };
    loop {
        let rows = queued(&t.db.lock().unwrap(), BATCH)?;
        let Some(&(last, _)) = rows.last() else {
            return Ok(());
        };
        let body = format!(
            "{{\"events\":[{}]}}",
            rows.iter()
                .map(|(_, j)| j.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
        let res = client()
            .post(format!("{}/v1/events", collector_url()))
            .bearer_auth(&token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .context("reaching the collector")?;
        match res.status() {
            s if s.is_success() => sent_up_to(&t.db.lock().unwrap(), last)?,
            StatusCode::GONE => {
                let mut c = t.consent.lock().unwrap();
                c.events = false;
                c.threads = false;
                c.save().ok();
                clear(&t.db.lock().unwrap())?;
                bail!("the collector stopped collecting from this install");
            }
            // A batch the collector can't read would block the queue
            // forever: drop it.
            StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE => {
                tracing::warn!("the collector refused a batch of usage events; dropping it");
                sent_up_to(&t.db.lock().unwrap(), last)?;
            }
            s => bail!("the collector answered {s}"),
        }
        if rows.len() < BATCH {
            return Ok(());
        }
    }
}
