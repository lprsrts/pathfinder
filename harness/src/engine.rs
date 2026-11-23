use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

use crate::config::Config;
use crate::critic::{Violation, format_violations};
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineTier {
    System1,
    System2,
}

impl EngineTier {
    pub fn label(self) -> &'static str {
        match self {
            Self::System1 => "system1",
            Self::System2 => "system2",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAccess {
    /// No tools: used to reformat an answer without re-doing the work.
    None,
    ReadOnly,
    Full,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Thought(String),
    TextDelta(String),
}

pub type StreamSink = Arc<dyn Fn(StreamEvent) + Send + Sync>;

#[cfg(test)]
pub fn noop_sink() -> StreamSink {
    Arc::new(|_| {})
}

#[derive(Debug, Clone)]
pub struct EngineRequest {
    pub tier: EngineTier,
    pub tools: ToolAccess,
    pub cwd: PathBuf,
    pub system_prompt: String,
    pub prompt: String,
    pub files: Vec<PathBuf>,
    pub stream_text: bool,
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Engine: Send + Sync {
    fn run_text<'a>(
        &'a self,
        req: &'a EngineRequest,
        sink: StreamSink,
    ) -> BoxFuture<'a, Result<String>>;
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
    fn run_text<'a>(
        &'a self,
        req: &'a EngineRequest,
        sink: StreamSink,
    ) -> BoxFuture<'a, Result<String>> {
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
                ToolAccess::None => {
                    cmd.arg("--no-tools");
                }
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

            let mut child = cmd
                .spawn()
                .with_context(|| format!("spawn `{}`", self.config.pi_bin))?;

            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow!("pi stdout pipe unavailable"))?;
            let mut stderr = child
                .stderr
                .take()
                .ok_or_else(|| anyhow!("pi stderr pipe unavailable"))?;

            let stderr_task = tokio::spawn(async move {
                let mut buf = String::new();
                let _ = stderr.read_to_string(&mut buf).await;
                buf
            });

            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let mut final_text: Option<String> = None;
            let mut delta_buf = String::new();
            let mut last_error: Option<String> = None;

            loop {
                line.clear();
                let n = reader.read_line(&mut line).await?;
                if n == 0 {
                    break;
                }
                let trimmed = line.trim_end_matches(['\r', '\n']).trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
                    continue;
                };
                process_pi_event(
                    &val,
                    req.stream_text,
                    &sink,
                    &mut final_text,
                    &mut delta_buf,
                    &mut last_error,
                );
            }

            let status = child.wait().await?;
            let stderr_text = stderr_task.await.unwrap_or_default();

            if !status.success() {
                bail!("pi exited with {status}: {}", stderr_text.trim());
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
            Err(anyhow!(
                "no assistant text found in pi json stream (stderr: {})",
                stderr_text.trim()
            ))
        })
    }
}

