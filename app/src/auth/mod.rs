//! API keys for the provider routes and the ChatGPT session.
//!
//! Lookup order: environment variable, then Pathfinder's auth file
//! (`auth.json` in the config folder, mode 0600). `pathfinder auth import` copies
//! an existing `<provider>-api-key` macOS keychain item into the file once;
//! nothing else touches the keychain, so there are no prompts.

pub mod accounts;
pub mod chatgpt;
pub mod connect;
pub mod flow;
pub mod secrets;

use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use thiserror::Error;

use crate::config::Route;
use chatgpt::ChatGpt;
use secrets::{FileStore, SecretStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Env(&'static str),
    File,
}

impl KeySource {
    pub fn describe(self) -> String {
        match self {
            Self::Env(var) => format!("environment variable {var}"),
            Self::File => "Pathfinder's auth file".to_string(),
        }
    }
}

#[derive(Debug, Error)]
#[error(
    "{name} needs an API key. Run `pathfinder auth set {id}` in a terminal (or set {env}), then try again."
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

/// Reads a generic-password item by service name, for `auth import` only.
type KeychainReader = Arc<dyn Fn(&str) -> Result<Option<String>> + Send + Sync>;

pub struct Credentials {
    store: Arc<dyn SecretStore>,
    chatgpt: Arc<ChatGpt>,
    keychain: KeychainReader,
    use_env: bool,
    /// Successful lookups only, so a key added later is found on the next try.
    cache: Mutex<HashMap<Route, String>>,
}

/// The entry name in the auth file, also the keychain service `auth import`
/// reads: `anthropic-api-key`, `openai-api-key`.
fn key_name(route: Route) -> String {
    format!("{}-api-key", route.provider_id())
}

/// `security find-generic-password -s <service> -w`, as in your own scripts.
fn read_macos_keychain(service: &str) -> Result<Option<String>> {
    if !cfg!(target_os = "macos") {
        bail!("`auth import` reads the macOS keychain; use `pathfinder auth set` instead");
    }
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()?;
    match out.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )),
        // errSecItemNotFound
        Some(44) => Ok(None),
        _ => bail!("keychain: {}", String::from_utf8_lossy(&out.stderr).trim()),
    }
}

impl Credentials {
    /// `auth.json` in the config folder ([`crate::paths`]).
    pub fn system() -> Self {
        let store: Arc<dyn SecretStore> =
            Arc::new(FileStore::new(chatgpt::config_dir().join("auth.json")));
        Self {
            chatgpt: Arc::new(ChatGpt::new(store.clone())),
            store,
            keychain: Arc::new(read_macos_keychain),
            use_env: true,
            cache: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    pub fn for_tests(store: Arc<dyn SecretStore>, keychain: KeychainReader) -> Self {
        Self {
            chatgpt: Arc::new(ChatGpt::new(store.clone())),
            store,
            keychain,
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
        if self.use_env
            && let Some(var) = route.key_env()
            && let Ok(v) = env::var(var)
            && !v.trim().is_empty()
        {
            return Some((v.trim().to_string(), KeySource::Env(var)));
        }
        match self.store.get(&key_name(route)) {
            Ok(v) => v
                .filter(|v| !v.trim().is_empty())
                .map(|v| (v, KeySource::File)),
            Err(e) => {
                tracing::warn!("{e:#}");
                None
            }
        }
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
        self.store.set(&key_name(route), key)?;
        self.cache.lock().unwrap().remove(&route);
        Ok(())
    }

    pub fn remove_key(&self, route: Route) -> Result<bool> {
        self.cache.lock().unwrap().remove(&route);
        self.store.delete(&key_name(route))
    }

    /// A key that isn't a model route's, such as a decision service's: the
    /// environment variable, then the auth file entry `name`.
    pub fn named_key(&self, name: &str, env_var: &'static str) -> Option<(String, KeySource)> {
        if self.use_env
            && let Ok(v) = env::var(env_var)
            && !v.trim().is_empty()
        {
            return Some((v.trim().to_string(), KeySource::Env(env_var)));
        }
        self.store
            .get(name)
            .ok()
            .flatten()
            .filter(|v| !v.trim().is_empty())
            .map(|v| (v, KeySource::File))
    }

    /// A key stored under `name` in the auth file (an endpoint's key).
    pub fn stored(&self, name: &str) -> Option<String> {
        self.store
            .get(name)
            .ok()
            .flatten()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    pub fn set_named(&self, name: &str, key: &str) -> Result<()> {
        self.store.set(name, key)
    }

    pub fn remove_named(&self, name: &str) -> Result<bool> {
        self.store.delete(name)
    }

    /// Copies the keychain item `name` into the auth file entry `name`.
    pub fn import_named(&self, name: &str) -> Result<bool> {
        match (self.keychain)(name)? {
            Some(key) if !key.trim().is_empty() => {
                self.store.set(name, key.trim())?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Copies a `<provider>-api-key` keychain item into the auth file.
    pub fn import_keychain(&self, route: Route) -> Result<bool> {
        match (self.keychain)(&key_name(route))? {
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

    fn no_keychain() -> KeychainReader {
        Arc::new(|_| Ok(None))
    }

    #[test]
    fn keys_round_trip_through_the_store() {
        let store = Arc::new(MemoryStore::default());
        let creds = Credentials::for_tests(store.clone(), no_keychain());
        assert!(
            creds.api_key(Route::Openai).is_err(),
            "misses aren't cached"
        );
        creds
            .set_key(Route::Anthropic, "sk-ant-own-111111111111")
            .unwrap();
        assert_eq!(
            creds.api_key(Route::Anthropic).unwrap(),
            "sk-ant-own-111111111111"
        );
        assert_eq!(creds.lookup(Route::Anthropic).unwrap().1, KeySource::File);
        assert_eq!(
            store.get("anthropic-api-key").unwrap().as_deref(),
            Some("sk-ant-own-111111111111")
        );
        assert!(creds.remove_key(Route::Anthropic).unwrap());
        assert!(!creds.remove_key(Route::Anthropic).unwrap());
        let err = creds.api_key(Route::Openai).unwrap_err().to_string();
        assert!(err.contains("pathfinder auth set openai"));
        assert!(err.contains("OPENAI_API_KEY"));
    }

    #[test]
    fn import_copies_the_keychain_item() {
        let store = Arc::new(MemoryStore::default());
        let keychain: KeychainReader = Arc::new(|service| {
            Ok((service == "openai-api-key").then(|| "sk-proj-abcdefghijklmnop\n".to_string()))
        });
        let creds = Credentials::for_tests(store, keychain);
        assert!(
            creds.lookup(Route::Openai).is_none(),
            "keychain is never read on lookup"
        );
        assert!(creds.import_keychain(Route::Openai).unwrap());
        assert!(!creds.import_keychain(Route::Anthropic).unwrap());
        assert_eq!(
            creds.api_key(Route::Openai).unwrap(),
            "sk-proj-abcdefghijklmnop"
        );
        assert_eq!(mask("sk-proj-abcdefghijklmnop"), "sk-…mnop");
        assert!(check_key_shape("short").is_err());
        assert!(check_key_shape("sk-proj-abc def-ghijklmnopqr").is_err());
        assert!(check_key_shape("sk-proj-abcdefghijklmnop").is_ok());
    }
}
