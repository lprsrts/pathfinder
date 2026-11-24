//! API keys for the provider routes.
//!
//! Lookup order: environment variable, then Pathfinder's own keychain entry
//! (service `pathfinder`, account `api-key:<provider>`), then an existing
//! `<provider>-api-key` keychain item for the current user, which is only
//! ever read, never changed.

pub mod chatgpt;
pub mod secrets;

use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use thiserror::Error;

use crate::config::Route;
use chatgpt::ChatGpt;
use secrets::SecretStore;

pub const SERVICE: &str = "pathfinder";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Env(&'static str),
    Keychain,
    /// An existing `<provider>-api-key` item made by another tool.
    Shared,
}

impl KeySource {
    pub fn describe(self) -> String {
        match self {
            Self::Env(var) => format!("environment variable {var}"),
            Self::Keychain => "Pathfinder's keychain entry".to_string(),
            Self::Shared => "shared keychain item".to_string(),
        }
    }
}

#[derive(Debug, Error)]
#[error(
    "{name} needs an API key. Run `harness auth set {id}` in a terminal (or set {env}), then try again."
)]
pub struct MissingCredentials {
    pub route: Route,
    name: &'static str,
    id: &'static str,
    env: &'static str,
}

impl MissingCredentials {
    pub fn new(route: Route) -> Self {
        Self {
            route,
            name: route.provider_name(),
            id: route.provider_id(),
            env: route.key_env().unwrap_or("its API key variable"),
        }
    }
}

pub struct Credentials {
    store: Arc<dyn SecretStore>,
    chatgpt: Arc<ChatGpt>,
    user: String,
    use_env: bool,
    /// Successful lookups only, so a key added later is found on the next try.
    cache: Mutex<HashMap<Route, String>>,
}

fn own_account(route: Route) -> String {
    format!("api-key:{}", route.provider_id())
}

fn shared_service(route: Route) -> String {
    format!("{}-api-key", route.provider_id())
}

