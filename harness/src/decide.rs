//! Decision models: a fast first pass that answers typed questions about a
//! chat message with calibrated probabilities, before any LLM runs. Three
//! hosted services share the idea (shared input, typed questions, no output
//! text), each with its own wire format:
//!
//! - OpenAI Decisions API (`POST /v1/decisions`, `gpt-6-luna`)
//! - Gemini Decision 1 (`POST /v1beta/decisions`, `gemini-decision-1`)
//! - TypeSafe Jev (`POST /v1/systemone`, `jev-1.14.0`)
//!
//! Pathfinder only asks choice questions.

use std::collections::HashMap;
use std::env;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::future::BoxFuture;
use serde_json::{Map, Value, json};

use crate::auth::Credentials;

/// A choice question: pick one of `options` (value, description).
#[derive(Debug, Clone)]
pub struct Question {
    pub name: String,
    pub instructions: String,
    pub options: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub choice: String,
    /// Probability of the chosen option.
    pub probability: f64,
}

#[derive(Debug, Clone)]
pub struct Decision {
    /// Which service answered, for the thinking line ("OpenAI Decisions").
    pub by: String,
    pub answers: HashMap<String, Answer>,
}

pub trait Decide: Send + Sync {
    /// `prefer`: a service id the thread picked. `Ok(None)` when no decision
    /// service is configured.
    fn decide<'a>(
        &'a self,
        input: &'a str,
        questions: &'a [Question],
        prefer: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<Decision>>>;

    /// Services with a key, in auto order, for the thread's picker.
    fn available(&self) -> Vec<Provider> {
        Vec::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Openai,
    Gemini,
    Typesafe,
}

impl Provider {
    /// Auto order: fastest and cheapest first.
    pub const ALL: [Provider; 3] = [Provider::Typesafe, Provider::Openai, Provider::Gemini];

