//! Talking to the collector.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use pathfinder_schema::{Consent as RemoteConsent, Enroll, Enrolled, NOTICE_VERSION, SharedThread};
use reqwest::StatusCode;

use super::{Consent, GLOBAL, clear, queued, sent_up_to};

/// Events per upload.
const BATCH: usize = 500;

pub fn collector_url() -> String {
    std::env::var("HARNESS_COLLECTOR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "https://pathfinder.lprsrts.com".into())
        .trim_end_matches('/')
        .to_string()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
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

/// Uploads a thread you chose to share.
pub async fn share(consent: &Consent, thread: &SharedThread) -> Result<()> {
    if !consent.may_share() {
        bail!("sharing threads is off; turn it on with `harness data on --threads`");
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