fn process_pi_event(
    val: &Value,
    stream_text: bool,
    sink: &StreamSink,
    final_text: &mut Option<String>,
    delta_buf: &mut String,
    last_error: &mut Option<String>,
) {
    match val.get("type").and_then(Value::as_str) {
        Some("message_end") => {
            if let Some(msg) = val.get("message") {
                if msg.get("role").and_then(Value::as_str) == Some("assistant") {
                    let text = extract_content_text(msg.get("content"));
                    if !text.is_empty() {
                        *final_text = Some(text);
                    }
                    if let Some(err) = msg.get("errorMessage").and_then(Value::as_str) {
                        *last_error = Some(err.to_string());
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
                            if stream_text {
                                sink(StreamEvent::TextDelta(d.to_string()));
                            }
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(d) = ev.get("delta").and_then(Value::as_str) {
                            sink(StreamEvent::Thought(d.to_string()));
                        }
                    }
                    Some("error") => {
                        if let Some(err) = ev.get("error").and_then(Value::as_str) {
                            *last_error = Some(err.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        Some("tool_execution_start") => {
            if let Some(name) = val.get("toolName").and_then(Value::as_str) {
                sink(StreamEvent::Thought(format!("Using `{name}`...\n")));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub fn parse_pi_jsonl(stdout: &str) -> Result<String> {
    let sink = noop_sink();
    let mut final_text = None;
    let mut delta_buf = String::new();
    let mut last_error = None;

    for raw_line in stdout.split('\n') {
        let line = raw_line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        process_pi_event(
            &val,
            false,
            &sink,
            &mut final_text,
            &mut delta_buf,
            &mut last_error,
        );
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

/// Runs a structured proposal through deterministic Rust critics.
/// When starting on System 1, any parse error or critic rejection automatically
/// escalates to System 2 with exact machine-readable violation feedback.
pub async fn run_gated<T, C>(
    engine: &dyn Engine,
    store: &Store,
    session_id: &str,
    kind: &str,
    base_req: &EngineRequest,
    sink: StreamSink,
    critic: C,
) -> Result<T>
where
    T: DeserializeOwned,
    C: Fn(&T) -> Vec<Violation>,
{
    let mut req = base_req.clone();
    req.stream_text = false;

    let attempts: Vec<EngineTier> = match base_req.tier {
        EngineTier::System1 => vec![
            EngineTier::System1,
            EngineTier::System2,
            EngineTier::System2,
        ],
        EngineTier::System2 => vec![EngineTier::System2, EngineTier::System2],
    };

    let mut last_violations: Vec<Violation> = Vec::new();

    for (idx, tier) in attempts.iter().copied().enumerate() {
        req.tier = tier;
        let is_escalation =
            idx > 0 && base_req.tier == EngineTier::System1 && tier == EngineTier::System2;

        if idx == 0 {
            sink(StreamEvent::Thought(format!(
                "Drafting {kind} ({})...\n",
                tier.label()
            )));
        } else if is_escalation && idx == 1 {
            let codes = last_violations
                .iter()
                .map(|v| v.code)
                .collect::<Vec<_>>()
                .join(", ");
            sink(StreamEvent::Thought(format!(
                "Critic rejected System 1 ({codes}) — escalating to System 2...\n"
            )));
        } else {
            let codes = last_violations
                .iter()
                .map(|v| v.code)
                .collect::<Vec<_>>()
                .join(", ");
            sink(StreamEvent::Thought(format!(
                "Critic requested revision ({codes})...\n"
            )));
        }

        let raw = engine.run_text(&req, sink.clone()).await?;
        let mut outcome = evaluate_proposal::<T, _>(&raw, &critic);

        // Weak models often answer well but drop the JSON envelope after a
        // tool call. Ask the same tier to re-wrap its own answer (no tools)
        // before treating it as a quality failure and escalating.
        if is_only_invalid_json(&outcome) && !raw.trim().is_empty() {
            let _ = store.record_critic_event(
                session_id,
                kind,
                tier.label(),
                false,
                false,
                outcome.as_ref().err().map(Vec::as_slice).unwrap_or(&[]),
            );
            sink(StreamEvent::Thought(
                "Answer was not JSON — reformatting...\n".to_string(),
            ));
            let mut repair = req.clone();
            repair.tools = ToolAccess::None;
            repair.files.clear();
            repair.prompt = format!(
                "{}\n\nYour previous answer was not valid JSON:\n<<<\n{}\n>>>\n\nKeep its content (do not redo the work and do not call tools) and return it as ONLY one valid JSON object matching the schema.",
                base_req.prompt,
                raw.trim()
            );
            let repaired = engine.run_text(&repair, sink.clone()).await?;
            outcome = evaluate_proposal::<T, _>(&repaired, &critic);
        }

        match outcome {
            Ok(val) => {
                let _ = store.record_critic_event(
                    session_id,
                    kind,
                    tier.label(),
                    true,
                    is_escalation,
                    &[],
                );
                return Ok(val);
            }
            Err(violations) => {
                let will_escalate = tier == EngineTier::System1
                    && attempts.get(idx + 1) == Some(&EngineTier::System2);
                let _ = store.record_critic_event(
                    session_id,
                    kind,
                    tier.label(),
                    false,
                    will_escalate,
                    &violations,
                );
                last_violations = violations;
                let report = format_violations(&last_violations);
                req.prompt = format!(
                    "{}\n\nYour previous proposal was rejected by the deterministic critic for:\n{report}\n\nFix every violation above and return ONLY one valid JSON object matching the schema.",
                    base_req.prompt
                );
            }
        }
    }

    bail!(
        "{kind} proposal rejected by critic after retries:\n{}",
        format_violations(&last_violations)
    )
}

fn is_only_invalid_json<T>(outcome: &std::result::Result<T, Vec<Violation>>) -> bool {
    matches!(outcome, Err(v) if v.len() == 1 && v[0].code == "invalid_json")
}

fn evaluate_proposal<T, C>(raw: &str, critic: &C) -> std::result::Result<T, Vec<Violation>>
where
    T: DeserializeOwned,
    C: Fn(&T) -> Vec<Violation>,
{
    let candidate = extract_json_object(raw);
    let parsed: T = serde_json::from_str(candidate).map_err(|err| {
        vec![Violation::new(
            "invalid_json",
            format!("could not parse JSON: {err}"),
        )]
    })?;
    let violations = critic(&parsed);
    if violations.is_empty() {
        Ok(parsed)
    } else {
        Err(violations)
    }
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
