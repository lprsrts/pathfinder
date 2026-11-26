use std::fs;
use std::path::{Path, PathBuf};

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
    /// OpenAI Responses API through Sign in with ChatGPT (plan usage).
    Chatgpt,
    /// An OpenAI-compatible Chat Completions server from `endpoints`: a
    /// self-hosted vLLM, Ollama, llama.cpp or LM Studio, or a hosted
    /// open-model service.
    Endpoint,
}

impl Route {
    /// Group label in the model pickers.
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Anthropic => "Anthropic API",
            Self::Openai => "OpenAI API",
            Self::Chatgpt => "ChatGPT",
            Self::Endpoint => "Endpoint",
        }
    }

    /// Order for picking a default: the own harness first (plan usage, then
    /// your own servers, then API keys), the external CLIs last.
    pub fn preference(self) -> u8 {
        match self {
            Self::Chatgpt => 0,
            Self::Endpoint => 1,
            Self::Anthropic => 2,
            Self::Openai => 3,
            Self::Claude => 4,
            Self::Codex => 5,
        }
    }

    pub fn is_cli(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }

    /// Routes that use API keys.
    pub const API: [Route; 2] = [Route::Anthropic, Route::Openai];

    /// Routes whose models Pathfinder discovers, in picker order.
    pub const DISCOVERED: [Route; 3] = [Route::Chatgpt, Route::Anthropic, Route::Openai];

    /// Provider id used in commands and auth file entries.
    pub fn provider_id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Chatgpt => "chatgpt",
            Self::Endpoint => "endpoint",
        }
    }

    pub fn from_provider_id(id: &str) -> Option<Route> {
        Self::API.into_iter().find(|r| r.provider_id() == id)
    }

    /// Provider name for prompts ("Anthropic", "OpenAI").
    pub fn provider_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Anthropic => "Anthropic",
            Self::Openai => "OpenAI",
            Self::Chatgpt => "ChatGPT",
            Self::Endpoint => "your endpoint",
        }
    }

    /// Environment variable that overrides stored keys.
    pub fn key_env(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::Openai => Some("OPENAI_API_KEY"),
            Self::Claude | Self::Codex | Self::Chatgpt | Self::Endpoint => None,
        }
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
    /// For the `endpoint` route: which entry of `endpoints`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

/// An OpenAI-compatible server: `{"id": "lab", "base_url":
/// "http://gpu-box:8000/v1", "key": "lab-api-key", "models": ["..."]}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// Short id, used in model ids (`lab:<model>`).
    pub id: String,
    /// Picker group name; the id when unset.
    #[serde(default)]
    pub label: Option<String>,
    /// Up to and including `/v1`.
    pub base_url: String,
    /// Auth file entry holding the key (`pathfinder auth import <name>` copies
    /// a keychain item); none for servers without auth.
    #[serde(default)]
    pub key: Option<String>,
    /// Models to offer. Empty: whatever the server lists (`/models`).
    #[serde(default)]
    pub models: Vec<String>,
    /// Fields merged into every request body, for server-specific options
    /// (on OpenRouter, `{"provider": {"require_parameters": true}}` keeps
    /// decision requests on providers that return log probabilities).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

/// Merges `extra`'s top-level fields into a request body.
pub fn merge_extra(body: &mut serde_json::Value, extra: Option<&serde_json::Value>) {
    if let (Some(serde_json::Value::Object(extra)), serde_json::Value::Object(body)) = (extra, body)
    {
        for (k, v) in extra {
            body.insert(k.clone(), v.clone());
        }
    }
}

