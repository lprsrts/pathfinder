use std::env;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

/// Where a model runs: an external CLI (with its own login) or a provider API
/// that Pathfinder calls itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// Claude Code CLI (`claude -p`), using the CLI's own login.
    Claude,
    /// Codex CLI (`codex exec`), using the CLI's own login.
    Codex,
    /// Anthropic Messages API with an API key.
    Anthropic,
    /// OpenAI Responses API with an API key.
    Openai,
}

impl Route {
    /// Group label in the model pickers.
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Anthropic => "Anthropic API",
            Self::Openai => "OpenAI API",
        }
    }

    pub fn is_cli(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }
}

/// One entry in the model picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub label: String,
    #[serde(alias = "backend")]
    pub route: Route,
    /// Model name passed to the route. Empty means the route's own default.
    #[serde(default)]
    pub model: String,
    /// Thinking or reasoning effort (e.g. `low`, `high`).
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_claude_bin")]
    pub claude_bin: String,
    #[serde(default = "default_codex_bin")]
    pub codex_bin: String,
    /// The models offered in Zed's pickers. Entries with an unknown route
    /// (e.g. the removed `pi`) are skipped with a warning.
    #[serde(default, deserialize_with = "lenient_models")]
    pub models: Vec<ModelEntry>,
    /// Default picks (entry ids) for new sessions.
    #[serde(default)]
    pub system1: Option<String>,
    #[serde(default)]
    pub system2: Option<String>,
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,
}

fn lenient_models<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<ModelEntry>, D::Error> {
    let raw: Vec<serde_json::Value> = Vec::deserialize(de)?;
    Ok(raw
        .into_iter()
        .filter_map(|v| match serde_json::from_value::<ModelEntry>(v.clone()) {
            Ok(entry) => Some(entry),
            Err(err) => {
                tracing::warn!("skipping model entry {v}: {err}");
                None
            }
        })
        .collect())
}

fn default_claude_bin() -> String {
    "claude".to_string()
}

fn default_codex_bin() -> String {
    "codex".to_string()
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
            claude_bin: default_claude_bin(),
            codex_bin: default_codex_bin(),
            models: Vec::new(),
            system1: None,
            system2: None,
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
        set("HARNESS_CLAUDE_BIN", &mut cfg.claude_bin);
        set("HARNESS_CODEX_BIN", &mut cfg.codex_bin);
        if let Ok(v) = env::var("HARNESS_DB_PATH") {
            if !v.trim().is_empty() {
                cfg.db_path = PathBuf::from(v);
            }
        }
        cfg
    }

    /// The picker list.
    pub fn catalog(&self) -> Vec<ModelEntry> {
        self.models.clone()
    }

    pub fn entry(&self, id: &str) -> Option<ModelEntry> {
        self.catalog().into_iter().find(|m| m.id == id)
    }

    /// The entry for a tier: the session's pick if still listed, else the
    /// configured default, else the first (System 1) or last (System 2)
    /// entry. None when no model is configured.
    pub fn pick(&self, choice: Option<&str>, system2: bool) -> Option<ModelEntry> {
        let default = if system2 {
            &self.system2
        } else {
            &self.system1
        };
        choice
            .and_then(|id| self.entry(id))
            .or_else(|| default.as_deref().and_then(|id| self.entry(id)))
            .or_else(|| {
                let catalog = self.catalog();
                if system2 {
                    catalog.last().cloned()
                } else {
                    catalog.first().cloned()
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, route: Route) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            label: id.into(),
            route,
            model: id.into(),
            effort: None,
        }
    }

    #[test]
    fn unknown_routes_are_skipped_and_backend_is_an_alias() {
        let cfg: Config = serde_json::from_str(
            r#"{"models":[
                {"id":"pi-luna","label":"x","backend":"pi","model":"m"},
                {"id":"haiku","label":"Haiku","backend":"claude","model":"haiku"},
                {"id":"sonnet","label":"Sonnet","route":"anthropic","model":"claude-sonnet"}
            ]}"#,
        )
        .unwrap();
        let ids: Vec<&str> = cfg.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["haiku", "sonnet"]);
        assert_eq!(cfg.models[1].route, Route::Anthropic);
    }

    #[test]
    fn pick_prefers_session_then_default_then_position() {
        let cfg = Config {
            models: vec![
                entry("a", Route::Claude),
                entry("b", Route::Anthropic),
                entry("c", Route::Codex),
            ],
            system1: Some("b".into()),
            ..Config::default()
        };
        assert_eq!(cfg.pick(Some("c"), false).unwrap().id, "c");
        assert_eq!(cfg.pick(Some("gone"), false).unwrap().id, "b");
        assert_eq!(cfg.pick(None, false).unwrap().id, "b");
        assert_eq!(cfg.pick(None, true).unwrap().id, "c");
        assert!(Config::default().pick(None, true).is_none());
    }
}
