use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::config::{Backend, Config, ModelEntry};
use crate::store::Store;
use crate::validate::{Violation, format_violations};

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

/// The models a request may use. Escalation moves from System 1 to System 2.
#[derive(Debug, Clone)]
pub struct TierModels {
    pub system1: ModelEntry,
    pub system2: ModelEntry,
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
    pub models: TierModels,
}

impl EngineRequest {
    pub fn model(&self) -> &ModelEntry {
        match self.tier {
            EngineTier::System1 => &self.models.system1,
            EngineTier::System2 => &self.models.system2,
        }
    }

    /// Prompt plus attached file paths, for CLIs without `@file` arguments.
    fn prompt_with_files(&self) -> String {
        let files: Vec<String> = self
            .files
            .iter()
            .filter(|f| f.exists())
            .map(|f| format!("- {}", f.display()))
            .collect();
        if files.is_empty() {
            self.prompt.clone()
        } else {
            format!(
                "{}\n\nAttached files (read them as needed):\n{}",
                self.prompt,
                files.join("\n")
            )
        }
    }
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Engine: Send + Sync {
    fn run_text<'a>(
        &'a self,
        req: &'a EngineRequest,
        sink: StreamSink,
    ) -> BoxFuture<'a, Result<String>>;
}

/// Runs models through pi, Claude Code or Codex, per the request's model.
pub struct CliEngine {
    config: Config,
}

impl CliEngine {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Engine for CliEngine {
    fn run_text<'a>(
        &'a self,
        req: &'a EngineRequest,
        sink: StreamSink,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let model = req.model();
            sink(StreamEvent::Thought(format!(
                "{} via {}{}...\n",
                req.tier.label(),
                model.backend.label(),
                if model.model.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", model.model)
                }
            )));
            match model.backend {
                Backend::Pi => self.run_pi(req, sink).await,
                Backend::Claude => self.run_claude(req, sink).await,
                Backend::Codex => self.run_codex(req, sink).await,
            }
        })
    }
}

/// Spawns `cmd`, optionally writes `stdin_text`, and hands each JSON line of
/// stdout to `on_event`. Returns the exit status and stderr.
async fn run_jsonl(
    mut cmd: Command,
    bin: &str,
    stdin_text: Option<String>,
    mut on_event: impl FnMut(&Value),
) -> Result<(std::process::ExitStatus, String)> {
    cmd.stdin(if stdin_text.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().with_context(|| format!("spawn `{bin}`"))?;

    if let Some(text) = stdin_text {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("{bin} stdin pipe unavailable"))?;
        stdin.write_all(text.as_bytes()).await?;
        drop(stdin);
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("{bin} stdout pipe unavailable"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("{bin} stderr pipe unavailable"))?;
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf).await;
        buf
    });

    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        if let Ok(val) = serde_json::from_str::<Value>(line.trim()) {
            on_event(&val);
        }
    }
    let status = child.wait().await?;
    let stderr_text = stderr_task.await.unwrap_or_default();
    Ok((status, stderr_text))
}

impl CliEngine {
    async fn run_pi(&self, req: &EngineRequest, sink: StreamSink) -> Result<String> {
        let model = req.model();
        let mut cmd = Command::new(&self.config.pi_bin);
        cmd.current_dir(&req.cwd).args([
            "--mode",
            "json",
            "--print",
            "--no-extensions",
            "--no-mcp",
            "--no-skills",
            "--no-context-files",
            "--no-session",
        ]);
        if !model.model.is_empty() {
            cmd.args(["--model", &model.model]);
        }
        if let Some(effort) = model.effort.as_deref().filter(|e| !e.is_empty()) {
            cmd.args(["--thinking", effort]);
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

        let mut final_text: Option<String> = None;
        let mut delta_buf = String::new();
        let mut last_error: Option<String> = None;
        let (status, stderr) = run_jsonl(cmd, &self.config.pi_bin, None, |val| {
            process_pi_event(
                val,
                req.stream_text,
                &sink,
                &mut final_text,
                &mut delta_buf,
                &mut last_error,
            )
        })
        .await?;

        if !status.success() {
            bail!("pi exited with {status}: {}", stderr.trim());
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
            stderr.trim()
        ))
    }

