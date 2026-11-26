//! What you agreed to: `telemetry.json` in the config folder. The install
//! token sits with the other secrets in `auth.json`.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use pathfinder_schema::NOTICE_VERSION;
use serde::{Deserialize, Serialize};

use crate::auth::secrets::{FileStore, SecretStore};

pub const TOKEN_NAME: &str = "telemetry-token";

/// Where the privacy notice is published: on the collector this build
/// talks to.
pub fn notice_url() -> String {
    format!("{}/privacy/", super::collector_url())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Consent {
    /// Random per install. The collector stores it with the email you join
    /// with (or the name on your invite code).
    pub install: String,
    /// The notice version you answered.
    pub notice: u32,
    /// Usage events (no message text).
    pub events: bool,
    /// Allows sharing a thread you choose, after seeing what is sent.
    pub threads: bool,
    /// Whether this install has an upload token.
    pub enrolled: bool,
    /// When you last answered, as Unix seconds.
    #[serde(default)]
    pub answered_at: Option<i64>,
}

impl Default for Consent {
    fn default() -> Self {
        Self {
            install: uuid::Uuid::new_v4().to_string(),
            notice: NOTICE_VERSION,
            events: false,
            threads: false,
            enrolled: false,
            answered_at: None,
        }
    }
}

pub fn path() -> PathBuf {
    crate::auth::chatgpt::config_dir().join("telemetry.json")
}

fn secrets() -> FileStore {
    FileStore::new(crate::auth::chatgpt::config_dir().join("auth.json"))
}

impl Consent {
    pub fn load() -> Self {
        fs::read_to_string(path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension(format!("json.{}", std::process::id()));
        fs::write(&tmp, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
    }

    /// Events are queued: you said yes to them, for the current notice,
    /// and the install is enrolled.
    pub fn collecting(&self) -> bool {
        self.events && self.enrolled && self.notice == NOTICE_VERSION
    }

    /// Whether to show the consent screen: never answered, or answered
    /// for an older notice while saying yes to something.
    pub fn needs_answer(&self) -> bool {
        self.answered_at.is_none()
            || (self.notice != NOTICE_VERSION && (self.events || self.threads))
    }

    pub fn may_share(&self) -> bool {
        self.threads && self.enrolled && self.notice == NOTICE_VERSION
    }

    pub fn token(&self) -> Option<String> {
        secrets().get(TOKEN_NAME).ok().flatten()
    }

    pub fn set_token(&mut self, token: &str) -> Result<()> {
        secrets().set(TOKEN_NAME, token)?;
        self.enrolled = true;
        Ok(())
    }

    /// Forgets the token and the yes answers; keeps the install id only
    /// until the next enrolment makes a new one.
    pub fn forget(&mut self) -> Result<()> {
        secrets().delete(TOKEN_NAME)?;
        *self = Consent {
            answered_at: self.answered_at,
            ..Consent::default()
        };
        Ok(())
    }

    pub fn stamp(&mut self) {
        self.notice = NOTICE_VERSION;
        self.answered_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() as i64);
    }
}