impl Credentials {
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self {
            chatgpt: Arc::new(ChatGpt::new(store.clone())),
            store,
            user: env::var("USER").unwrap_or_default(),
            use_env: true,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The system keychain.
    pub fn system() -> Self {
        Self::new(Arc::new(secrets::KeyringStore))
    }

    #[cfg(test)]
    pub fn for_tests(store: Arc<dyn SecretStore>, user: &str) -> Self {
        Self {
            chatgpt: Arc::new(ChatGpt::new(store.clone())),
            store,
            user: user.to_string(),
            use_env: false,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The Sign in with ChatGPT session.
    pub fn chatgpt(&self) -> Arc<ChatGpt> {
        self.chatgpt.clone()
    }

    /// Finds a key and where it came from, without caching.
    pub fn lookup(&self, route: Route) -> Option<(String, KeySource)> {
        self.find(route, true)
    }

    /// Like `lookup`, but never reads shared `<provider>-api-key` items:
    /// macOS asks before another tool's item is read, so background work
    /// (model discovery) must not touch them.
    pub fn lookup_own(&self, route: Route) -> Option<(String, KeySource)> {
        self.find(route, false)
    }

    fn find(&self, route: Route, include_shared: bool) -> Option<(String, KeySource)> {
        if self.use_env {
            if let Some(var) = route.key_env() {
                if let Ok(v) = env::var(var) {
                    if !v.trim().is_empty() {
                        return Some((v.trim().to_string(), KeySource::Env(var)));
                    }
                }
            }
        }
        let read = |service: &str, account: &str| match self.store.get(service, account) {
            Ok(v) => v.filter(|v| !v.trim().is_empty()),
            Err(e) => {
                tracing::warn!("reading {service}/{account}: {e}");
                None
            }
        };
        if let Some(key) = read(SERVICE, &own_account(route)) {
            return Some((key, KeySource::Keychain));
        }
        if include_shared && !self.user.is_empty() {
            if let Some(key) = read(&shared_service(route), &self.user) {
                return Some((key.trim().to_string(), KeySource::Shared));
            }
        }
        None
    }

    /// The key for an API route, cached once found.
    pub fn api_key(&self, route: Route) -> Result<String, MissingCredentials> {
        if let Some(key) = self.cache.lock().unwrap().get(&route) {
            return Ok(key.clone());
        }
        let (key, _) = self
            .lookup(route)
            .ok_or_else(|| MissingCredentials::new(route))?;
        self.cache.lock().unwrap().insert(route, key.clone());
        Ok(key)
    }

    pub fn set_key(&self, route: Route, key: &str) -> Result<()> {
        self.store.set(SERVICE, &own_account(route), key)?;
        self.cache.lock().unwrap().remove(&route);
        Ok(())
    }

    /// Removes Pathfinder's own entry; shared items are left alone.
    pub fn remove_key(&self, route: Route) -> Result<bool> {
        self.cache.lock().unwrap().remove(&route);
        self.store.delete(SERVICE, &own_account(route))
    }

    /// Copies a shared `<provider>-api-key` item into Pathfinder's own entry.
    pub fn import_shared(&self, route: Route) -> Result<bool> {
        match self.store.get(&shared_service(route), &self.user)? {
            Some(key) if !key.trim().is_empty() => {
                self.set_key(route, key.trim())?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// `sk-…1234` style display.
pub fn mask(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let head: String = key.chars().take(3).collect();
    format!("{head}…{tail}")
}

/// Rejects input that can't be an API key (pasted with spaces, too short).
pub fn check_key_shape(key: &str) -> Result<(), String> {
    if key.len() < 20 {
        return Err("that looks too short for an API key".into());
    }
    if key.chars().any(char::is_whitespace) {
        return Err("API keys don't contain spaces or line breaks".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::secrets::MemoryStore;
    use super::*;

    #[test]
    fn own_entry_wins_over_shared_and_shared_is_never_changed() {
        let store = Arc::new(MemoryStore::default());
        store
            .set("anthropic-api-key", "alper", "sk-ant-shared-0000000000")
            .unwrap();
        let creds = Credentials::for_tests(store.clone(), "alper");

        let (key, source) = creds.lookup(Route::Anthropic).unwrap();
        assert_eq!(
            (key.as_str(), source),
            ("sk-ant-shared-0000000000", KeySource::Shared)
        );
        assert!(
            creds.lookup_own(Route::Anthropic).is_none(),
            "discovery must not read shared items"
        );

        creds
            .set_key(Route::Anthropic, "sk-ant-own-111111111111")
            .unwrap();
        assert_eq!(
            creds.api_key(Route::Anthropic).unwrap(),
            "sk-ant-own-111111111111"
        );
        assert_eq!(
            creds.lookup(Route::Anthropic).unwrap().1,
            KeySource::Keychain
        );

        assert!(creds.remove_key(Route::Anthropic).unwrap());
        assert!(!creds.remove_key(Route::Anthropic).unwrap());
        assert_eq!(
            store.get("anthropic-api-key", "alper").unwrap().as_deref(),
            Some("sk-ant-shared-0000000000")
        );
        assert!(creds.lookup(Route::Openai).is_none());
        let err = creds.api_key(Route::Openai).unwrap_err().to_string();
        assert!(err.contains("harness auth set openai"));
        assert!(err.contains("OPENAI_API_KEY"));
    }

    #[test]
    fn missing_keys_are_not_cached_and_import_copies() {
        let store = Arc::new(MemoryStore::default());
        let creds = Credentials::for_tests(store.clone(), "alper");
        assert!(creds.api_key(Route::Openai).is_err());
        store
            .set("openai-api-key", "alper", "sk-proj-abcdefghijklmnop")
            .unwrap();
        assert_eq!(
            creds.api_key(Route::Openai).unwrap(),
            "sk-proj-abcdefghijklmnop"
        );
        assert!(creds.import_shared(Route::Openai).unwrap());
        assert_eq!(
            store.get(SERVICE, "api-key:openai").unwrap().as_deref(),
            Some("sk-proj-abcdefghijklmnop")
        );
        assert_eq!(mask("sk-proj-abcdefghijklmnop"), "sk-…mnop");
        assert!(check_key_shape("short").is_err());
        assert!(check_key_shape("sk-proj-abc def-ghijklmnopqr").is_err());
        assert!(check_key_shape("sk-proj-abcdefghijklmnop").is_ok());
    }
}