/// The decision service on an endpoint: a model that returns token log
/// probabilities, used for the gate and step routing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionEndpoint {
    pub endpoint: String,
    pub model: String,
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
    /// Default effort for the main model (`low`, `medium`, `high`).
    #[serde(default)]
    pub effort: Option<String>,
    /// Default effort for the fast model (`low`, `medium`, `high`).
    #[serde(default)]
    pub fast_effort: Option<String>,
    /// Default Decisions switch (`true` on, `false` off; `None` = on).
    #[serde(default)]
    pub decisions_on: Option<bool>,
    /// Default decision service id (`typesafe`, `openai`, `gemini`).
    #[serde(default)]
    pub decision_model: Option<String>,
    /// Default review checks (`off`, `light`, `normal`).
    #[serde(default)]
    pub checks: Option<String>,
    /// Add the models your API keys give access to (cached, refreshed daily).
    #[serde(default = "default_true")]
    pub discover: bool,
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,
    /// OpenAI-compatible servers (self-hosted or hosted open models).
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    /// Decisions from an endpoint's model instead of a hosted service.
    #[serde(default)]
    pub decisions: Option<DecisionEndpoint>,
    /// Only endpoints: no model or decision request leaves for a provider's
    /// cloud, for on-prem use. Pinned and discovered cloud models are hidden.
    #[serde(default)]
    pub local_only: bool,
    /// Ask before every agent command, even sandboxed ones. Off: commands
    /// run in the sandbox without asking, and only those needing the
    /// network or to leave the sandbox ask.
    #[serde(default)]
    pub ask_every_command: bool,
    /// Longest verbatim quote shown per hit, in characters.
    #[serde(default = "default_quote_cap")]
    pub quote_cap: usize,
    /// Who searches the sources for a question: `model` (the model proposes
    /// hits, code verifies them) or `code` (keyword search only, no model).
    #[serde(default = "default_sources_search")]
    pub sources_search: String,
}

fn default_quote_cap() -> usize {
    crate::sources::QUOTE_CAP
}

fn default_sources_search() -> String {
    "model".into()
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

fn default_true() -> bool {
    true
}

fn default_claude_bin() -> String {
    "claude".to_string()
}

fn default_codex_bin() -> String {
    "codex".to_string()
}

/// Fields to update in `config.json` defaults: `Some(Some(v))` sets, `Some(None)`
/// clears, and `None` leaves the existing setting untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefaultSettings {
    pub system1: Option<Option<String>>,
    pub system2: Option<Option<String>>,
    pub effort: Option<Option<String>>,
    pub fast_effort: Option<Option<String>>,
    pub decisions_on: Option<Option<bool>>,
    pub decision_model: Option<Option<String>>,
    pub checks: Option<Option<String>>,
}

/// Reads `path` as a JSON object (starting empty only when missing), applies
/// `edit`, and replaces the file atomically.
fn update_config_json(
    path: &Path,
    edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut root: serde_json::Value = match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("{} isn't valid JSON: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => anyhow::bail!("can't read {}: {e}. Nothing was changed.", path.display()),
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} isn't a JSON object", path.display()))?;
    edit(obj)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&root)? + "\n")?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// [`Config::add_endpoint`] for the file at `path`. Only a missing file starts
/// empty: one that can't be read is an error, never overwritten, and the new
/// file replaces the old one in a single rename.
fn add_endpoint_at(path: &Path, ep: &Endpoint) -> anyhow::Result<()> {
    update_config_json(path, |obj| {
        let list = obj
            .entry("endpoints")
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("`endpoints` in {} isn't a list", path.display()))?;
        list.retain(|e| e.get("id").and_then(|v| v.as_str()) != Some(ep.id.as_str()));
        let mut entry = serde_json::to_value(ep)?;
        if let Some(e) = entry.as_object_mut() {
            e.retain(|_, v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()));
        }
        list.push(entry);
        Ok(())
    })
}

fn set_optional_json<T: Serialize>(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    patch: &Option<Option<T>>,
) -> anyhow::Result<()> {
    if let Some(val) = patch {
        match val {
            Some(v) => {
                obj.insert(key.to_string(), serde_json::to_value(v)?);
            }
            None => {
                obj.remove(key);
            }
        }
    }
    Ok(())
}

pub fn save_defaults_at(path: &Path, defaults: &DefaultSettings) -> anyhow::Result<()> {
    update_config_json(path, |obj| {
        set_optional_json(obj, "system1", &defaults.system1)?;
        set_optional_json(obj, "system2", &defaults.system2)?;
        set_optional_json(obj, "effort", &defaults.effort)?;
        set_optional_json(obj, "fast_effort", &defaults.fast_effort)?;
        set_optional_json(obj, "decisions_on", &defaults.decisions_on)?;
        set_optional_json(obj, "decision_model", &defaults.decision_model)?;
        set_optional_json(obj, "checks", &defaults.checks)?;
        Ok(())
    })
}

