//! Decision models: a fast first pass that answers typed questions about a
//! chat message with calibrated probabilities, before any LLM runs. Three
//! hosted services share the idea (shared input, typed questions, no output
//! text), each with its own wire format:
//!
//! - OpenAI Decisions API (`POST /v1/decisions`, `gpt-6-luna`)
//! - Gemini Decision 1 (`POST /v1beta/decisions`, `gemini-decision-1`)
//! - TypeSafe Jev (`POST /v1/systemone`, `jev-1.14.0`)
//!
//! A fourth, `local`, uses a model on one of your endpoints (`decisions` in
//! the config): each question becomes a one-letter answer whose token log
//! probabilities give the distribution. It needs a server that returns log
//! probabilities (vLLM, llama.cpp, many hosted services) and a model that
//! doesn't think before answering.
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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Answer {
    pub choice: String,
    /// Probability of the chosen option.
    pub probability: f64,
    /// Every option's probability, where the service reports them.
    pub all: HashMap<String, f64>,
}

impl Answer {
    /// The probability of `option`: reported, or else whatever the chosen
    /// option leaves, split evenly over the other `n_options - 1`.
    pub fn p(&self, option: &str, n_options: usize) -> f64 {
        if let Some(p) = self.all.get(option) {
            *p
        } else if self.choice == option {
            self.probability
        } else {
            (1.0 - self.probability) / n_options.saturating_sub(1).max(1) as f64
        }
    }
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
    /// A model on one of your endpoints, through log probabilities.
    Local,
}

impl Provider {
    /// Auto order: your own endpoint first, then fastest and cheapest.
    pub const ALL: [Provider; 4] = [
        Provider::Local,
        Provider::Typesafe,
        Provider::Openai,
        Provider::Gemini,
    ];

    /// Used in `harness auth set <id>` and `HARNESS_DECISION`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Gemini => "gemini",
            Self::Typesafe => "typesafe",
            Self::Local => "local",
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
            Self::Local => "Local decisions",
        }
    }

    /// Name on the toolbar button.
    pub fn short(self) -> &'static str {
        match self {
            Self::Openai => "OpenAI",
            Self::Gemini => "Gemini",
            Self::Typesafe => "Jev",
            Self::Local => "Local",
        }
    }

    /// The model it runs, shown on hover.
    pub fn model(self) -> &'static str {
        match self {
            Self::Openai => "gpt-6-luna",
            Self::Gemini => "gemini-decision-1",
            Self::Typesafe => "jev-1.14.0",
            Self::Local => "your endpoint's model",
        }
    }

    /// Auth file entry, also the keychain item `auth import` reads.
    pub fn key_name(self) -> &'static str {
        match self {
            Self::Openai => "openai-api-key",
            Self::Gemini => "gemini-api-key",
            Self::Typesafe => "typesafe-api-key",
            Self::Local => "",
        }
    }

    pub fn key_env(self) -> &'static str {
        match self {
            Self::Openai => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Typesafe => "TYPESAFE_API_KEY",
            Self::Local => "",
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
            Self::Local => ("", ""),
        };
        env::var(var)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| default.to_string())
    }

    fn request(self, input: &str, questions: &[Question]) -> (String, Value) {
        self.request_at(&self.base_url(), input, questions)
    }

    fn request_at(self, base: &str, input: &str, questions: &[Question]) -> (String, Value) {
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
            Self::Local => (String::new(), Value::Null),
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
            let all = match &a["probabilities"] {
                Value::Array(items) => items
                    .iter()
                    .filter_map(|p| {
                        Some((p["value"].as_str()?.to_string(), p["probability"].as_f64()?))
                    })
                    .collect(),
                Value::Object(map) => map
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_f64()?)))
                    .collect(),
                _ => HashMap::new(),
            };
            Ok(Answer {
                choice,
                probability,
                all,
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
                anyhow!("HARNESS_DECISION={id}: use local, openai, gemini, typesafe or off")
            })?],
        };
        let cfg = crate::config::Config::load();
        Ok(candidates.into_iter().find_map(|p| {
            if p == Provider::Local {
                return cfg.decisions.is_some().then(|| (p, String::new()));
            }
            // On-prem: no hosted decision service, ever.
            if cfg.local_only {
                return None;
            }
            self.creds
                .named_key(p.key_name(), p.key_env())
                .map(|(k, _)| (p, k))
                .or_else(|| {
                    (p == Provider::Typesafe)
                        .then(|| proxy_token())
                        .flatten()
                        .map(|t| (p, t))
                })
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
            let own_key = self
                .creds
                .named_key(provider.key_name(), provider.key_env())
                .is_some();
            let answers = if provider == Provider::Local {
                local::decide(&self.http, &self.creds, input, questions).await?
            } else if provider == Provider::Typesafe && !own_key {
                // No key of your own: the shared gate service, through
                // Pathfinder's collector, with your install token.
                let base = format!("{}/v1/typesafe", crate::telemetry::collector_url());
                call_at(&self.http, provider, &base, &key, input, questions).await?
            } else {
                call(&self.http, provider, &key, input, questions).await?
            };
            Ok(Some(Decision {
                by: provider.label().to_string(),
                answers,
            }))
        })
    }

    fn available(&self) -> Vec<Provider> {
        let cfg = crate::config::Config::load();
        Provider::ALL
            .into_iter()
            .filter(|p| match p {
                Provider::Local => cfg.decisions.is_some(),
                _ if cfg.local_only => false,
                Provider::Typesafe => {
                    self.creds.named_key(p.key_name(), p.key_env()).is_some()
                        || proxy_token().is_some()
                }
                _ => self.creds.named_key(p.key_name(), p.key_env()).is_some(),
            })
            .collect()
    }
}

