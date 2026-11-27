//! External CLI backends: Claude Code (`claude -p`) and Codex (`codex exec`).
//! Each CLI owns its own login and billing.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::{BoxFuture, Engine, EngineRequest, StreamEvent, StreamSink, ToolAccess};
use crate::config::{Config, Route};
use crate::stream::{ToolKind, ToolStart, describe_tool, strip_shell_wrapper};

/// Runs models through the Claude Code or Codex CLI.
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
            // Tool call ids must be unique per thread; CLI ids repeat across runs.
            let run = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
            match model.route {
                Route::Claude => self.run_claude(req, sink, &run).await,
                Route::Codex => self.run_codex(req, sink, &run).await,
                other => bail!("{} is not a CLI route", other.label()),
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

    // Stopping the turn (Esc in the TUI) kills the CLI.
    let cancel = super::turn_ctx().cancel;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        let read = tokio::select! {
            r = reader.read_line(&mut line) => r?,
            _ = cancel.cancelled() => {
                child.kill().await.ok();
                stderr_task.abort();
                bail!("stopped");
            }
        };
        if read == 0 {
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
    /// Claude Code hides its thinking in `-p` mode unless asked with
    /// `--thinking-display summarized`; versions without the flag run without it.
    async fn run_claude(&self, req: &EngineRequest, sink: StreamSink, run: &str) -> Result<String> {
        match self.run_claude_with(req, sink.clone(), run, true).await {
            Err(e) if e.to_string().contains("thinking-display") => {
                self.run_claude_with(req, sink, run, false).await
            }
            other => other,
        }
    }

    async fn run_claude_with(
        &self,
        req: &EngineRequest,
        sink: StreamSink,
        run: &str,
        show_thinking: bool,
    ) -> Result<String> {
        let model = req.model();
        let bin = resolve_bin(&self.config.claude_bin, "PATHFINDER_CLAUDE_BIN")?;
        let mut cmd = Command::new(&bin);
        cmd.current_dir(&req.cwd).args([
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--no-session-persistence",
            "--strict-mcp-config",
        ]);
        if show_thinking {
            cmd.args(["--thinking-display", "summarized"]);
        }
        if !model.model.is_empty() {
            cmd.args(["--model", &model.model]);
        }
        if let Some(effort) = model.effort.as_deref().filter(|e| !e.is_empty()) {
            cmd.args(["--effort", effort]);
        }
        // Where Claude Code can't sandbox commands (Windows, Linux without
        // bubblewrap and socat), full access leaves out Bash: pre-allowing it
        // there would run the agent's commands unsandboxed without asking.
        let sandboxed = claude_can_sandbox();
        let mut system_prompt = std::borrow::Cow::Borrowed(req.system_prompt.as_str());
        if req.tools == ToolAccess::Full && !sandboxed {
            system_prompt =
                std::borrow::Cow::Owned(format!("{}\n\n{NO_SHELL_NOTE}", req.system_prompt));
        }
        // Windows caps a whole command line at 32,767 characters and Linux
        // one argument at 128 KiB: a long prompt goes in a file instead.
        let _prompt_file = if prompt_needs_file(&system_prompt) {
            let file = TempPrompt::write(&system_prompt)?;
            cmd.arg("--system-prompt-file").arg(&file.path);
            Some(file)
        } else {
            cmd.args(["--system-prompt", &system_prompt]);
            None
        };
        // Attached files (a pasted screenshot, a PDF from elsewhere) may lie
        // outside the workspace: let Claude Code read their folders.
        for dir in attachment_dirs(&req.files, &req.cwd) {
            cmd.arg("--add-dir").arg(dir);
        }
        match req.tools {
            ToolAccess::None => {
                cmd.args(["--tools", ""]);
            }
            ToolAccess::ReadOnly => {
                cmd.args(["--tools", "Read,Glob,Grep"]);
            }
            ToolAccess::Full if !sandboxed => {
                cmd.args([
                    "--tools",
                    "Read,Glob,Grep,Edit,Write",
                    "--permission-mode",
                    "acceptEdits",
                ]);
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
        let mut thought_streamed = false;
        let (status, stderr) = run_jsonl(
            cmd,
            &bin.to_string_lossy(),
            Some(req.prompt_with_files()),
            |val| {
                process_claude_event(
                    val,
                    req.stream_text,
                    &sink,
                    run,
                    &req.cwd,
                    &mut text_started,
                    &mut thought_streamed,
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
        // Codex's Windows sandbox is experimental and opt-in; without it,
        // `workspace-write` would run the agent's commands unsandboxed.
        if cfg!(windows) && req.tools == ToolAccess::Full {
            bail!(
                "Codex can't sandbox commands on Windows yet. Pick an API or ChatGPT \
                 model for this, which asks you before each command."
            );
        }
        let bin = resolve_bin(&self.config.codex_bin, "PATHFINDER_CODEX_BIN")?;
        let mut cmd = Command::new(&bin);
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
        // Images go to Codex as images; other files are listed in the prompt.
        for f in req
            .files
            .iter()
            .filter(|f| crate::llm::attachable_type(f).is_some_and(|t| t.starts_with("image/")))
        {
            cmd.arg("-i").arg(f);
        }
        if let Some(effort) = model.effort.as_deref().filter(|e| !e.is_empty()) {
            cmd.args(["-c", &format!("model_reasoning_effort=\"{effort}\"")]);
        }
        // Without this, `codex exec` emits no reasoning at all.
        cmd.args(["-c", "model_reasoning_summary=\"detailed\""]);
        // Codex has no system-prompt flag; the instructions lead the prompt.
        let mut prompt = format!("<instructions>\n{}\n</instructions>\n\n", req.system_prompt);
        if req.tools == ToolAccess::None {
            prompt.push_str("Do not run any commands or read any files for this answer.\n\n");
        }
        prompt.push_str(&req.prompt_with_files());

        let mut last_message: Option<String> = None;
        let mut last_error: Option<String> = None;
        let (status, stderr) = run_jsonl(cmd, &bin.to_string_lossy(), Some(prompt), |val| {
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
    thought_streamed: &mut bool,
    result: &mut Option<(bool, String)>,
) {
    match val.get("type").and_then(Value::as_str) {
        // Token-level text and thinking, from --include-partial-messages.
        Some("stream_event") => {
            let event = &val["event"];
            if let Some(t) = event
                .pointer("/delta/thinking")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                *thought_streamed = true;
                sink(StreamEvent::Thought(t.to_string()));
                return;
            }
            if !stream_text {
                return;
            }
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
                    // Already streamed as deltas when partial messages are on.
                    Some("thinking") if !*thought_streamed => {
                        if let Some(t) = item.get("thinking").and_then(Value::as_str)
                            && !t.is_empty()
                        {
                            sink(StreamEvent::Thought(t.to_string()));
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
            // Summaries come as heading lines; keep each its own paragraph.
            let t = text()
                .lines()
                .filter(|l| !l.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            if !t.is_empty() {
                sink(StreamEvent::Thought(format!("{t}\n\n")));
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

/// Told to the model when Claude Code runs without Bash because it can't
/// sandbox commands on this machine.
const NO_SHELL_NOTE: &str = "Shell commands are unavailable in this run: Claude Code can't \
sandbox them on this machine. Read and edit files instead, and tell the user which commands \
to run themselves.";

/// Whether Claude Code's own sandbox works here: Seatbelt on macOS, and on
/// Linux bubblewrap and socat. It has none on Windows.
fn claude_can_sandbox() -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    if cfg!(target_os = "linux") {
        return matches!(
            crate::agent::sandbox::available(),
            Some(crate::agent::sandbox::Kind::Bwrap(_))
        ) && on_path("socat");
    }
    false
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

/// Linux caps one argument at 128 KiB; stay well under it.
const LONG_ARG: usize = 100_000;

/// Whether the system prompt must go through a file rather than the
/// command line.
fn prompt_needs_file(prompt: &str) -> bool {
    cfg!(windows) || (cfg!(target_os = "linux") && prompt.len() > LONG_ARG)
}

/// A system prompt written to a private temp file, removed when dropped.
struct TempPrompt {
    path: PathBuf,
}

impl TempPrompt {
    fn write(text: &str) -> Result<Self> {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!(
            "pathfinder-prompt-{}.md",
            uuid::Uuid::new_v4().simple()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        let prompt = Self { path };
        file.write_all(text.as_bytes())
            .with_context(|| format!("writing {}", prompt.path.display()))?;
        Ok(prompt)
    }
}

impl Drop for TempPrompt {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

/// The program to spawn for a CLI setting such as `claude`. Elsewhere it is
/// used as is; on Windows a native `.exe` is looked up on PATH, because std
/// finds only `.exe` files and won't pass long, quoted arguments through a
/// `.cmd` shim (npm installs those).
fn resolve_bin(bin: &str, env_var: &str) -> Result<PathBuf> {
    if !cfg!(windows) {
        return Ok(PathBuf::from(bin));
    }
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    resolve_windows_bin(bin, env_var, &dirs, |p| p.is_file())
}

fn windows_target() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64-pc-windows-msvc"
    } else {
        "x86_64-pc-windows-msvc"
    }
}

fn is_script(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// [`resolve_bin`]'s Windows lookup over `dirs` (PATH), with `is_file`
/// standing in for the disk.
fn resolve_windows_bin(
    bin: &str,
    env_var: &str,
    dirs: &[PathBuf],
    is_file: impl Fn(&Path) -> bool,
) -> Result<PathBuf> {
    let script_error = |script: &Path| {
        anyhow!(
            "{} is a .cmd script, which can't safely carry Pathfinder's long prompts on \
             Windows. Install the CLI's native Windows build (claude.exe or codex.exe) or \
             set {env_var} to its .exe.",
            script.display()
        )
    };
    let given = Path::new(bin);
    if is_script(given) {
        return Err(script_error(given));
    }
    let with_ext = |p: &Path, ext: &str| {
        let mut s = p.as_os_str().to_os_string();
        s.push(".");
        s.push(ext);
        PathBuf::from(s)
    };
    let has_ext = given.extension().is_some();
    // A path, not a bare name: only add `.exe` if it's missing.
    if given.components().count() > 1 || given.is_absolute() {
        if !has_ext {
            let exe = with_ext(given, "exe");
            if is_file(&exe) {
                return Ok(exe);
            }
            if let Some(s) = ["cmd", "bat"]
                .iter()
                .map(|e| with_ext(given, e))
                .find(|s| is_file(s))
            {
                return Err(script_error(&s));
            }
        }
        return Ok(given.to_path_buf());
    }
    if has_ext {
        return Ok(dirs
            .iter()
            .map(|d| d.join(bin))
            .find(|p| is_file(p))
            .unwrap_or_else(|| given.to_path_buf()));
    }
    // A native .exe anywhere on PATH wins over npm's shims.
    if let Some(exe) = dirs
        .iter()
        .map(|d| with_ext(&d.join(bin), "exe"))
        .find(|p| is_file(p))
    {
        return Ok(exe);
    }
    let shim = dirs.iter().find_map(|d| {
        ["cmd", "bat"]
            .iter()
            .map(|e| with_ext(&d.join(bin), e))
            .find(|s| is_file(s))
            .map(|s| (d, s))
    });
    if let Some((dir, script)) = shim {
        // The codex npm package ships its native binary beside the shim.
        if bin.eq_ignore_ascii_case("codex") {
            let vendored = dir
                .join("node_modules")
                .join("@openai")
                .join("codex")
                .join("vendor")
                .join(windows_target())
                .join("codex")
                .join("codex.exe");
            if is_file(&vendored) {
                return Ok(vendored);
            }
        }
        return Err(script_error(&script));
    }
    // Not found: spawning reports it as before.
    Ok(given.to_path_buf())
}

/// Folders of attached files outside `cwd`, once each.
fn attachment_dirs(files: &[std::path::PathBuf], cwd: &std::path::Path) -> Vec<std::path::PathBuf> {
    let cwd = dunce::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    for f in files {
        let Some(dir) = f.parent() else { continue };
        let dir = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if !dir.starts_with(&cwd) && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn stopping_the_turn_kills_the_cli() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo '{}'; sleep 10"]);
        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = crate::engine::RunContext {
            cancel: cancel.clone(),
            host: None,
        };
        let started = std::time::Instant::now();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            stopper.cancel();
        });
        let result = crate::engine::with_turn(ctx, run_jsonl(cmd, "sh", None, |_| {})).await;
        assert!(result.is_err(), "a stopped run is an error");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "killed, not waited for"
        );
    }

    #[test]
    fn windows_lookup_prefers_native_binaries_over_npm_shims() {
        let npm = PathBuf::from("/u/npm");
        let local = PathBuf::from("/u/local");
        let dirs = vec![npm.clone(), local.clone()];
        let vendored = npm
            .join("node_modules/@openai/codex/vendor")
            .join(windows_target())
            .join("codex/codex.exe");
        let files = [
            npm.join("claude.cmd"),
            local.join("claude.exe"),
            npm.join("codex.cmd"),
            vendored.clone(),
            npm.join("other.cmd"),
        ];
        let is_file = |p: &Path| files.iter().any(|f| f == p);
        let resolve = |bin: &str| resolve_windows_bin(bin, "VAR", &dirs, is_file);
        assert_eq!(resolve("claude").unwrap(), local.join("claude.exe"));
        assert_eq!(resolve("codex").unwrap(), vendored);
        let err = resolve("other").unwrap_err().to_string();
        assert!(err.contains("other.cmd") && err.contains("VAR"), "{err}");
        assert!(resolve("x.cmd").is_err());
        assert_eq!(resolve("missing").unwrap(), PathBuf::from("missing"));
        assert_eq!(
            resolve("/u/local/claude").unwrap(),
            local.join("claude.exe")
        );
        assert!(resolve("/u/npm/other").is_err());
    }

    #[test]
    fn long_prompts_go_through_a_private_file_removed_after() {
        let file = TempPrompt::write("be brief").unwrap();
        let path = file.path.clone();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "be brief");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        drop(file);
        assert!(!path.exists());
        assert!(!prompt_needs_file("short") || cfg!(windows));
    }

    #[test]
    fn attachments_outside_the_workspace_get_their_folder_added() {
        let ws = std::env::temp_dir().join(format!("pf-ws-{}", uuid::Uuid::new_v4().simple()));
        let out = std::env::temp_dir().join(format!("pf-out-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let files = vec![ws.join("a.pdf"), out.join("x.png"), out.join("y.png")];
        let dirs = attachment_dirs(&files, &ws);
        assert_eq!(dirs, vec![dunce::canonicalize(&out).unwrap()]);
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
                StreamEvent::ToolUpdate { id, .. } => format!("update {id}"),
            };
            record.lock().unwrap().push(line);
        });
        (sink, events)
    }

    #[test]
    fn claude_streams_text_and_tool_calls() {
        let (sink, events) = recording_sink();
        let (mut result, mut text_started, mut thought_streamed) = (None, false, false);
        for line in [
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"Checking a.rs"}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Checking a.rs","signature":"s"}]}}"#,
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
                &mut thought_streamed,
                &mut result,
            );
        }
        assert_eq!(result, Some((false, r#"{"a":1}"#.to_string())));
        assert_eq!(
            *events.lock().unwrap(),
            [
                // Streamed once; the full block that follows isn't repeated.
                "thought Checking a.rs",
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
}