pub fn default_config_path() -> PathBuf {
    crate::paths::config_dir().join("config.json")
}

fn default_db_path() -> PathBuf {
    crate::paths::data_dir().join("harness.sqlite")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            claude_bin: default_claude_bin(),
            codex_bin: default_codex_bin(),
            models: Vec::new(),
            system1: None,
            system2: None,
            effort: None,
            fast_effort: None,
            decisions_on: None,
            decision_model: None,
            checks: None,
            discover: true,
            db_path: default_db_path(),
            endpoints: Vec::new(),
            decisions: None,
            local_only: false,
            ask_every_command: false,
            quote_cap: default_quote_cap(),
            sources_search: default_sources_search(),
        }
    }
}

impl Config {
    /// Reads the config file and env overrides. Called per turn, so edits
    /// apply without restarting the editor.
    /// Adds `ep` to `config.json` (replacing one with the same id), keeping
    /// everything else in the file as it is.
    pub fn add_endpoint(ep: &Endpoint) -> anyhow::Result<()> {
        add_endpoint_at(&default_config_path(), ep)
    }

    /// Updates default model/effort/decision/check fields in `config.json`,
    /// preserving all other settings.
    pub fn save_defaults(defaults: &DefaultSettings) -> anyhow::Result<()> {
        save_defaults_at(&default_config_path(), defaults)
    }

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
            if let Ok(v) = crate::envvar::get(var)
                && !v.trim().is_empty()
            {
                *slot = v;
            }
        };
        set("PATHFINDER_CLAUDE_BIN", &mut cfg.claude_bin);
        set("PATHFINDER_CODEX_BIN", &mut cfg.codex_bin);
        if let Ok(v) = crate::envvar::get("PATHFINDER_DB_PATH")
            && !v.trim().is_empty()
        {
            cfg.db_path = PathBuf::from(v);
        }
        cfg
    }

    /// The picker list: pinned models, then discovered ones not already
    /// pinned.
    pub fn catalog(&self) -> Vec<ModelEntry> {
        let cache = crate::catalog::ModelCache::load();
        let discovered = if self.discover {
            cache.entries()
        } else {
            Vec::new()
        };
        let mut all = self.merged(discovered);
        all.extend(self.endpoint_entries(&cache));
        if self.local_only {
            all.retain(|m| m.route == Route::Endpoint);
        }
        all
    }

    pub fn endpoint(&self, id: &str) -> Option<&Endpoint> {
        self.endpoints.iter().find(|e| e.id == id)
    }

    /// Each endpoint's models: the configured list, else what its server
    /// listed at the last refresh. Ids like `lab:qwen3-32b`.
    fn endpoint_entries(&self, cache: &crate::catalog::ModelCache) -> Vec<ModelEntry> {
        self.endpoints
            .iter()
            .flat_map(|ep| {
                let models: Vec<String> = if ep.models.is_empty() {
                    cache
                        .providers
                        .get(&format!("endpoint:{}", ep.id))
                        .into_iter()
                        .flatten()
                        .map(|m| m.id.clone())
                        .collect()
                } else {
                    ep.models.clone()
                };
                models.into_iter().map(move |model| ModelEntry {
                    id: format!("{}:{model}", ep.id),
                    label: model.clone(),
                    route: Route::Endpoint,
                    model,
                    effort: None,
                    endpoint: Some(ep.id.clone()),
                })
            })
            .filter(|m| !self.models.iter().any(|p| p.id == m.id))
            .collect()
    }

    fn merged(&self, discovered: Vec<ModelEntry>) -> Vec<ModelEntry> {
        let mut out = self.models.clone();
        for entry in discovered {
            if !out
                .iter()
                .any(|m| m.route == entry.route && m.model == entry.model)
            {
                out.push(entry);
            }
        }
        out
    }

    pub fn entry(&self, id: &str) -> Option<ModelEntry> {
        self.catalog().into_iter().find(|m| m.id == id)
    }

    /// The entry for a tier: the session's pick if still listed, else the
    /// configured default, else the first (System 1) or last (System 2)
    /// entry. None when no model is configured.
    pub fn pick(&self, choice: Option<&str>, system2: bool) -> Option<ModelEntry> {
        pick_from(&self.catalog(), self, choice, system2)
    }
}