    /// Used in `harness auth set <id>` and `HARNESS_DECISION`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Gemini => "gemini",
            Self::Typesafe => "typesafe",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Openai => "OpenAI Decisions (gpt-6-luna)",
            Self::Gemini => "Gemini Decision 1",
            Self::Typesafe => "TypeSafe Jev",
        }
    }

    /// Name on the toolbar button.
    pub fn short(self) -> &'static str {
        match self {
            Self::Openai => "OpenAI",
            Self::Gemini => "Gemini",
            Self::Typesafe => "Jev",
        }
    }

    /// The model it runs, shown on hover.
    pub fn model(self) -> &'static str {
        match self {
            Self::Openai => "gpt-6-luna",
            Self::Gemini => "gemini-decision-1",
            Self::Typesafe => "jev-1.14.0",
        }
    }

    /// Auth file entry, also the keychain item `auth import` reads.
    pub fn key_name(self) -> &'static str {
        match self {
            Self::Openai => "openai-api-key",
            Self::Gemini => "gemini-api-key",
            Self::Typesafe => "typesafe-api-key",
        }
    }

    pub fn key_env(self) -> &'static str {
        match self {
            Self::Openai => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Typesafe => "TYPESAFE_API_KEY",
        }
    }

    fn base_url(self) -> String {
        let (var, default) = match self {
            Self::Openai => ("HARNESS_OPENAI_BASE_URL", "https://api.openai.com/v1"),
            Self::Gemini => (
                "HARNESS_GEMINI_BASE_URL",
                "https://generativelanguage.googleapis.com/v1beta",
            ),
            Self::Typesafe => ("HARNESS_TYPESAFE_BASE_URL", "https://api.typesafe.ai/v1"),
        };
        env::var(var)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| default.to_string())
    }

    fn request(self, input: &str, questions: &[Question]) -> (String, Value) {
        let base = self.base_url();
        match self {
            Self::Openai => (
                format!("{base}/decisions"),
                json!({
                    "model": "gpt-6-luna",
                    "input": input,
                    "questions": questions.iter().map(|q| json!({
                        "type": "choice",
                        "name": q.name,
                        "instructions": q.instructions,
                        "choices": q.options.iter()
                            .map(|(v, d)| json!({"value": v, "description": d}))
                            .collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                }),
            ),
            Self::Gemini | Self::Typesafe => {
                let mut qs = Map::new();
                for q in questions {
                    let criteria: Map<String, Value> = q
                        .options
                        .iter()
                        .map(|(v, d)| (v.clone(), json!(d)))
                        .collect();
                    qs.insert(
                        q.name.clone(),
                        json!({"type": "choice", "instructions": q.instructions, "criteria": criteria}),
                    );
                }
                if self == Self::Gemini {
                    (
                        format!("{base}/decisions"),
                        json!({"model": "gemini-decision-1", "input": input, "questions": qs}),
                    )
                } else {
                    (
                        format!("{base}/systemone"),
                        json!({"model": "jev-1.14.0", "state": input, "questions": qs}),
                    )
                }
            }
        }
    }

    fn parse(self, body: &Value) -> Result<HashMap<String, Answer>> {
        let answer = |a: &Value| -> Result<Answer> {
            let choice = match &a["choice"] {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                other => bail!("answer has no choice: {other}"),
            };
            // OpenAI lists probabilities; Gemini and Jev map them.
            let probability = match &a["probabilities"] {
                Value::Array(items) => items
                    .iter()
                    .find(|p| p["value"].as_str() == Some(choice.as_str()))
                    .and_then(|p| p["probability"].as_f64()),
                Value::Object(map) => map.get(&choice).and_then(Value::as_f64),
                _ => None,
            }
            .or_else(|| a["confidence"].as_f64())
            .unwrap_or(0.0);
            Ok(Answer {
                choice,
                probability,
            })
        };
        let mut out = HashMap::new();
        match &body["answers"] {
            Value::Array(items) => {
                for a in items {
                    if a["type"] == "refusal" {
                        continue;
                    }
                    let name = a["name"].as_str().context("answer without a name")?;
                    out.insert(name.to_string(), answer(a)?);
                }
            }
            Value::Object(map) => {
                for (name, a) in map {
                    out.insert(name.clone(), answer(a)?);
                }
            }
            other => bail!("no answers in the response: {other}"),
        }
        Ok(out)
    }
}

/// The configured service: `HARNESS_DECISION` (`openai`, `gemini`,
/// `typesafe` or `off`), else the first with a key: Jev, OpenAI, Gemini.
pub struct SystemDecider {
    creds: Arc<Credentials>,
    http: reqwest::Client,
}

impl SystemDecider {
    pub fn new(creds: Arc<Credentials>) -> Self {
        Self {
            creds,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    fn pick(&self, prefer: Option<&str>) -> Result<Option<(Provider, String)>> {
        let wanted = env::var("HARNESS_DECISION").unwrap_or_default();
        // `off` in the environment wins; otherwise the thread's pick does.
        let wanted = match (wanted.trim(), prefer) {
            ("off", _) => "off",
            (_, Some(p)) => p,
            (w, None) => w,
        };
        let candidates: Vec<Provider> = match wanted {
            "" | "auto" => Provider::ALL.to_vec(),
            "off" => return Ok(None),
            id => vec![Provider::from_id(id).ok_or_else(|| {
                anyhow!("HARNESS_DECISION={id}: use openai, gemini, typesafe or off")
            })?],
        };
        Ok(candidates.into_iter().find_map(|p| {
            self.creds
                .named_key(p.key_name(), p.key_env())
                .map(|(k, _)| (p, k))
        }))
    }
}

impl Decide for SystemDecider {
    fn decide<'a>(
        &'a self,
        input: &'a str,
        questions: &'a [Question],
        prefer: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<Decision>>> {
        Box::pin(async move {
            let Some((provider, key)) = self.pick(prefer)? else {
                return Ok(None);
            };
            let answers = call(&self.http, provider, &key, input, questions).await?;
            Ok(Some(Decision {
                by: provider.label().to_string(),
                answers,
            }))
        })
    }

    fn available(&self) -> Vec<Provider> {
        Provider::ALL
            .into_iter()
            .filter(|p| self.creds.named_key(p.key_name(), p.key_env()).is_some())
            .collect()
    }
}

/// One request to `provider`; also used by `harness auth set` to check a key.
pub async fn call(
    http: &reqwest::Client,
    provider: Provider,
    key: &str,
    input: &str,
    questions: &[Question],
) -> Result<HashMap<String, Answer>> {
    let (url, body) = provider.request(input, questions);
    let req = http.post(&url).json(&body);
    let req = match provider {
        Provider::Gemini => req.header("x-goog-api-key", key),
        _ => req.bearer_auth(key),
    };
    let resp = req
        .send()
        .await
        .with_context(|| format!("{} request", provider.label()))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let snippet: String = text.chars().take(300).collect();
        bail!("{} returned {status}: {snippet}", provider.label());
    }
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} returned non-JSON", provider.label()))?;
    provider.parse(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q() -> Vec<Question> {
        vec![Question {
            name: "step".into(),
            instructions: "What is it?".into(),
            options: vec![
                ("log".into(), "a log".into()),
                ("known".into(), "skip".into()),
            ],
        }]
    }

    #[test]
    fn builds_each_wire_format() {
        let (url, body) = Provider::Openai.request("hi", &q());
        assert!(url.ends_with("/decisions"));
        assert_eq!(body["model"], "gpt-6-luna");
        assert_eq!(body["questions"][0]["choices"][1]["value"], "known");

        let (url, body) = Provider::Gemini.request("hi", &q());
        assert!(url.ends_with("/v1beta/decisions"));
        assert_eq!(body["input"], "hi");
        assert_eq!(body["questions"]["step"]["criteria"]["known"], "skip");

        let (url, body) = Provider::Typesafe.request("hi", &q());
        assert!(url.ends_with("/systemone"));
        assert_eq!(body["state"], "hi");
        assert_eq!(body["questions"]["step"]["type"], "choice");
    }

    #[test]
    fn parses_each_answer_format() {
        let openai = json!({"answers": [{"type": "choice", "name": "step", "choice": "known",
            "probabilities": [{"value": "log", "probability": 0.1}, {"value": "known", "probability": 0.9}],
            "confidence": 0.8}]});
        let a = &Provider::Openai.parse(&openai).unwrap()["step"];
        assert_eq!((a.choice.as_str(), a.probability), ("known", 0.9));

        let mapped = json!({"answers": {"step": {"type": "choice", "choice": "log",
            "probabilities": {"log": 0.7, "known": 0.3}, "confidence": 0.6}}});
        for p in [Provider::Gemini, Provider::Typesafe] {
            let a = &p.parse(&mapped).unwrap()["step"];
            assert_eq!((a.choice.as_str(), a.probability), ("log", 0.7));
        }
    }
}
