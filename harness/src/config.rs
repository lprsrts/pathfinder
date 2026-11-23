use std::env;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_pi_bin")]
    pub pi_bin: String,
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
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("harness").join("config.json")
    } else {
        home_dir()
            .join(".config")
            .join("harness")
            .join("config.json")
    }
}

fn default_db_path() -> PathBuf {
    if let Some(xdg) = env::var_os("XDG_DATA_HOME") {
        PathBuf::from(xdg).join("harness").join("harness.sqlite")
    } else {
        home_dir()
            .join(".local")
            .join("share")
            .join("harness")
            .join("harness.sqlite")
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pi_bin: default_pi_bin(),
            system1_model: None,
            system1_thinking: default_s1_thinking(),
            system2_model: None,
            system2_thinking: default_s2_thinking(),
            db_path: default_db_path(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let mut cfg = fs::read_to_string(default_config_path())
            .ok()
            .and_then(|raw| serde_json::from_str::<Config>(&raw).ok())
            .unwrap_or_default();

        if let Ok(v) = env::var("HARNESS_PI_BIN") {
            if !v.trim().is_empty() {
                cfg.pi_bin = v;
            }
        }
        if let Ok(v) = env::var("HARNESS_S1_MODEL") {
            if !v.trim().is_empty() {
                cfg.system1_model = Some(v);
            }
        }
        if let Ok(v) = env::var("HARNESS_S1_THINKING") {
            if !v.trim().is_empty() {
                cfg.system1_thinking = v;
            }
        }
        if let Ok(v) = env::var("HARNESS_S2_MODEL") {
            if !v.trim().is_empty() {
                cfg.system2_model = Some(v);
            }
        }
        if let Ok(v) = env::var("HARNESS_S2_THINKING") {
            if !v.trim().is_empty() {
                cfg.system2_thinking = v;
            }
        }
        if let Ok(v) = env::var("HARNESS_DB_PATH") {
            if !v.trim().is_empty() {
                cfg.db_path = PathBuf::from(v);
            }
        }

        cfg
    }
}
