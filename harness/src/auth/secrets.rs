//! Where secrets live: `~/.config/harness/auth.json`, readable only by you,
//! like Codex's `auth.json`. A plain file instead of the OS keychain, because
//! macOS asks again after every rebuild of an unsigned binary. Memory in tests.

use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Mutex;

use anyhow::{Context, Result};

pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>>;
    fn set(&self, name: &str, secret: &str) -> Result<()>;
    /// Returns whether an entry was deleted.
    fn delete(&self, name: &str) -> Result<bool>;
}

/// A flat JSON object of name → secret, written atomically with mode 0600.
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn load(&self) -> Result<BTreeMap<String, String>> {
        match fs::read_to_string(&self.path) {
            Ok(raw) if raw.trim().is_empty() => Ok(BTreeMap::new()),
            Ok(raw) => serde_json::from_str(&raw)
                .with_context(|| format!("reading {}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e).with_context(|| format!("reading {}", self.path.display())),
        }
    }

    fn save(&self, entries: &BTreeMap<String, String>) -> Result<()> {
        let dir = self.path.parent().context("auth file has no directory")?;
        fs::create_dir_all(dir)?;
        let tmp = self
            .path
            .with_extension(format!("json.{}", std::process::id()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        file.write_all(serde_json::to_string_pretty(entries)?.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, &self.path).with_context(|| format!("writing {}", self.path.display()))
    }
}

impl SecretStore for FileStore {
    fn get(&self, name: &str) -> Result<Option<String>> {
        Ok(self.load()?.remove(name))
    }

    fn set(&self, name: &str, secret: &str) -> Result<()> {
        let mut entries = self.load()?;
        entries.insert(name.to_string(), secret.to_string());
        self.save(&entries)
    }

    fn delete(&self, name: &str) -> Result<bool> {
        let mut entries = self.load()?;
        if entries.remove(name).is_none() {
            return Ok(false);
        }
        self.save(&entries)?;
        Ok(true)
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, String>>,
}

#[cfg(test)]
impl SecretStore for MemoryStore {
    fn get(&self, name: &str) -> Result<Option<String>> {
        Ok(self.entries.lock().unwrap().get(name).cloned())
    }

    fn set(&self, name: &str, secret: &str) -> Result<()> {
        self.entries
            .lock()
            .unwrap()
            .insert(name.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<bool> {
        Ok(self.entries.lock().unwrap().remove(name).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trips_with_private_mode() {
        let dir = std::env::temp_dir().join(format!("harness-auth-{}", std::process::id()));
        let store = FileStore::new(dir.join("auth.json"));
        assert_eq!(store.get("openai").unwrap(), None);
        store.set("openai", "sk-1").unwrap();
        store.set("chatgpt", "{\"a\":1}").unwrap();
        assert_eq!(store.get("openai").unwrap().as_deref(), Some("sk-1"));
        assert!(store.delete("openai").unwrap());
        assert!(!store.delete("openai").unwrap());
        assert_eq!(store.get("chatgpt").unwrap().as_deref(), Some("{\"a\":1}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::remove_dir_all(dir).ok();
    }
}
