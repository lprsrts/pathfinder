use std::env;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The CLI a model runs through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Pi,
    Claude,
    Codex,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }
}

/// One entry in the model picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub label: String,
    pub backend: Backend,
    /// Model name passed to the CLI. Empty means the CLI's own default.
    #[serde(default)]
    pub model: String,
    /// Thinking or reasoning effort, in the CLI's own terms (e.g. `low`, `high`).
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_pi_bin")]
    pub pi_bin: String,
    #[serde(default = "default_claude_bin")]
    pub claude_bin: String,
    #[serde(default = "default_codex_bin")]
    pub codex_bin: String,
    /// The models offered in Zed's pickers.
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    /// Default picks (entry ids) for new sessions.
    #[serde(default)]
    pub system1: Option<String>,
    #[serde(default)]
    pub system2: Option<String>,
    // Older single-backend fields, still honored when `models` is empty.
    #[serde(default)]
    pub system1_model: Option<String>,
    #[serde(default = "default_s1_thinking")]
    pub system1_thinking: String,
    #[serde(default)]
    pub system2_model: Option<String>,
    #[serde(default = "default_s2_thinking")]
    pub system2_thinking: String,
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,
}

fn default_pi_bin() -> String {
    "pi".to_string()
}

fn default_claude_bin() -> String {
    "claude".to_string()
}

fn default_codex_bin() -> String {
    "codex".to_string()
}

fn default_s1_thinking() -> String {
    "low".to_string()
}

fn default_s2_thinking() -> String {
    "high".to_string()
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_config_path() -> PathBuf {
    match env::var_os("XDG_CONFIG_HOME") {
        Some(xdg) => PathBuf::from(xdg).join("harness").join("config.json"),
        None => home_dir()
            .join(".config")
            .join("harness")
            .join("config.json"),
    }
}

fn default_db_path() -> PathBuf {
    match env::var_os("XDG_DATA_HOME") {
        Some(xdg) => PathBuf::from(xdg).join("harness").join("harness.sqlite"),
        None => home_dir()
            .join(".local")
            .join("share")
            .join("harness")
            .join("harness.sqlite"),
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pi_bin: default_pi_bin(),
            claude_bin: default_claude_bin(),
            codex_bin: default_codex_bin(),
            models: Vec::new(),
            system1: None,
            system2: None,
            system1_model: None,
            system1_thinking: default_s1_thinking(),
            system2_model: None,
            system2_thinking: default_s2_thinking(),
            db_path: default_db_path(),
        }
    }
}

impl Config {
    /// Reads the config file and env overrides. Called per turn, so edits
    /// apply without restarting the editor.
    pub fn load() -> Self {
        let path = default_config_path();
        let mut cfg = match fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str::<Config>(&raw).unwrap_or_else(|err| {
                tracing::warn!("ignoring unreadable {}: {err}", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        };

        let set = |var: &str, slot: &mut String| {
            if let Ok(v) = env::var(var) {
                if !v.trim().is_empty() {
                    *slot = v;
                }
            }
        };
        set("HARNESS_PI_BIN", &mut cfg.pi_bin);
        set("HARNESS_CLAUDE_BIN", &mut cfg.claude_bin);
        set("HARNESS_CODEX_BIN", &mut cfg.codex_bin);
        set("HARNESS_S1_THINKING", &mut cfg.system1_thinking);
        set("HARNESS_S2_THINKING", &mut cfg.system2_thinking);
        if let Ok(v) = env::var("HARNESS_S1_MODEL") {
            if !v.trim().is_empty() {
                cfg.system1_model = Some(v);
            }
        }
        if let Ok(v) = env::var("HARNESS_S2_MODEL") {
            if !v.trim().is_empty() {
                cfg.system2_model = Some(v);
            }
        }
        if let Ok(v) = env::var("HARNESS_DB_PATH") {
            if !v.trim().is_empty() {
                cfg.db_path = PathBuf::from(v);
            }
        }
        cfg
    }

    /// The picker list. Falls back to pi entries built from the older fields.
    pub fn catalog(&self) -> Vec<ModelEntry> {
        if !self.models.is_empty() {
            return self.models.clone();
        }
        let legacy = |id: &str, tier: &str, model: &Option<String>, effort: &str| ModelEntry {
            id: id.to_string(),
            label: format!("{} ({tier})", model.as_deref().unwrap_or("pi default")),
            backend: Backend::Pi,
            model: model.clone().unwrap_or_default(),
            effort: (!effort.is_empty()).then(|| effort.to_string()),
        };
        vec![
            legacy(
                "pi-system1",
                "System 1",
                &self.system1_model,
                &self.system1_thinking,
            ),
            legacy(
                "pi-system2",
                "System 2",
                &self.system2_model,
                &self.system2_thinking,
            ),
        ]
    }

    pub fn entry(&self, id: &str) -> Option<ModelEntry> {
        self.catalog().into_iter().find(|m| m.id == id)
    }

    /// The entry for a tier: the session's pick if still listed, else the
    /// configured default, else the first (System 1) or last (System 2) entry.
    pub fn pick(&self, choice: Option<&str>, system2: bool) -> ModelEntry {
        let catalog = self.catalog();
        let default = if system2 {
            &self.system2
        } else {
            &self.system1
        };
        choice
            .and_then(|id| self.entry(id))
            .or_else(|| default.as_deref().and_then(|id| self.entry(id)))
            .unwrap_or_else(|| {
                if system2 {
                    catalog.last().cloned().unwrap()
                } else {
                    catalog.first().cloned().unwrap()
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, backend: Backend) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            label: id.into(),
            backend,
            model: id.into(),
            effort: None,
        }
    }

    #[test]
    fn legacy_fields_become_pi_entries() {
        let cfg = Config {
            system1_model: Some("openai/gpt-6-luna".into()),
            ..Config::default()
        };
        let catalog = cfg.catalog();
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].model, "openai/gpt-6-luna");
        assert_eq!(catalog[0].effort.as_deref(), Some("low"));
        assert_eq!(catalog[1].model, "");
        assert_eq!(cfg.pick(None, true).id, "pi-system2");
    }

    #[test]
    fn pick_prefers_session_then_default_then_position() {
        let cfg = Config {
            models: vec![
                entry("a", Backend::Pi),
                entry("b", Backend::Claude),
                entry("c", Backend::Codex),
            ],
            system1: Some("b".into()),
            ..Config::default()
        };
        assert_eq!(cfg.pick(Some("c"), false).id, "c");
        assert_eq!(cfg.pick(Some("gone"), false).id, "b");
        assert_eq!(cfg.pick(None, false).id, "b");
        assert_eq!(cfg.pick(None, true).id, "c");
    }
}