/// Decisions from a model on your endpoint, through log probabilities.
pub mod local {
    use std::collections::HashMap;

    use anyhow::{Context, Result, anyhow, bail};
    use futures_util::future::try_join_all;
    use serde_json::{Value, json};

    use super::{Answer, Question};
    use crate::auth::Credentials;

    /// Answers every question at once, one request each.
    pub async fn decide(
        http: &reqwest::Client,
        creds: &Credentials,
        input: &str,
        questions: &[Question],
    ) -> Result<HashMap<String, Answer>> {
        let cfg = crate::config::Config::load();
        let d = cfg
            .decisions
            .clone()
            .context("no `decisions` in the config")?;
        let ep = cfg.endpoint(&d.endpoint).ok_or_else(|| {
            anyhow!(
                "`decisions` names endpoint `{}`, which isn't configured",
                d.endpoint
            )
        })?;
        let key = ep.key.as_deref().and_then(|k| creds.stored(k));
        let answers = try_join_all(questions.iter().map(|q| {
            ask(
                http,
                &ep.base_url,
                key.as_deref(),
                ep.extra.as_ref(),
                &d.model,
                input,
                q,
            )
        }))
        .await?;
        Ok(questions
            .iter()
            .map(|q| q.name.clone())
            .zip(answers)
            .collect())
    }

    fn letter(i: usize) -> char {
        (b'A' + i as u8) as char
    }

    /// The prompt for one question: options as letters, one letter back.
    pub fn prompt(input: &str, q: &Question) -> String {
        let options: Vec<String> = q
            .options
            .iter()
            .enumerate()
            .map(|(i, (v, d))| format!("{}) {v}: {d}", letter(i)))
            .collect();
        format!(
            "{input}\n\n---\nQuestion: {}\n\nOptions:\n{}\n\nAnswer with the letter of the best option only.",
            q.instructions,
            options.join("\n")
        )
    }

    /// The distribution over options from the first token's log
    /// probabilities: tokens that are an option's letter (spacing and
    /// case aside) add up, then it is normalised over the options.
    pub fn distribution(body: &Value, n: usize) -> Option<Vec<f64>> {
        let top = body
            .pointer("/choices/0/logprobs/content/0/top_logprobs")?
            .as_array()?;
        let mut p = vec![0.0; n];
        for t in top {
            let token = t["token"].as_str().unwrap_or_default();
            let token = token
                .trim()
                .trim_start_matches('(')
                .trim_end_matches([')', '.', ':']);
            let mut chars = token.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                continue;
            };
            let i = (c.to_ascii_uppercase() as u8).wrapping_sub(b'A') as usize;
            if i < n {
                p[i] += t["logprob"].as_f64().unwrap_or(f64::NEG_INFINITY).exp();
            }
        }
        let total: f64 = p.iter().sum();
        (total > 0.0).then(|| p.into_iter().map(|x| x / total).collect())
    }

    pub(crate) async fn ask(
        http: &reqwest::Client,
        base_url: &str,
        key: Option<&str>,
        extra: Option<&Value>,
        model: &str,
        input: &str,
        q: &Question,
    ) -> Result<Answer> {
        if q.options.is_empty() || q.options.len() > 26 {
            bail!("question `{}` needs 1 to 26 options", q.name);
        }
        let mut body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": "You classify carefully. Reply with exactly one letter."},
                {"role": "user", "content": prompt(input, q)},
            ],
            "max_tokens": 1,
            "temperature": 0,
            "logprobs": true,
            "top_logprobs": 20,
        });
        crate::config::merge_extra(&mut body, extra);
        let mut req = http
            .post(format!(
                "{}/chat/completions",
                base_url.trim_end_matches('/')
            ))
            .timeout(std::time::Duration::from_secs(60))
            .json(&body);
        if let Some(key) = key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.context("local decision request")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let snippet: String = text.chars().take(300).collect();
            bail!("the decision endpoint returned {status}: {snippet}");
        }
        let value: Value =
            serde_json::from_str(&text).context("decision endpoint returned non-JSON")?;
        let dist = distribution(&value, q.options.len()).ok_or_else(|| {
            anyhow!(
                "the decision endpoint returned no usable log probabilities for `{model}`: use a server that supports `logprobs` and a model that answers without thinking first"
            )
        })?;
        let (best, p) = dist
            .iter()
            .copied()
            .enumerate()
            .fold((0, f64::MIN), |a, b| if b.1 > a.1 { b } else { a });
        Ok(Answer {
            choice: q.options[best].0.clone(),
            probability: p,
            all: q.options.iter().map(|(v, _)| v.clone()).zip(dist).collect(),
        })
    }
}

