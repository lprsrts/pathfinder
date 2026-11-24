//! Where secrets live: the OS keychain (macOS Keychain, Windows Credential
//! Manager, Linux Secret Service), or memory in tests.

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex;

use anyhow::{Result, anyhow};

pub trait SecretStore: Send + Sync {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>>;
    fn set(&self, service: &str, account: &str, secret: &str) -> Result<()>;
    /// Returns whether an entry was deleted.
    fn delete(&self, service: &str, account: &str) -> Result<bool>;
}

pub struct KeyringStore;

impl SecretStore for KeyringStore {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(service, account).map_err(|e| anyhow!("keychain: {e}"))?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow!("keychain: {e}")),
        }
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> Result<()> {
        keyring::Entry::new(service, account)
            .and_then(|e| e.set_password(secret))
            .map_err(|e| anyhow!("keychain: {e}"))
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool> {
        let entry = keyring::Entry::new(service, account).map_err(|e| anyhow!("keychain: {e}"))?;
        match entry.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(anyhow!("keychain: {e}")),
        }
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<(String, String), String>>,
}

#[cfg(test)]
impl SecretStore for MemoryStore {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(&(service.to_string(), account.to_string()))
            .cloned())
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> Result<()> {
        self.entries.lock().unwrap().insert(
            (service.to_string(), account.to_string()),
            secret.to_string(),
        );
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .remove(&(service.to_string(), account.to_string()))
            .is_some())
    }
}