    async fn run_claude(&self, req: &EngineRequest, sink: StreamSink) -> Result<String> {
        let model = req.model();
        let mut cmd = Command::new(&self.config.claude_bin);
        cmd.current_dir(&req.cwd).args([
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--strict-mcp-config",
        ]);
        if !model.model.is_empty() {
            cmd.args(["--model", &model.model]);
        }
        if let Some(effort) = model.effort.as_deref().filter(|e| !e.is_empty()) {
            cmd.args(["--effort", effort]);
        }
        cmd.args(["--system-prompt", &req.system_prompt]);
        match req.tools {
            ToolAccess::None => {
                cmd.args(["--tools", ""]);
            }
            ToolAccess::ReadOnly => {
                cmd.args(["--tools", "Read,Glob,Grep"]);
            }
            ToolAccess::Full => {
                cmd.args([
                    "--tools",
                    "Read,Glob,Grep,Edit,Write,Bash",
                    "--permission-mode",
                    "acceptEdits",
                    "--allowedTools",
                    "Bash",
                    "--settings",
                    r#"{"sandbox":{"enabled":true,"autoAllowBashIfSandboxed":true}}"#,
                ]);
            }
        }

        let mut result: Option<(bool, String)> = None;
        let (status, stderr) = run_jsonl(
            cmd,
            &self.config.claude_bin,
            Some(req.prompt_with_files()),
            |val| process_claude_event(val, req.stream_text, &sink, &mut result),
        )
        .await?;

        match result {
            Some((false, text)) => Ok(text),
            Some((true, err)) => bail!("model error: {err}"),
            None => bail!(
                "claude exited with {status} without a result: {}",
                stderr.trim()
            ),
        }
    }

    async fn run_codex(&self, req: &EngineRequest, sink: StreamSink) -> Result<String> {
        let model = req.model();
        let mut cmd = Command::new(&self.config.codex_bin);
        cmd.arg("exec")
            .args([
                "--json",
                "--ignore-user-config",
                "--skip-git-repo-check",
                "--ephemeral",
            ])
            .arg("-C")
            .arg(&req.cwd)
            .args([
                "-s",
                match req.tools {
                    ToolAccess::Full => "workspace-write",
                    ToolAccess::ReadOnly | ToolAccess::None => "read-only",
                },
            ]);
        if !model.model.is_empty() {
            cmd.args(["-m", &model.model]);
        }
        if let Some(effort) = model.effort.as_deref().filter(|e| !e.is_empty()) {
            cmd.args(["-c", &format!("model_reasoning_effort=\"{effort}\"")]);
        }
        // Codex has no system-prompt flag; the instructions lead the prompt.
        let mut prompt = format!("<instructions>\n{}\n</instructions>\n\n", req.system_prompt);
        if req.tools == ToolAccess::None {
            prompt.push_str("Do not run any commands or read any files for this answer.\n\n");
        }
        prompt.push_str(&req.prompt_with_files());

        let mut last_message: Option<String> = None;
        let mut last_error: Option<String> = None;
        let (status, stderr) = run_jsonl(cmd, &self.config.codex_bin, Some(prompt), |val| {
            process_codex_event(
                val,
                req.stream_text,
                &sink,
                &mut last_message,
                &mut last_error,
            )
        })
        .await?;

        if let Some(text) = last_message {
            return Ok(text);
        }
        if let Some(err) = last_error {
            bail!("model error: {err}");
        }
        bail!(
            "codex exited with {status} without a message: {}",
            stderr.trim()
        )
    }
}

fn process_claude_event(
    val: &Value,
    stream_text: bool,
    sink: &StreamSink,
    result: &mut Option<(bool, String)>,
) {
    match val.get("type").and_then(Value::as_str) {
        Some("assistant") => {
            let content = val
                .pointer("/message/content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for item in content {
                match item.get("type").and_then(Value::as_str) {
                    Some("text") if stream_text => {
                        if let Some(t) = item.get("text").and_then(Value::as_str) {
                            sink(StreamEvent::TextDelta(format!("{t}\n\n")));
                        }
                    }
                    Some("thinking") => {
                        if let Some(t) = item.get("thinking").and_then(Value::as_str) {
                            if !t.is_empty() {
                                sink(StreamEvent::Thought(t.to_string()));
                            }
                        }
                    }
                    Some("tool_use") => {
                        if let Some(name) = item.get("name").and_then(Value::as_str) {
                            sink(StreamEvent::Thought(format!("Using `{name}`...\n")));
                        }
                    }
                    _ => {}
                }
            }
        }
        Some("result") => {
            let is_error = val
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let text = val
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            *result = Some((is_error, text));
        }
        _ => {}
    }
}

