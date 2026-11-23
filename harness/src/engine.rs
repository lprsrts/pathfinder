use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::config::{Backend, Config, ModelEntry};
use crate::store::Store;
use crate::stream::{FieldStreamer, ToolKind, ToolStart, describe_tool, strip_shell_wrapper};
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
    ToolStart(ToolStart),
    ToolEnd { id: String, ok: bool },
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
            // Tool call ids must be unique per thread; CLI ids repeat across runs.
            let run = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
            match model.backend {
                Backend::Pi => self.run_pi(req, sink, &run).await,
                Backend::Claude => self.run_claude(req, sink, &run).await,
                Backend::Codex => self.run_codex(req, sink, &run).await,
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
    async fn run_pi(&self, req: &EngineRequest, sink: StreamSink, run: &str) -> Result<String> {
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
                &PiCtx {
                    stream_text: req.stream_text,
                    sink: &sink,
                    run,
                    cwd: &req.cwd,
                },
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

    async fn run_claude(&self, req: &EngineRequest, sink: StreamSink, run: &str) -> Result<String> {
        let model = req.model();
        let mut cmd = Command::new(&self.config.claude_bin);
        cmd.current_dir(&req.cwd).args([
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
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
        let mut text_started = false;
        let (status, stderr) = run_jsonl(
            cmd,
            &self.config.claude_bin,
            Some(req.prompt_with_files()),
            |val| {
                process_claude_event(
                    val,
                    req.stream_text,
                    &sink,
                    run,
                    &req.cwd,
                    &mut text_started,
                    &mut result,
                )
            },
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

    async fn run_codex(&self, req: &EngineRequest, sink: StreamSink, run: &str) -> Result<String> {
        let model = req.model();
        let mut cmd = Command::new(&self.config.codex_bin);
        cmd.arg("exec")
            .args([
                "--json",
                "--ignore-user-config",
                "--skip-git-repo-check",
                "--ephemeral",
                // No MCP servers, account apps (ChatGPT connectors), plugins
                // or hooks: Pathfinder runs see only the workspace.
                "-c",
                "mcp_servers={}",
                "--disable",
                "apps",
                "--disable",
                "plugins",
                "--disable",
                "hooks",
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
                run,
                &req.cwd,
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

#[allow(clippy::too_many_arguments)]
fn process_claude_event(
    val: &Value,
    stream_text: bool,
    sink: &StreamSink,
    run: &str,
    cwd: &Path,
    text_started: &mut bool,
    result: &mut Option<(bool, String)>,
) {
    match val.get("type").and_then(Value::as_str) {
        // Token-level text, from --include-partial-messages.
        Some("stream_event") if stream_text => {
            let event = &val["event"];
            match event.get("type").and_then(Value::as_str) {
                Some("content_block_start")
                    if event.pointer("/content_block/type").and_then(Value::as_str)
                        == Some("text")
                        && *text_started =>
                {
                    sink(StreamEvent::TextDelta("\n\n".to_string()));
                }
                Some("content_block_delta") => {
                    if let Some(t) = event.pointer("/delta/text").and_then(Value::as_str) {
                        *text_started = true;
                        sink(StreamEvent::TextDelta(t.to_string()));
                    }
                }
                _ => {}
            }
        }
        Some("assistant") => {
            let content = val
                .pointer("/message/content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for item in content {
                match item.get("type").and_then(Value::as_str) {
                    Some("thinking") => {
                        if let Some(t) = item.get("thinking").and_then(Value::as_str) {
                            if !t.is_empty() {
                                sink(StreamEvent::Thought(t.to_string()));
                            }
                        }
                    }
                    Some("tool_use") => {
                        let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
                        let name = item.get("name").and_then(Value::as_str).unwrap_or("tool");
                        sink(StreamEvent::ToolStart(describe_tool(
                            format!("{run}-{id}"),
                            name,
                            item.get("input").unwrap_or(&Value::Null),
                            cwd,
                        )));
                    }
                    _ => {}
                }
            }
        }
        Some("user") => {
            let content = val
                .pointer("/message/content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for item in content {
                if item.get("type").and_then(Value::as_str) == Some("tool_result") {
                    let id = item
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let ok = !item
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    sink(StreamEvent::ToolEnd {
                        id: format!("{run}-{id}"),
                        ok,
                    });
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

#[allow(clippy::too_many_arguments)]
fn process_codex_event(
    val: &Value,
    stream_text: bool,
    sink: &StreamSink,
    run: &str,
    cwd: &Path,
    last_message: &mut Option<String>,
    last_error: &mut Option<String>,
) {
    let item = val.get("item").cloned().unwrap_or(Value::Null);
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    let id = format!(
        "{run}-{}",
        item.get("id").and_then(Value::as_str).unwrap_or_default()
    );
    let text = || {
        item.get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let started = |title: String, kind: ToolKind, path: Option<PathBuf>| {
        sink(StreamEvent::ToolStart(ToolStart {
            id: id.clone(),
            title,
            kind,
            path,
        }))
    };
    let ended = |ok: bool| sink(StreamEvent::ToolEnd { id: id.clone(), ok });
    let status_ok = || {
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("completed");
        let exit_ok = item
            .get("exit_code")
            .and_then(Value::as_i64)
            .is_none_or(|c| c == 0);
        status != "failed" && exit_ok
    };

    match (val.get("type").and_then(Value::as_str), item_type) {
        (Some("item.started"), "command_execution") => {
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            started(
                format!("Run `{}`", strip_shell_wrapper(command)),
                ToolKind::Execute,
                None,
            );
        }
        (Some("item.completed"), "command_execution") => ended(status_ok()),
        (Some("item.started"), "mcp_tool_call") => {
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("tool");
            started(tool.to_string(), ToolKind::Other, None);
        }
        (Some("item.completed"), "mcp_tool_call") => ended(status_ok()),
        (Some("item.started"), "web_search") => {
            let query = item
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            started(format!("Search the web `{query}`"), ToolKind::Fetch, None);
        }
        (Some("item.completed"), "web_search") => ended(true),
        (Some("item.completed"), "file_change") => {
            let paths: Vec<PathBuf> = item
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|c| c.get("path").and_then(Value::as_str))
                .map(|p| {
                    let p = PathBuf::from(p);
                    if p.is_absolute() { p } else { cwd.join(p) }
                })
                .collect();
            let shown: Vec<String> = paths
                .iter()
                .map(|p| p.strip_prefix(cwd).unwrap_or(p).display().to_string())
                .collect();
            started(
                format!("Edit {}", shown.join(", ")),
                ToolKind::Edit,
                paths.first().cloned(),
            );
            ended(status_ok());
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

struct PiCtx<'a> {
    stream_text: bool,
    sink: &'a StreamSink,
    run: &'a str,
    cwd: &'a Path,
}

fn process_pi_event(
    val: &Value,
    ctx: &PiCtx<'_>,
    final_text: &mut Option<String>,
    delta_buf: &mut String,
    last_error: &mut Option<String>,
) {
    let sink = ctx.sink;
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
                            if ctx.stream_text {
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
            let id = val
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = val
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            sink(StreamEvent::ToolStart(describe_tool(
                format!("{}-{id}", ctx.run),
                name,
                val.get("args").unwrap_or(&Value::Null),
                ctx.cwd,
            )));
        }
        Some("tool_execution_end") => {
            let id = val
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let ok = !val.get("isError").and_then(Value::as_bool).unwrap_or(false);
            sink(StreamEvent::ToolEnd {
                id: format!("{}-{id}", ctx.run),
                ok,
            });
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
            &PiCtx {
                stream_text: false,
                sink: &sink,
                run: "t",
                cwd: Path::new("."),
            },
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
    run_gated_streamed(engine, store, session_id, kind, base_req, sink, check, None)
        .await
        .map(|(value, _)| value)
}

/// Like `run_gated`, but streams the JSON string `field` (e.g. `"reply"`) to
/// the user as it is generated. Returns the value and the text shown for the
/// accepted attempt, so callers don't send it twice.
#[allow(clippy::too_many_arguments)]
pub async fn run_gated_streamed<T, C>(
    engine: &dyn Engine,
    store: &Store,
    session_id: &str,
    kind: &str,
    base_req: &EngineRequest,
    sink: StreamSink,
    check: C,
    field: Option<&str>,
) -> Result<(T, String)>
where
    T: DeserializeOwned,
    C: Fn(&T) -> Vec<Violation>,
{
    let mut req = base_req.clone();
    req.stream_text = field.is_some();
    let mut shown_any = false;
    // Wraps the sink so raw JSON never reaches the user, only the decoded
    // `field`. Returns the wrapped sink and a record of what it showed.
    let field_sink = |shown_any: bool| -> (StreamSink, Arc<Mutex<String>>) {
        let shown = Arc::new(Mutex::new(String::new()));
        let Some(field) = field else {
            return (sink.clone(), shown);
        };
        let streamer = Mutex::new(FieldStreamer::new(field));
        let separator_due = Mutex::new(shown_any);
        let inner = sink.clone();
        let record = shown.clone();
        let wrapped: StreamSink = Arc::new(move |ev| match ev {
            StreamEvent::TextDelta(t) => {
                let out = streamer.lock().unwrap().push(&t);
                if out.is_empty() {
                    return;
                }
                // A retry after visible text starts a new paragraph.
                if std::mem::take(&mut *separator_due.lock().unwrap()) {
                    inner(StreamEvent::TextDelta("\n\n".to_string()));
                }
                record.lock().unwrap().push_str(&out);
                inner(StreamEvent::TextDelta(out));
            }
            other => inner(other),
        });
        (wrapped, shown)
    };

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

        let (attempt_sink, mut shown) = field_sink(shown_any);
        let raw = engine.run_text(&req, attempt_sink).await?;
        shown_any |= !shown.lock().unwrap().is_empty();
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
            let (repair_sink, repair_shown) = field_sink(shown_any);
            let repaired = engine.run_text(&repair, repair_sink).await?;
            shown_any |= !repair_shown.lock().unwrap().is_empty();
            shown = repair_shown;
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
                let shown = shown.lock().unwrap().clone();
                return Ok((val, shown));
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

    fn recording_sink() -> (StreamSink, Arc<Mutex<Vec<String>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let record = events.clone();
        let sink: StreamSink = Arc::new(move |ev| {
            let line = match ev {
                StreamEvent::Thought(t) => format!("thought {t}"),
                StreamEvent::TextDelta(t) => format!("text {t}"),
                StreamEvent::ToolStart(t) => format!("start {} {}", t.id, t.title),
                StreamEvent::ToolEnd { id, ok } => format!("end {id} {ok}"),
            };
            record.lock().unwrap().push(line);
        });
        (sink, events)
    }

    #[test]
    fn claude_streams_text_and_tool_calls() {
        let (sink, events) = recording_sink();
        let (mut result, mut text_started) = (None, false);
        for line in [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu1","name":"Read","input":{"file_path":"/w/a.rs"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"..."}]}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"text","text":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"{\"a\""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":":1}"}}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"{\"a\":1}"}"#,
        ] {
            process_claude_event(
                &serde_json::from_str(line).unwrap(),
                true,
                &sink,
                "r",
                Path::new("/w"),
                &mut text_started,
                &mut result,
            );
        }
        assert_eq!(result, Some((false, r#"{"a":1}"#.to_string())));
        assert_eq!(
            *events.lock().unwrap(),
            [
                "start r-tu1 Read a.rs",
                "end r-tu1 true",
                r#"text {"a""#,
                "text :1}"
            ]
        );
    }

    #[test]
    fn codex_tool_calls_and_last_agent_message() {
        let (sink, events) = recording_sink();
        let (mut last, mut err) = (None, None);
        for line in [
            r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Checking."}}"#,
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc \"rg name\""}}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"x","exit_code":1,"status":"failed"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"{\"a\":1}"}}"#,
            r#"{"type":"turn.completed"}"#,
        ] {
            process_codex_event(
                &serde_json::from_str(line).unwrap(),
                false,
                &sink,
                "r",
                Path::new("/w"),
                &mut last,
                &mut err,
            );
        }
        assert_eq!(last.as_deref(), Some(r#"{"a":1}"#));
        assert!(err.is_none());
        assert_eq!(
            *events.lock().unwrap(),
            ["start r-item_1 Run `rg name`", "end r-item_1 false"]
        );
    }

    #[test]
    fn extracts_json_from_fenced_block() {
        let raw = "Sure:\n```json\n{\"a\": 1}\n```";
        assert_eq!(extract_json_object(raw), "{\"a\": 1}");
    }
}
