use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::process::Command;

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineTier {
    System1,
    System2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAccess {
    ReadOnly,
    Full,
}

#[derive(Debug, Clone)]
pub struct EngineRequest {
    pub tier: EngineTier,
    pub tools: ToolAccess,
    pub cwd: PathBuf,
    pub system_prompt: String,
    pub prompt: String,
    pub files: Vec<PathBuf>,
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Engine: Send + Sync {
    fn run_text<'a>(&'a self, req: &'a EngineRequest) -> BoxFuture<'a, Result<String>>;
}

pub struct PiEngine {
    config: Config,
}

impl PiEngine {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Engine for PiEngine {
    fn run_text<'a>(&'a self, req: &'a EngineRequest) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let mut cmd = Command::new(&self.config.pi_bin);
            cmd.current_dir(&req.cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .args([
                    "--mode",
                    "json",
                    "--print",
                    "--no-extensions",
                    "--no-mcp",
                    "--no-skills",
                    "--no-context-files",
                    "--no-session",
                ]);

            let (model, thinking) = match req.tier {
                EngineTier::System1 => (
                    self.config.system1_model.as_deref(),
                    self.config.system1_thinking.as_str(),
                ),
                EngineTier::System2 => (
                    self.config.system2_model.as_deref(),
                    self.config.system2_thinking.as_str(),
                ),
            };

            if let Some(m) = model {
                cmd.args(["--model", m]);
            }
            if !thinking.is_empty() {
                cmd.args(["--thinking", thinking]);
            }

            match req.tools {
                ToolAccess::ReadOnly => {
                    cmd.args(["--tools", "read"]);
                }
                ToolAccess::Full => {}
            }

            cmd.args(["--system-prompt", &req.system_prompt]);
            cmd.arg("--");

            for file in &req.files {
                if file.exists() {
                    cmd.arg(format!("@{}", file.display()));
                }
            }
            cmd.arg(&req.prompt);

            let output = cmd
                .output()
                .await
                .with_context(|| format!("spawn `{}`", self.config.pi_bin))?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !output.status.success() {
                bail!(
                    "pi exited with {}: {}",
                    output.status,
                    stderr.trim()
                );
            }

            parse_pi_jsonl(&stdout).with_context(|| {
                format!(
                    "failed to extract assistant reply from pi output (stderr: {})",
                    stderr.trim()
                )
            })
        })
    }
}

pub fn parse_pi_jsonl(stdout: &str) -> Result<String> {
    let mut final_text: Option<String> = None;
    let mut delta_buf = String::new();
    let mut last_error: Option<String> = None;

    for raw_line in stdout.split('\n') {
        let line = raw_line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match val.get("type").and_then(Value::as_str) {
            Some("message_end") => {
                if let Some(msg) = val.get("message") {
                    if msg.get("role").and_then(Value::as_str) == Some("assistant") {
                        let text = extract_content_text(msg.get("content"));
                        if !text.is_empty() {
                            final_text = Some(text);
                        }
                        if let Some(err) = msg.get("errorMessage").and_then(Value::as_str) {
                            last_error = Some(err.to_string());
                        }
                    }
                }
            }
            Some("message_update") => {
                if let Some(ev) = val.get("assistantMessageEvent") {
                    match ev.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            if let Some(d) = ev.get("delta").and_then(Value::as_str) {
                                delta_buf.push_str(d);
                            }
                        }
                        Some("error") => {
                            if let Some(err) = ev.get("error").and_then(Value::as_str) {
                                last_error = Some(err.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(text) = final_text {
        return Ok(text);
    }
    if !delta_buf.trim().is_empty() {
        return Ok(delta_buf);
    }
    if let Some(err) = last_error {
        bail!("model error: {err}");
    }
    Err(anyhow!("no assistant text found in pi json stream"))
}

fn extract_content_text(content: Option<&Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    let Some(arr) = content.as_array() else {
        return String::new();
    };
    let mut out = String::new();
    for block in arr {
        if block.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(t) = block.get("text").and_then(Value::as_str) {
                out.push_str(t);
            }
        }
    }
    out
}

pub fn extract_json_object(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim();
        }
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim();
        }
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start <= end {
            return &trimmed[start..=end];
        }
    }
    trimmed
}

pub async fn run_structured<T, V>(
    engine: &dyn Engine,
    base_req: &EngineRequest,
    validate: V,
) -> Result<T>
where
    T: DeserializeOwned,
    V: Fn(&T) -> Result<()>,
{
    let first = engine.run_text(base_req).await?;
    match parse_and_validate::<T, _>(&first, &validate) {
        Ok(val) => Ok(val),
        Err(first_err) => {
            let mut retry_req = base_req.clone();
            retry_req.prompt = format!(
                "{}\n\nYour previous reply did not match the required JSON shape ({first_err}). Return ONLY a single valid JSON object matching the schema with no extra text.",
                base_req.prompt
            );
            let second = engine.run_text(&retry_req).await?;
            parse_and_validate::<T, _>(&second, &validate).with_context(|| {
                format!("structured output failed after retry (first error: {first_err})")
            })
        }
    }
}

fn parse_and_validate<T, V>(raw: &str, validate: &V) -> Result<T>
where
    T: DeserializeOwned,
    V: Fn(&T) -> Result<()>,
{
    let candidate = extract_json_object(raw);
    let parsed: T = serde_json::from_str(candidate)
        .with_context(|| format!("invalid JSON: {candidate}"))?;
    validate(&parsed)?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_assistant_text_from_pi_jsonl() {
        let stream = r#"{"type":"session","version":3,"id":"1"}
{"type":"message_start","message":{"role":"assistant","content":[]}}
{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"hi"}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"{\"reply\":\"ok\"}"}]}}
{"type":"agent_settled","aborted":false}"#;
        assert_eq!(parse_pi_jsonl(stream).unwrap(), r#"{"reply":"ok"}"#);
    }

    #[test]
    fn extracts_json_from_fenced_block() {
        let raw = "Sure:\n```json\n{\"a\": 1}\n```";
        assert_eq!(extract_json_object(raw), "{\"a\": 1}");
    }
}