fn process_codex_event(
    val: &Value,
    stream_text: bool,
    sink: &StreamSink,
    last_message: &mut Option<String>,
    last_error: &mut Option<String>,
) {
    let item = val.get("item");
    let item_type = item
        .and_then(|i| i.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text = || {
        item.and_then(|i| i.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match (val.get("type").and_then(Value::as_str), item_type) {
        (Some("item.started"), "command_execution") => {
            let command = item
                .and_then(|i| i.get("command"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            sink(StreamEvent::Thought(format!("Running `{command}`...\n")));
        }
        (Some("item.completed"), "reasoning") => {
            let t = text();
            if !t.is_empty() {
                sink(StreamEvent::Thought(t));
            }
        }
        (Some("item.completed"), "agent_message") => {
            let t = text();
            if stream_text {
                sink(StreamEvent::TextDelta(format!("{t}\n\n")));
            }
            *last_message = Some(t);
        }
        (Some("error"), _) | (Some("turn.failed"), _) => {
            let msg = val
                .get("message")
                .or_else(|| val.pointer("/error/message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            *last_error = Some(msg.to_string());
        }
        _ => {}
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

/// Runs a structured proposal through its shape check (see validate.rs).
/// When starting on System 1, any parse error or failed check automatically
/// escalates to System 2 with exact machine-readable violation feedback.
pub async fn run_gated<T, C>(
    engine: &dyn Engine,
    store: &Store,
    session_id: &str,
    kind: &str,
    base_req: &EngineRequest,
    sink: StreamSink,
    check: C,
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
                "System 1 output failed the shape check ({codes}) — escalating to System 2...\n"
            )));
        } else {
            let codes = last_violations
                .iter()
                .map(|v| v.code)
                .collect::<Vec<_>>()
                .join(", ");
            sink(StreamEvent::Thought(format!(
                "Output failed the shape check ({codes}) — retrying...\n"
            )));
        }

        let raw = engine.run_text(&req, sink.clone()).await?;
        let mut outcome = evaluate_proposal::<T, _>(&raw, &check);

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
            outcome = evaluate_proposal::<T, _>(&repaired, &check);
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
                    "{}\n\nYour previous output failed the shape check:\n{report}\n\nFix every violation above and return ONLY one valid JSON object matching the schema.",
                    base_req.prompt
                );
            }
        }
    }

    bail!(
        "{kind} output failed the shape check after retries:\n{}",
        format_violations(&last_violations)
    )
}

fn is_only_invalid_json<T>(outcome: &std::result::Result<T, Vec<Violation>>) -> bool {
    matches!(outcome, Err(v) if v.len() == 1 && v[0].code == "invalid_json")
}

fn evaluate_proposal<T, C>(raw: &str, check: &C) -> std::result::Result<T, Vec<Violation>>
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
    let violations = check(&parsed);
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
    fn claude_result_event_is_the_answer() {
        let sink = noop_sink();
        let mut result = None;
        for line in [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"{\"a\":1}"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"{\"a\":1}"}"#,
        ] {
            process_claude_event(
                &serde_json::from_str(line).unwrap(),
                false,
                &sink,
                &mut result,
            );
        }
        assert_eq!(result, Some((false, r#"{"a":1}"#.to_string())));
    }

    #[test]
    fn codex_last_agent_message_is_the_answer() {
        let sink = noop_sink();
        let (mut last, mut err) = (None, None);
        for line in [
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"Checking."}}"#,
            r#"{"type":"item.started","item":{"type":"command_execution","command":"rg name"}}"#,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"a\":1}"}}"#,
            r#"{"type":"turn.completed"}"#,
        ] {
            process_codex_event(
                &serde_json::from_str(line).unwrap(),
                false,
                &sink,
                &mut last,
                &mut err,
            );
        }
        assert_eq!(last.as_deref(), Some(r#"{"a":1}"#));
        assert!(err.is_none());
    }

    #[test]
    fn extracts_json_from_fenced_block() {
        let raw = "Sure:\n```json\n{\"a\": 1}\n```";
        assert_eq!(extract_json_object(raw), "{\"a\": 1}");
    }
}