/// Model-name hints for a sensible default per tier when none is configured.
const FAST_HINTS: [&str; 4] = ["haiku", "luna", "mini", "flash"];
const MAIN_HINTS: [&str; 3] = ["sonnet", "sol", "opus"];

fn pick_from(
    catalog: &[ModelEntry],
    cfg: &Config,
    choice: Option<&str>,
    system2: bool,
) -> Option<ModelEntry> {
    let find = |id: &str| catalog.iter().find(|m| m.id == id).cloned();
    let default = if system2 { &cfg.system2 } else { &cfg.system1 };
    let hints: &[&str] = if system2 { &MAIN_HINTS } else { &FAST_HINTS };
    // Pathfinder's own harness before the CLIs: within each route in this
    // order, the first model a hint names.
    let mut routes: Vec<Route> = catalog.iter().map(|m| m.route).collect();
    routes.sort_by_key(|r| r.preference());
    routes.dedup();
    choice
        .and_then(find)
        .or_else(|| default.as_deref().and_then(find))
        .or_else(|| {
            routes.iter().find_map(|route| {
                hints.iter().find_map(|h| {
                    catalog
                        .iter()
                        .find(|m| m.route == *route && m.model.contains(h))
                        .cloned()
                })
            })
        })
        .or_else(|| {
            let own: Vec<&ModelEntry> = catalog.iter().filter(|m| !m.route.is_cli()).collect();
            let pool = if own.is_empty() {
                catalog.iter().collect()
            } else {
                own
            };
            if system2 {
                pool.last().map(|m| (*m).clone())
            } else {
                pool.first().map(|m| (*m).clone())
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_a_server_keeps_the_rest_of_config_json() {
        let dir = std::env::temp_dir().join(format!("pf-cfg-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let ep = |id: &str| Endpoint {
            id: id.into(),
            label: None,
            base_url: "http://localhost:11434/v1".into(),
            key: None,
            models: Vec::new(),
            extra: None,
        };
        fs::write(
            &path,
            r#"{"quote_cap": 30, "endpoints": [{"id": "old", "base_url": "http://x/v1"}]}"#,
        )
        .unwrap();
        add_endpoint_at(&path, &ep("lab")).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["quote_cap"], 30);
        assert_eq!(v["endpoints"].as_array().unwrap().len(), 2);
        // A file that can't be read as text is left alone, not replaced.
        let mut bytes = br#"{"label": "Gr"#.to_vec();
        bytes.extend([0xfc, b'n', b'"', b'}']);
        fs::write(&path, &bytes).unwrap();
        assert!(add_endpoint_at(&path, &ep("lab")).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        // A missing file starts empty.
        fs::remove_file(&path).unwrap();
        add_endpoint_at(&path, &ep("lab")).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("\"lab\""));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn endpoints_list_their_models_and_local_only_hides_the_cloud() {
        let mut cache = crate::catalog::ModelCache::default();
        cache.providers.insert(
            "endpoint:lab".into(),
            vec![crate::catalog::CachedModel {
                id: "qwen-32b".into(),
                label: "qwen-32b".into(),
            }],
        );
        let cfg = Config {
            endpoints: vec![
                Endpoint {
                    id: "lab".into(),
                    label: None,
                    base_url: "http://gpu-box:8000/v1".into(),
                    key: None,
                    models: vec![],
                    extra: None,
                },
                Endpoint {
                    id: "openrouter".into(),
                    label: None,
                    base_url: "https://openrouter.ai/api/v1".into(),
                    key: Some("openrouter-api-key".into()),
                    models: vec!["some/open-model".into()],
                    extra: None,
                },
            ],
            ..Config::default()
        };
        let entries = cfg.endpoint_entries(&cache);
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["lab:qwen-32b", "openrouter:some/open-model"]);
        assert_eq!(entries[0].route, Route::Endpoint);
        assert_eq!(entries[0].endpoint.as_deref(), Some("lab"));
        assert_eq!(entries[0].model, "qwen-32b");

        let parsed: Config = serde_json::from_str(
            r#"{"local_only": true, "decisions": {"endpoint": "lab", "model": "small"},
                "endpoints": [{"id": "lab", "base_url": "http://gpu-box:8000/v1"}]}"#,
        )
        .unwrap();
        assert!(parsed.local_only);
        assert_eq!(parsed.decisions.as_ref().unwrap().model, "small");
        assert!(parsed.endpoint("lab").is_some());
    }

    fn entry(id: &str, route: Route) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            label: id.into(),
            route,
            model: id.into(),
            effort: None,
            endpoint: None,
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
    fn discovered_models_merge_and_hint_defaults() {
        let cfg = Config {
            models: vec![entry("pinned-sonnet", Route::Anthropic)],
            ..Config::default()
        };
        let mut haiku = entry("anthropic:claude-haiku-5-5", Route::Anthropic);
        haiku.model = "claude-haiku-5-5".into();
        let mut dup = entry("anthropic:pinned-sonnet", Route::Anthropic);
        dup.model = "pinned-sonnet".into();
        let merged = cfg.merged(vec![dup, haiku]);
        let ids: Vec<&str> = merged.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["pinned-sonnet", "anthropic:claude-haiku-5-5"]);
        assert_eq!(
            pick_from(&merged, &cfg, None, false).unwrap().id,
            "anthropic:claude-haiku-5-5"
        );
        assert_eq!(
            pick_from(&merged, &cfg, None, true).unwrap().id,
            "pinned-sonnet"
        );
    }

    #[test]
    fn defaults_prefer_the_own_harness_and_plan_usage() {
        let m = |id: &str, model: &str, route| ModelEntry {
            model: model.into(),
            ..entry(id, route)
        };
        let cfg = Config {
            models: vec![
                m("claude-haiku", "haiku", Route::Claude),
                m("claude-opus", "opus", Route::Claude),
                m("anthropic:haiku", "claude-haiku-5-5", Route::Anthropic),
                m("chatgpt:sol", "gpt-6.2-sol", Route::Chatgpt),
                m("chatgpt:luna", "gpt-6-luna", Route::Chatgpt),
            ],
            discover: false,
            ..Config::default()
        };
        assert_eq!(cfg.pick(None, false).unwrap().id, "chatgpt:luna");
        assert_eq!(cfg.pick(None, true).unwrap().id, "chatgpt:sol");
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
            discover: false,
            ..Config::default()
        };
        assert_eq!(cfg.pick(Some("c"), false).unwrap().id, "c");
        assert_eq!(cfg.pick(Some("gone"), false).unwrap().id, "b");
        assert_eq!(cfg.pick(None, false).unwrap().id, "b");
        // No main default: the own harness before the CLIs.
        assert_eq!(cfg.pick(None, true).unwrap().id, "b");
        let empty = Config {
            discover: false,
            ..Config::default()
        };
        assert!(empty.pick(None, true).is_none());
    }

    #[test]
    fn save_defaults_preserves_other_fields_and_clears_reset_values() {
        let dir = std::env::temp_dir().join(format!("pf-def-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(
            &path,
            r#"{"quote_cap": 42, "effort": "low", "checks": "normal"}"#,
        )
        .unwrap();

        save_defaults_at(
            &path,
            &DefaultSettings {
                system1: Some(Some("fast".into())),
                system2: Some(Some("main".into())),
                effort: Some(Some("high".into())),
                fast_effort: Some(Some("medium".into())),
                decisions_on: Some(Some(false)),
                decision_model: Some(Some("openai".into())),
                checks: Some(None),
            },
        )
        .unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        let cfg: Config = serde_json::from_str(&raw).unwrap();
        assert_eq!(cfg.quote_cap, 42);
        assert_eq!(cfg.system1.as_deref(), Some("fast"));
        assert_eq!(cfg.system2.as_deref(), Some("main"));
        assert_eq!(cfg.effort.as_deref(), Some("high"));
        assert_eq!(cfg.fast_effort.as_deref(), Some("medium"));
        assert_eq!(cfg.decisions_on, Some(false));
        assert_eq!(cfg.decision_model.as_deref(), Some("openai"));
        assert_eq!(cfg.checks, None);

        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(v.get("checks").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