/// The install token of an enrolled install, which the collector accepts
/// in place of a TypeSafe key. `HARNESS_DECISION_PROXY=off` ignores it.
fn proxy_token() -> Option<String> {
    if env::var("HARNESS_DECISION_PROXY").is_ok_and(|v| v.trim() == "off") {
        return None;
    }
    let consent = crate::telemetry::Consent::load();
    consent.enrolled.then(|| consent.token()).flatten()
}

/// One request to `provider`; also used by `harness auth set` to check a key.
pub async fn call(
    http: &reqwest::Client,
    provider: Provider,
    key: &str,
    input: &str,
    questions: &[Question],
) -> Result<HashMap<String, Answer>> {
    call_at(http, provider, &provider.base_url(), key, input, questions).await
}

/// `call`, at another base URL (the collector's gate proxy).
pub async fn call_at(
    http: &reqwest::Client,
    provider: Provider,
    base: &str,
    key: &str,
    input: &str,
    questions: &[Question],
) -> Result<HashMap<String, Answer>> {
    let (url, body) = provider.request_at(base, input, questions);
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

    #[test]
    fn local_distribution_sums_letter_tokens() {
        let body = json!({"choices": [{"logprobs": {"content": [{"top_logprobs": [
            {"token": "B", "logprob": (0.6f64).ln()},
            {"token": " b", "logprob": (0.1f64).ln()},
            {"token": "(A", "logprob": (0.2f64).ln()},
            {"token": "Yes", "logprob": (0.1f64).ln()},
        ]}]}}]});
        let d = local::distribution(&body, 2).unwrap();
        assert!((d[0] - 0.2 / 0.9).abs() < 1e-9);
        assert!((d[1] - 0.7 / 0.9).abs() < 1e-9);
        assert!(local::distribution(&json!({"choices": [{}]}), 2).is_none());
        let p = local::prompt("The draft.", &q()[0]);
        assert!(p.contains("A) log:") && p.contains("B) known:"), "{p}");
    }

    #[tokio::test]
    async fn local_decisions_come_from_log_probabilities() {
        let reply = json!({"choices": [{"message": {"content": "B"}, "logprobs": {"content": [
            {"token": "B", "logprob": -0.1, "top_logprobs": [
                {"token": "B", "logprob": (0.8f64).ln()},
                {"token": "A", "logprob": (0.2f64).ln()},
            ]}
        ]}}]})
        .to_string();
        let (url, captured) = crate::llm::testing::serve_once("200 OK", &reply).await;
        let a = local::ask(
            &reqwest::Client::new(),
            &url,
            None,
            None,
            "small",
            "The draft.",
            &q()[0],
        )
        .await
        .unwrap();
        assert_eq!(a.choice, "known");
        assert!((a.probability - 0.8).abs() < 1e-9);
        assert!((a.p("log", 2) - 0.2).abs() < 1e-9);
        let c = captured.lock().unwrap();
        let body = &c.as_ref().unwrap().body;
        assert_eq!(body["max_tokens"], 1);
        assert_eq!(body["logprobs"], true);

        // No log probabilities: an error, so the gate fails closed.
        let plain = json!({"choices": [{"message": {"content": "B"}}]}).to_string();
        let (url, _) = crate::llm::testing::serve_once("200 OK", &plain).await;
        let err = local::ask(
            &reqwest::Client::new(),
            &url,
            None,
            None,
            "small",
            "x",
            &q()[0],
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("log probabilities"));
    }

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
        assert_eq!(a.p("log", 2), 0.1);

        let mapped = json!({"answers": {"step": {"type": "choice", "choice": "log",
            "probabilities": {"log": 0.7, "known": 0.3}, "confidence": 0.6}}});
        for p in [Provider::Gemini, Provider::Typesafe] {
            let a = &p.parse(&mapped).unwrap()["step"];
            assert_eq!((a.choice.as_str(), a.probability), ("log", 0.7));
            assert_eq!(a.p("known", 2), 0.3);
        }
        let bare = Answer {
            choice: "a".into(),
            probability: 0.6,
            ..Default::default()
        };
        assert!((bare.p("b", 3) - 0.2).abs() < 1e-9);
    }
}
