//! Models discovered from the providers you have keys for, cached on disk so
//! pickers fill in without editing config and without a network call per turn.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::Credentials;
use crate::config::{ModelEntry, Route};

const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CachedModel {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelCache {
    /// Unix seconds of the last refresh.
    pub fetched_at: u64,
    /// Provider id → models, newest first.
    pub providers: BTreeMap<String, Vec<CachedModel>>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn cache_path() -> PathBuf {
    crate::paths::cache_dir().join("models.json")
}

impl ModelCache {
    pub fn load() -> Self {
        fs::read_to_string(cache_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = cache_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn is_stale(&self) -> bool {
        self.providers.is_empty() || now().saturating_sub(self.fetched_at) > MAX_AGE.as_secs()
    }

    /// Picker entries, ids like `anthropic:claude-sonnet-5-5`.
    pub fn entries(&self) -> Vec<ModelEntry> {
        Route::DISCOVERED
            .into_iter()
            .flat_map(|route| {
                self.providers
                    .get(route.provider_id())
                    .into_iter()
                    .flatten()
                    .map(move |m| ModelEntry {
                        id: format!("{}:{}", route.provider_id(), m.id),
                        label: m.label.clone(),
                        route,
                        model: m.id.clone(),
                        effort: None,
                        endpoint: None,
                    })
            })
            .collect()
    }
}

fn base_url(var: &str, default: &str) -> String {
    crate::envvar::get(var)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Lists the chat models one provider offers for this key.
pub async fn list_models(
    http: &reqwest::Client,
    route: Route,
    key: &str,
) -> Result<Vec<CachedModel>> {
    let (resp, field) = match route {
        Route::Anthropic => (
            http.get(format!(
                "{}/v1/models?limit=100",
                base_url("PATHFINDER_ANTHROPIC_BASE_URL", crate::providers::ANTHROPIC)
            ))
            .header("x-api-key", key)
            .header("anthropic-version", "2026-10-21"),
            "display_name",
        ),
        Route::Openai | Route::Chatgpt => (
            http.get(format!(
                "{}/models",
                base_url("PATHFINDER_OPENAI_BASE_URL", crate::providers::OPENAI)
            ))
            .bearer_auth(key),
            "id",
        ),
        other => bail!("{} has no model list", other.label()),
    };
    let resp = resp
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .context("listing models")?;
    let status = resp.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        bail!("{} rejected the key ({status})", route.provider_name());
    }
    if !status.is_success() {
        bail!("{} model list failed ({status})", route.provider_name());
    }
    let body: Value = resp.json().await.context("reading model list")?;
    if route == Route::Chatgpt {
        // Plan models: `models[]`, listable ones only, server order kept.
        return Ok(body["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| m["visibility"].as_str() == Some("list"))
            .filter_map(|m| {
                let id = m["slug"].as_str()?.to_string();
                let label = m["display_name"].as_str().unwrap_or(&id).to_string();
                Some(CachedModel { id, label })
            })
            .collect());
    }
    let mut listed: Vec<(i64, CachedModel)> = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let label = m[field].as_str().unwrap_or(&id).to_string();
            // OpenAI lists in no particular order; `created` sorts newest first.
            let created = m["created"].as_i64().unwrap_or(0);
            Some((created, CachedModel { id, label }))
        })
        .filter(|(_, m)| keep(route, &m.id))
        .collect();
    if route == Route::Openai {
        listed.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    }
    Ok(listed.into_iter().map(|(_, m)| m).collect())
}

/// Ends in a date snapshot (`-2025-11-20` or `-20251101`); the undated
/// alias is listed separately, and Anthropic's dated ids predate adaptive
/// thinking.
fn is_dated(id: &str) -> bool {
    let Some((_, tail)) = id.rsplit_once('-') else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    (tail.len() == 8 && digits(tail))
        || (tail.len() == 2 && digits(tail) && id.len() > 11 && {
            let t = &id[id.len() - 11..];
            t.starts_with('-') && t[1..].chars().all(|c| c.is_ascii_digit() || c == '-')
        })
}

/// Keeps chat models that work with Pathfinder's requests (reasoning
/// settings, adaptive thinking).
fn keep(route: Route, id: &str) -> bool {
    match route {
        Route::Anthropic => id.starts_with("claude-") && !is_dated(id),
        Route::Openai => {
            const SKIP: [&str; 11] = [
                "embedding",
                "audio",
                "tts",
                "transcribe",
                "realtime",
                "live",
                "image",
                "search",
                "moderation",
                "instruct",
                "codex",
            ];
            // Reasoning models: GPT-5 and later, and the o-series.
            let gpt5_plus = id
                .strip_prefix("gpt-")
                .and_then(|rest| {
                    rest.split(|c: char| !c.is_ascii_digit())
                        .next()?
                        .parse::<u32>()
                        .ok()
                })
                .is_some_and(|major| major >= 5);
            let o_series = id.starts_with('o') && id[1..].starts_with(|c: char| c.is_ascii_digit());
            (gpt5_plus || o_series) && !is_dated(id) && !SKIP.iter().any(|s| id.contains(s))
        }
        _ => false,
    }
}

/// Refreshes the model lists of endpoints that don't pin theirs, under
/// `endpoint:<id>`. An endpoint that can't be reached keeps its last list.
pub async fn refresh_endpoints(
    cfg: &crate::config::Config,
    creds: &Credentials,
    http: &reqwest::Client,
) {
    let mut cache = ModelCache::load();
    for ep in cfg.endpoints.iter().filter(|e| e.models.is_empty()) {
        let key = ep.key.as_deref().and_then(|k| creds.stored(k));
        match crate::llm::chat::list_models(http, &ep.base_url, key.as_deref()).await {
            Ok(models) => {
                cache.providers.insert(
                    format!("endpoint:{}", ep.id),
                    models
                        .into_iter()
                        .map(|id| CachedModel {
                            label: id.clone(),
                            id,
                        })
                        .collect(),
                );
            }
            Err(e) => tracing::warn!("endpoint `{}`: {e:#}", ep.id),
        }
    }
    if let Err(e) = cache.save() {
        tracing::warn!("{e:#}");
    }
}

/// Refreshes the cache for these providers. Providers whose list can't be
/// fetched keep their previous list.
pub async fn refresh(creds: &Credentials, http: &reqwest::Client, routes: &[Route]) -> ModelCache {
    let mut cache = ModelCache::load();
    for &route in routes {
        let key = if route == Route::Chatgpt {
            let session = creds.chatgpt();
            if session.signed_in() {
                match session.access_token().await {
                    Ok(token) => Some(token),
                    Err(e) => {
                        tracing::warn!("{e}");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            creds.lookup(route).map(|(key, _)| key)
        };
        let Some(key) = key else {
            cache.providers.remove(route.provider_id());
            continue;
        };
        match list_models(http, route, &key).await {
            Ok(models) => {
                cache
                    .providers
                    .insert(route.provider_id().to_string(), models);
            }
            Err(e) => tracing::warn!("{e:#}"),
        }
    }
    cache.fetched_at = now();
    if let Err(e) = cache.save() {
        tracing::warn!("{e:#}");
    }
    cache
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_openai_to_chat_models() {
        for id in ["gpt-6.1-sol", "gpt-6-luna", "o4-mini", "gpt-5.4-pro"] {
            assert!(keep(Route::Openai, id), "{id}");
        }
        for id in [
            "gpt-4o",
            "gpt-4.1-mini",
            "gpt-live-1.1",
            "gpt-5-2025-08-07",
            "text-embedding-3-large",
            "gpt-4o-audio-preview",
            "gpt-4o-2024-11-20",
            "dall-e-3",
            "gpt-realtime",
            "omni-moderation-latest",
            "gpt-5.3-codex",
        ] {
            assert!(!keep(Route::Openai, id), "{id}");
        }
        assert!(keep(Route::Anthropic, "claude-haiku-5-5"));
        assert!(!keep(Route::Anthropic, "claude-haiku-4-5-20251001"));
    }

    #[test]
    fn entries_are_namespaced_by_provider() {
        let mut cache = ModelCache::default();
        cache.providers.insert(
            "anthropic".into(),
            vec![CachedModel {
                id: "claude-sonnet-5-5".into(),
                label: "Claude Sonnet 5.5".into(),
            }],
        );
        let entries = cache.entries();
        assert_eq!(entries[0].id, "anthropic:claude-sonnet-5-5");
        assert_eq!(entries[0].route, Route::Anthropic);
        assert_eq!(entries[0].model, "claude-sonnet-5-5");
        assert!(!cache.is_stale() || cache.fetched_at == 0);
    }

    #[tokio::test]
    async fn lists_models_over_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await.unwrap();
            let body = r#"{"data":[{"id":"gpt-5","created":1},{"id":"text-embedding-3-small"},{"id":"gpt-6-luna","created":9}]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(reply.as_bytes()).await.unwrap();
        });
        // SAFETY: tests in this module don't read this variable concurrently.
        unsafe { std::env::set_var("PATHFINDER_OPENAI_BASE_URL", format!("http://{addr}")) };
        let models = list_models(&reqwest::Client::new(), Route::Openai, "k")
            .await
            .unwrap();
        unsafe { std::env::remove_var("PATHFINDER_OPENAI_BASE_URL") };
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-6-luna", "gpt-5"]);
    }
}
