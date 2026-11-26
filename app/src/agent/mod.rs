//! Pathfinder's own agent loop: calls a model API, runs tools, repeats until
//! the model answers. Sits behind the `Engine` trait, so the theory loop in
//! runtime.rs doesn't know or care which route ran.

pub mod confine;
pub mod sandbox;
pub mod tools;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use futures_util::future::join_all;

use crate::auth::Credentials;
use crate::auth::chatgpt::ChatGptError;
use crate::config::{ModelEntry, Route};
use crate::engine::{
    BoxFuture, CommandOutput, Engine, EngineRequest, EngineTier, FinalCheck, Permission,
    StreamEvent, StreamSink, ToolAccess,
};
use crate::llm::anthropic::Anthropic;
use crate::llm::chat::Chat;
use crate::llm::responses::{Bearer, Responses};
use crate::llm::{Llm, LlmError, LlmEvent, LlmRequest, Message, Part, ToolResult, text_of};
use crate::stream::{ToolContent, describe_tool};
use tools::ToolEnv;

const MAX_TOOL_OUTPUT: usize = 50 * 1024;
const MAX_ATTEMPTS: u32 = 5;
/// Tool output kept in the conversation before older results are cleared.
const PRUNE_ABOVE: usize = 300 * 1024;
/// Recent steps whose tool results are never cleared.
const KEEP_RECENT: usize = 3;
const MAX_ATTACHED_INLINE: u64 = 64 * 1024;

const PROTOCOL_NOTE: &str = "\n\n---\nYou can inspect the user's workspace with the tools provided. When you are done, your final message must contain only the requested output.";

/// Builds a client for a model entry.
pub type LlmFactory = Arc<dyn Fn(&ModelEntry) -> Result<Arc<dyn Llm>> + Send + Sync>;

pub struct ApiEngine {
    factory: LlmFactory,
}

impl ApiEngine {
    pub fn new(factory: LlmFactory) -> Self {
        Self { factory }
    }

    /// Clients authenticated with keys from env vars or the auth file, or
    /// with the Sign in with ChatGPT session.
    pub fn with_credentials(creds: Arc<Credentials>) -> Self {
        let http = reqwest::Client::new();
        Self::new(Arc::new(
            move |entry: &ModelEntry| -> Result<Arc<dyn Llm>> {
                let base = |var: &str| {
                    crate::envvar::get(var)
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                };
                if entry.route == Route::Chatgpt {
                    let session = creds.chatgpt();
                    if !session.signed_in() {
                        return Err(ChatGptError::NotSignedIn.into());
                    }
                    return Ok(Arc::new(Responses::with_bearer(
                        http.clone(),
                        Bearer::ChatGpt(session),
                        base("PATHFINDER_OPENAI_BASE_URL"),
                    )));
                }
                if entry.route == Route::Endpoint {
                    let cfg = crate::config::Config::load();
                    let id = entry.endpoint.as_deref().unwrap_or_default();
                    let ep = cfg
                        .endpoint(id)
                        .ok_or_else(|| anyhow::anyhow!("no endpoint `{id}` in the config"))?;
                    let key = match &ep.key {
                        Some(name) => Some(creds.stored(name).ok_or_else(|| {
                            anyhow::anyhow!(
                                "endpoint `{id}` needs the key `{name}`: run `pathfinder auth import {name}` or `pathfinder auth set {name}`"
                            )
                        })?),
                        None => None,
                    };
                    return Ok(Arc::new(
                        Chat::new(http.clone(), ep.base_url.clone(), key)
                            .with_extra(ep.extra.clone()),
                    ));
                }
                let key = creds.api_key(entry.route)?;
                Ok(match entry.route {
                    Route::Anthropic => Arc::new(Anthropic::new(
                        http.clone(),
                        key,
                        base("PATHFINDER_ANTHROPIC_BASE_URL"),
                    )),
                    Route::Openai => Arc::new(Responses::new(
                        http.clone(),
                        key,
                        base("PATHFINDER_OPENAI_BASE_URL"),
                    )),
                    other => bail!("{} is not an API route", other.label()),
                })
            },
        ))
    }
}

fn max_steps(req: &EngineRequest) -> usize {
    match (req.tools, req.tier) {
        (ToolAccess::None, _) => 1,
        (ToolAccess::ReadOnly, EngineTier::System1) => 16,
        (ToolAccess::ReadOnly, EngineTier::System2) => 24,
        (ToolAccess::Full, _) => 80,
    }
}

/// Largest image or PDF sent to the model as content (the providers' own
/// limits are around 20–32 MB per request).
const MAX_ATTACHMENT: u64 = 20 * 1024 * 1024;

/// The first user message: the prompt, small text files inlined, and
/// images and PDFs sent as content the model can see.
fn first_message(req: &EngineRequest) -> Message {
    use base64::Engine as _;
    let mut text = req.prompt.clone();
    let mut attachments = Vec::new();
    for file in &req.files {
        let Ok(meta) = std::fs::metadata(file) else {
            continue;
        };
        if let Some(media_type) = crate::llm::attachable_type(file)
            && meta.len() <= MAX_ATTACHMENT
            && let Ok(bytes) = std::fs::read(file)
        {
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            let _ = write!(text, "\n\n(Attached: {name})");
            attachments.push(crate::llm::Attachment {
                name,
                media_type: media_type.into(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            });
            continue;
        }
        if meta.len() <= MAX_ATTACHED_INLINE
            && let Ok(content) = std::fs::read_to_string(file)
        {
            let _ = write!(
                text,
                "\n\n<attached path=\"{}\">\n{content}\n</attached>",
                file.display()
            );
            continue;
        }
        let _ = write!(
            text,
            "\n\nAttached file (read it as needed): {}",
            file.display()
        );
    }
    if attachments.is_empty() {
        Message::User(text)
    } else {
        Message::UserWith(text, attachments)
    }
}

/// Output cut to the cap, keeping the start and the end: errors and test
/// summaries come last.
fn cap(s: String) -> String {
    if s.len() <= MAX_TOOL_OUTPUT {
        return s;
    }
    let half = MAX_TOOL_OUTPUT / 2;
    let mut head = half;
    while !s.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = s.len() - half;
    while !s.is_char_boundary(tail) {
        tail += 1;
    }
    let lines = s[head..tail].lines().count();
    format!(
        "{}\n… ({lines} lines, {} KB cut from the middle) …\n{}",
        &s[..head],
        (tail - head) / 1024,
        &s[tail..]
    )
}

/// Where and when the agent works, for the system prompt: the date matters
/// for deadlines and exams, the access level for what it may try.
fn environment(req: &EngineRequest) -> String {
    let date = std::process::Command::new("date")
        .arg("+%Y-%m-%d (%A), %H:%M %Z")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let access = match req.tools {
        ToolAccess::None => "none: answer from what you are given",
        ToolAccess::ReadOnly => "read-only: you can read and search, not change anything",
        ToolAccess::Full => {
            "full: you can edit files and run commands (the user approves commands)"
        }
    };
    let mut env = format!(
        "\n\n<environment>\nWorkspace: {}\nToday: {date}\nPlatform: {}\nTool access: {access}\n</environment>",
        req.cwd.display(),
        std::env::consts::OS,
    );
    if req.tools == ToolAccess::Full {
        // The project's own instructions for agents, as other agents read them.
        for name in ["AGENTS.md", "CLAUDE.md"] {
            if let Ok(text) = std::fs::read_to_string(req.cwd.join(name)) {
                let text: String = text.chars().take(8 * 1024).collect();
                let _ = write!(
                    env,
                    "\n\n<project-instructions file=\"{name}\">\n{}\n</project-instructions>",
                    text.trim()
                );
                break;
            }
        }
    }
    env
}

/// A stable prompt-cache key per workspace, so the OpenAI routes reuse the
/// cached prefix across steps and turns.
fn cache_key(req: &EngineRequest) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    req.cwd.hash(&mut h);
    format!("pathfinder-{:016x}", h.finish())
}

/// Clears old tool results once they add up past `PRUNE_ABOVE`, keeping
/// the last `KEEP_RECENT` steps. Calls stay paired with their results.
fn prune(messages: &mut [Message]) {
    let size = |m: &Message| match m {
        Message::ToolResults(rs) => rs
            .iter()
            .map(|r| r.content.len() + r.images.iter().map(|i| i.data.len()).sum::<usize>())
            .sum(),
        _ => 0,
    };
    if messages.iter().map(size).sum::<usize>() <= PRUNE_ABOVE {
        return;
    }
    let steps: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| matches!(m, Message::ToolResults(_)))
        .map(|(i, _)| i)
        .collect();
    for &i in steps.iter().rev().skip(KEEP_RECENT) {
        if let Message::ToolResults(rs) = &mut messages[i] {
            for r in rs {
                if r.content.len() > 400 || !r.images.is_empty() {
                    r.content =
                        "[output cleared to save room; run the tool again if you need it]".into();
                    r.images.clear();
                }
            }
        }
    }
}

/// A tool call's fingerprint with its result, to notice repetition.
fn fingerprint(call: &crate::llm::ToolCall, result: &ToolResult) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    call.name.hash(&mut h);
    call.args.to_string().hash(&mut h);
    result.content.hash(&mut h);
    h.finish()
}

fn read_only_tool(name: &str) -> bool {
    matches!(name, "read" | "view" | "ls" | "glob" | "grep")
}

/// Times a failing final answer is sent back for fixing in the same
/// conversation before the gate retries from scratch.
const MAX_FIXES: usize = 2;

impl Engine for ApiEngine {
    fn run_text<'a>(
        &'a self,
        req: &'a EngineRequest,
        sink: StreamSink,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(self.agent_loop(req, sink, None))
    }

    fn run_checked<'a>(
        &'a self,
        req: &'a EngineRequest,
        sink: StreamSink,
        check: FinalCheck<'a>,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(self.agent_loop(req, sink, Some(check)))
    }
}

impl ApiEngine {
    async fn agent_loop(
        &self,
        req: &EngineRequest,
        sink: StreamSink,
        check: Option<FinalCheck<'_>>,
    ) -> Result<String> {
        {
            let model = req.model();
            let llm = (self.factory)(model)?;
            let specs = match req.tools {
                ToolAccess::None => Vec::new(),
                ToolAccess::ReadOnly => tools::read_only_specs(),
                ToolAccess::Full => tools::full_specs(),
            };
            let run = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
            let root: PathBuf = req.cwd.clone();
            let mut request = LlmRequest {
                model: model.model.clone(),
                instructions: format!("{}{}{}", req.system_prompt, PROTOCOL_NOTE, environment(req)),
                messages: vec![first_message(req)],
                tools: specs,
                // Unset effort follows the tier: quick for System 1.
                effort: model.effort.clone().or_else(|| {
                    Some(
                        match req.tier {
                            EngineTier::System1 => "low",
                            EngineTier::System2 => "medium",
                        }
                        .to_string(),
                    )
                }),
                allow_tools: req.tools != ToolAccess::None,
                cache_key: Some(cache_key(req)),
            };
            let steps = max_steps(req);
            let mut seen: Vec<u64> = Vec::new();
            let mut nudged = false;
            let mut fixes = 0;

            // Extra rounds beyond the step budget: a nudge and the fixes.
            for step in 0..=steps + 1 + MAX_FIXES {
                if req.ctx.cancel.is_cancelled() {
                    bail!("stopped");
                }
                if step == steps {
                    request.allow_tools = false;
                    request.messages.push(Message::User(
                        "You have used all available tool steps. Answer now with the requested output."
                            .into(),
                    ));
                }
                // A fix isn't streamed: the user already saw the answer, and
                // the gate shows the corrected one if it differs.
                let parts =
                    complete_with_retry(llm.as_ref(), &request, req, &sink, fixes == 0).await?;
                request.messages.push(Message::Assistant(parts.clone()));

                let calls: Vec<_> = parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::ToolCall(c) => Some(c.clone()),
                        _ => None,
                    })
                    .collect();
                if calls.is_empty() || !request.allow_tools {
                    let text = text_of(&parts);
                    if text.trim().is_empty() {
                        // Once: models sometimes end on thinking alone.
                        if nudged || step >= steps {
                            bail!("the model returned no answer");
                        }
                        nudged = true;
                        request.allow_tools = false;
                        request.messages.push(Message::User(
                            "Your last reply was empty. Give your final answer now, in the requested format."
                                .into(),
                        ));
                        continue;
                    }
                    if let Some(check) = check
                        && fixes < MAX_FIXES
                        && let Err(report) = check(&text)
                    {
                        fixes += 1;
                        tracing::info!(fixes, "final answer failed the check; fixing in place");
                        sink(StreamEvent::Thought(
                            "\n\nThe answer didn't fit the format; fixing it.\n".into(),
                        ));
                        request.allow_tools = false;
                        request.messages.push(Message::User(format!(
                            "Your final message failed the check:\n{report}\n\nFix every violation and reply with ONLY the corrected output. Keep your content; don't redo the work."
                        )));
                        continue;
                    }
                    return Ok(text);
                }

                // Reads run together; edits and commands run in order, since
                // they depend on each other. A run of reads between them
                // still goes at once.
                let mut results = Vec::new();
                let mut i = 0;
                while i < calls.len() {
                    if read_only_tool(&calls[i].name) || req.tools != ToolAccess::Full {
                        let mut j = i;
                        while j < calls.len()
                            && (read_only_tool(&calls[j].name) || req.tools != ToolAccess::Full)
                        {
                            j += 1;
                        }
                        results.extend(
                            join_all(
                                calls[i..j]
                                    .iter()
                                    .map(|c| run_call(c, &run, &root, req, &sink)),
                            )
                            .await,
                        );
                        i = j;
                    } else {
                        results.push(run_call(&calls[i], &run, &root, req, &sink).await);
                        i += 1;
                    }
                }

                // The same call with the same result again: say so, and stop
                // the tools if it keeps happening.
                let mut repeats = 0;
                for (call, result) in calls.iter().zip(results.iter_mut()) {
                    let print = fingerprint(call, result);
                    let before = seen.iter().rev().take(12).filter(|p| **p == print).count();
                    seen.push(print);
                    if before >= 2 {
                        repeats = repeats.max(before);
                        result.content.push_str(
                            "\n\n(You have made this exact call with this exact result before. Use what you have, or try something different.)",
                        );
                    }
                }
                if let Some(last) = results.last_mut() {
                    let left = steps.saturating_sub(step + 1);
                    if left == 3 {
                        last.content
                            .push_str("\n\n(3 tool steps left: start converging on your answer.)");
                    }
                }
                request.messages.push(Message::ToolResults(results));
                if repeats >= 4 {
                    request.allow_tools = false;
                    request.messages.push(Message::User(
                        "You are repeating the same tool calls. Stop using tools and answer now with what you have, in the requested format.".into(),
                    ));
                }
                prune(&mut request.messages);
            }
            bail!("the model kept going past its step budget")
        }
    }
}

/// Runs one tool call, showing it in the thread: diffs for file changes, a
/// live terminal (after the user allows it) for commands.
async fn run_call(
    call: &crate::llm::ToolCall,
    run: &str,
    root: &std::path::Path,
    req: &EngineRequest,
    sink: &StreamSink,
) -> ToolResult {
    let id = format!("{run}-{}", call.id);
    sink(StreamEvent::ToolStart(describe_tool(
        id.clone(),
        &call.name,
        &call.args,
        &req.cwd,
    )));
    let full = req.tools == ToolAccess::Full;
    let env = ToolEnv {
        root,
        attached: &req.files,
    };
    let mut images = Vec::new();
    let invalid = call
        .args
        .get(crate::llm::INVALID_ARGS)
        .and_then(serde_json::Value::as_str);
    let outcome = match call.name.as_str() {
        _ if invalid.is_some() => Err(format!(
            "The arguments for `{}` weren't valid JSON: {}. If they were cut off by the output limit (a very long `write`), split the work into smaller steps; otherwise send the call again with valid JSON.",
            call.name,
            invalid.unwrap_or_default()
        )),
        "edit" | "write" if full => change_file(call, &env, req).await.map(|(msg, c)| {
            sink(StreamEvent::ToolUpdate {
                id: id.clone(),
                content: ToolContent::Diff {
                    path: c.path,
                    old: c.old,
                    new: c.new,
                },
            });
            msg
        }),
        "bash" if full => run_command(&id, &call.args, req, sink).await,
        // Reads walk folders and run pdftotext: off the async workers, so
        // several really run at once.
        name => {
            let (name, args) = (name.to_string(), call.args.clone());
            let (root, attached) = (root.to_path_buf(), req.files.clone());
            let joined = tokio::task::spawn_blocking(move || {
                let env = ToolEnv {
                    root: &root,
                    attached: &attached,
                };
                if name == "view" {
                    tools::view(&args, &env)
                } else {
                    tools::run(&name, &args, &env).map(|t| (t, Vec::new()))
                }
            })
            .await;
            match joined {
                Ok(Ok((text, found))) => {
                    images = found;
                    Ok(text)
                }
                Ok(Err(e)) => Err(e),
                Err(e) => {
                    crate::telemetry::emit(pathfinder_schema::Event::Error {
                        code: pathfinder_schema::ErrorCode::Tool,
                    });
                    Err(format!("the tool failed: {e}"))
                }
            }
        }
    };
    sink(StreamEvent::ToolEnd {
        id,
        ok: outcome.is_ok(),
    });
    let (content, is_error) = match outcome {
        Ok(out) => (cap(out), false),
        Err(err) => (err, true),
    };
    ToolResult {
        call_id: call.id.clone(),
        content,
        is_error,
        images,
    }
}

/// `edit` and `write`. Through the client when it offers file access, so
/// the change shows up in its review of the agent's edits; here otherwise.
async fn change_file(
    call: &crate::llm::ToolCall,
    env: &ToolEnv<'_>,
    req: &EngineRequest,
) -> Result<(String, tools::Change), String> {
    let (path, full) = tools::change_target(&call.args, env)?;
    let host = req.ctx.host.as_deref();
    let old = match host.and_then(|h| h.read_file(&full)) {
        // A file the client can't open is read here, which also tells a
        // missing file from an unreadable one.
        Some(read) => match read.await {
            Ok(text) => Some(text),
            Err(_) => tools::read_existing(&full, &path)?,
        },
        None => tools::read_existing(&full, &path)?,
    };
    let (message, new) = tools::apply_change(&call.name, &call.args, &path, old.as_deref())?;
    match host.and_then(|h| h.write_file(&full, &new)) {
        Some(write) => {
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("can't create folders for {path}: {e}"))?;
            }
            write
                .await
                .map_err(|e| format!("can't write {path}: {e}"))?;
        }
        None => tools::write_here(&full, &path, &new)?,
    }
    Ok((
        message,
        tools::Change {
            path: full,
            old,
            new,
        },
    ))
}

/// `bash`: asks the user, then runs in the client's terminal when it has
/// one, else here.
async fn run_command(
    id: &str,
    args: &serde_json::Value,
    req: &EngineRequest,
    sink: &StreamSink,
) -> Result<String, String> {
    let command = args
        .get("command")
        .and_then(serde_json::Value::as_str)
        .filter(|c| !c.trim().is_empty())
        .ok_or("`command` is required")?;
    let timeout = Duration::from_secs(
        args.get("timeout_secs")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(120)
            .clamp(1, 600),
    );
    let flag = |k: &str| {
        args.get(k)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    let (network, outside) = (flag("network"), flag("outside_sandbox"));
    let cancel = &req.ctx.cancel;
    // In the sandbox unless the model asked to leave it; no sandbox on this
    // machine means every command asks.
    let jail = if outside { None } else { sandbox::available() };
    let policy = sandbox::Policy {
        workspace: req.cwd.clone(),
        network,
    };
    let ask = jail.is_none() || network || crate::config::Config::load().ask_every_command;
    if let Some(host) = &req.ctx.host
        && ask
    {
        // "Always allow" covers later commands with the same program and
        // the same kind of access.
        let program = command.split_whitespace().next().unwrap_or(command);
        let key = match (jail.is_some(), network) {
            (false, _) => format!("unsandboxed:{program}"),
            (true, true) => format!("network:{program}"),
            (true, false) => program.to_string(),
        };
        if host.ask(id, &key).await == Permission::Denied {
            return Err("The user declined to run this command.".into());
        }
    }
    let started = std::time::Instant::now();
    let merged = format!("{{ {command}\n}} 2>&1");
    let local_argv = match &jail {
        Some(kind) => sandbox::wrap(kind, &policy, &merged),
        None => vec!["/bin/sh".into(), "-c".into(), merged],
    };
    let terminal_line = match &jail {
        Some(kind) => sandbox::shell_join(&sandbox::wrap(kind, &policy, command)),
        None => command.to_string(),
    };
    let terminal = req
        .ctx
        .host
        .as_ref()
        .and_then(|h| h.run_in_terminal(id, &terminal_line, &req.cwd, timeout, cancel));
    let output = match terminal {
        Some(run) => run.await.map_err(|e| e.to_string())?,
        None => run_local(id, command, local_argv, &req.cwd, timeout, cancel, sink).await?,
    };
    let took = started.elapsed().as_secs_f32();
    let status = match output.exit_code {
        Some(0) => format!("Exit code 0 ({took:.1}s)"),
        Some(code) => format!("Exit code {code} ({took:.1}s)"),
        None if cancel.is_cancelled() => "Stopped by the user".to_string(),
        None => format!(
            "Killed after {}s. If it needs longer and isn't waiting for input, run it again with a larger `timeout_secs`; if it waits for input, pass the input or a non-interactive flag.",
            timeout.as_secs()
        ),
    };
    let body = output.output.trim_end();
    let mut text = if body.is_empty() {
        format!("{status} (no output)")
    } else {
        format!("{status}\n{body}")
    };
    if output.truncated {
        text.push_str("\n(output truncated)");
    }
    if jail.is_some()
        && output.exit_code != Some(0)
        && let Some(hint) = sandbox::denial_hint(&output.output, network)
    {
        text.push_str(&hint);
    }
    match output.exit_code {
        Some(0) => Ok(text),
        _ => Err(text),
    }
}

/// Runs a command here and shows its output in the tool call afterwards.
/// Output and errors come interleaved as the command wrote them, and what
/// arrived before a timeout or a stop is kept.
async fn run_local(
    id: &str,
    command: &str,
    argv: Vec<String>,
    cwd: &std::path::Path,
    timeout: Duration,
    cancel: &tokio_util::sync::CancellationToken,
    sink: &StreamSink,
) -> Result<CommandOutput, String> {
    use tokio::io::AsyncReadExt;
    let mut child = tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("can't start the command: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no output pipe")?;
    // Enough to keep the start and the end; `cap` cuts the middle.
    const KEEP: usize = 4 * MAX_TOOL_OUTPUT;
    let mut head: Vec<u8> = Vec::new();
    let mut tail: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
    let mut dropped = false;
    let mut buf = [0u8; 8192];
    let deadline = tokio::time::Instant::now() + timeout;
    let mut finished = true;
    loop {
        let n = tokio::select! {
            r = stdout.read(&mut buf) => r.unwrap_or(0),
            _ = tokio::time::sleep_until(deadline) => { finished = false; break }
            _ = cancel.cancelled() => { finished = false; break }
        };
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if head.len() < KEEP / 2 {
                head.push(b);
            } else {
                tail.push_back(b);
                if tail.len() > KEEP / 2 {
                    tail.pop_front();
                    dropped = true;
                }
            }
        }
    }
    let exit_code = if finished {
        child.wait().await.ok().and_then(|s| s.code())
    } else {
        child.kill().await.ok();
        None
    };
    let mut text = String::from_utf8_lossy(&head).into_owned();
    if dropped {
        text.push_str("\n… (more output cut here) …\n");
    }
    text.push_str(&String::from_utf8_lossy(tail.make_contiguous()));
    let truncated = dropped || text.len() > MAX_TOOL_OUTPUT;
    let text = cap(text);
    sink(StreamEvent::ToolUpdate {
        id: id.to_string(),
        content: ToolContent::Text(format!("```\n$ {command}\n{}\n```", text.trim_end())),
    });
    Ok(CommandOutput {
        output: text,
        exit_code,
        truncated,
    })
}

/// One model call, retried on transient errors while nothing has streamed.
async fn complete_with_retry(
    llm: &dyn Llm,
    request: &LlmRequest,
    req: &EngineRequest,
    sink: &StreamSink,
    stream: bool,
) -> Result<Vec<Part>> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        // Only shown answer text makes a retry unsafe: it would repeat in
        // the thread. Thinking may show twice.
        let emitted = AtomicBool::new(false);
        let on_event = |event: LlmEvent| match event {
            LlmEvent::Reasoning(t) => sink(StreamEvent::Thought(t)),
            LlmEvent::Text(t) if req.stream_text && stream => {
                emitted.store(true, Ordering::Relaxed);
                sink(StreamEvent::TextDelta(t))
            }
            LlmEvent::Text(_) => {}
        };
        crate::telemetry::count_model_call();
        match llm.complete(request, &on_event, &req.ctx.cancel).await {
            Ok(parts) => return Ok(parts),
            Err(err)
                if err.retryable()
                    && attempt < MAX_ATTEMPTS
                    && !emitted.load(Ordering::Relaxed) =>
            {
                // 1, 2, 4, 8 s with up to a quarter more, so parallel
                // runs don't retry in lockstep; the server's word first.
                let base = Duration::from_secs(1 << (attempt - 1));
                let jitter = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.subsec_nanos() as u64)
                    % (base.as_millis() as u64 / 4 + 1);
                let wait = err
                    .retry_after()
                    .unwrap_or(base + Duration::from_millis(jitter))
                    .min(Duration::from_secs(60));
                tracing::warn!("{err}; retrying in {}s", wait.as_secs());
                crate::telemetry::count_retry();
                sink(StreamEvent::Thought(format!(
                    "\n\nThe provider didn't answer; retrying in {}s.\n",
                    wait.as_secs()
                )));
                tokio::time::sleep(wait).await;
            }
            Err(LlmError::Cancelled) => bail!("cancelled"),
            // Counted once, by the turn (`telemetry::error_code`).
            Err(err) => return Err(anyhow!(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::Mutex;

    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::engine::TierModels;
    use crate::llm::ToolCall;

    /// Replays scripted answers and records each request.
    struct ScriptedLlm {
        answers: Mutex<VecDeque<Result<Vec<Part>, LlmError>>>,
        seen: Mutex<Vec<LlmRequest>>,
    }

    impl Llm for ScriptedLlm {
        fn complete<'a>(
            &'a self,
            req: &'a LlmRequest,
            events: crate::llm::EventSink<'a>,
            _cancel: &'a CancellationToken,
        ) -> BoxFuture<'a, Result<Vec<Part>, LlmError>> {
            Box::pin(async move {
                self.seen.lock().unwrap().push(req.clone());
                let answer = self
                    .answers
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected call");
                if let Ok(parts) = &answer {
                    let text = text_of(parts);
                    if !text.is_empty() {
                        events(LlmEvent::Text(text));
                    }
                }
                answer
            })
        }
    }

    fn scripted(answers: Vec<Result<Vec<Part>, LlmError>>) -> (ApiEngine, Arc<ScriptedLlm>) {
        let llm = Arc::new(ScriptedLlm {
            answers: Mutex::new(answers.into()),
            seen: Mutex::new(Vec::new()),
        });
        let shared = llm.clone();
        (
            ApiEngine::new(Arc::new(move |_: &ModelEntry| {
                Ok(shared.clone() as Arc<dyn Llm>)
            })),
            llm,
        )
    }

    fn request(cwd: PathBuf, tools: ToolAccess) -> EngineRequest {
        let entry = ModelEntry {
            id: "m".into(),
            label: "m".into(),
            route: Route::Anthropic,
            model: "claude-sonnet-5-5".into(),
            effort: Some("low".into()),
            endpoint: None,
        };
        EngineRequest {
            tier: EngineTier::System1,
            tools,
            cwd,
            system_prompt: "Answer in JSON.".into(),
            prompt: "What is the crate name?".into(),
            files: vec![],
            stream_text: true,
            models: TierModels {
                system1: entry.clone(),
                system2: entry,
            },
            ctx: crate::engine::turn_ctx(),
        }
    }

    fn workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-agent-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        dir.canonicalize().unwrap()
    }

    fn recorder() -> (StreamSink, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let record = log.clone();
        let sink: StreamSink = Arc::new(move |ev| {
            let line = match ev {
                StreamEvent::Thought(_) => return,
                StreamEvent::TextDelta(t) => format!("text {t}"),
                StreamEvent::ToolStart(t) => format!("start {}", t.title),
                StreamEvent::ToolUpdate { .. } => "update".into(),
                StreamEvent::ToolEnd { ok, .. } => format!("end {ok}"),
            };
            record.lock().unwrap().push(line);
        });
        (sink, log)
    }

    fn call(id: &str, name: &str, args: serde_json::Value) -> Part {
        Part::ToolCall(ToolCall {
            id: id.into(),
            name: name.into(),
            args,
        })
    }

    #[tokio::test]
    async fn runs_tools_then_returns_the_final_text() {
        let ws = workspace();
        let thinking = Part::Opaque(json!({"type": "thinking", "signature": "s"}));
        let (engine, llm) = scripted(vec![
            Ok(vec![
                thinking.clone(),
                call("1", "read", json!({"path": "Cargo.toml"})),
                call("2", "read", json!({"path": "../escape"})),
            ]),
            Ok(vec![Part::Text(r#"{"reply":"demo"}"#.into())]),
        ]);
        let (sink, log) = recorder();
        let out = engine
            .run_text(&request(ws.clone(), ToolAccess::ReadOnly), sink)
            .await
            .unwrap();
        assert_eq!(out, r#"{"reply":"demo"}"#);

        let log = log.lock().unwrap();
        assert!(log.contains(&"start Read Cargo.toml".to_string()));
        assert!(log.contains(&"end true".to_string()));
        assert!(log.contains(&"end false".to_string()));

        let seen = llm.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        // Thinking replayed unchanged; both results sent back in one message.
        assert_eq!(
            seen[1].messages[1],
            Message::Assistant(vec![
                thinking,
                call("1", "read", json!({"path": "Cargo.toml"})),
                call("2", "read", json!({"path": "../escape"})),
            ])
        );
        let Message::ToolResults(results) = &seen[1].messages[2] else {
            panic!()
        };
        assert!(results[0].content.contains("name = \"demo\""));
        assert!(!results[0].is_error);
        assert!(results[1].is_error);
        fs::remove_dir_all(ws).unwrap();
    }

    #[tokio::test]
    async fn step_budget_forces_an_answer_without_tools() {
        let ws = workspace();
        let mut answers: Vec<_> = (0..16)
            .map(|i| {
                Ok(vec![call(
                    &i.to_string(),
                    "glob",
                    json!({"pattern": format!("*{i}*")}),
                )])
            })
            .collect();
        answers.push(Ok(vec![Part::Text("final".into())]));
        let (engine, llm) = scripted(answers);
        let out = engine
            .run_text(&request(ws.clone(), ToolAccess::ReadOnly), recorder().0)
            .await
            .unwrap();
        assert_eq!(out, "final");
        let seen = llm.seen.lock().unwrap();
        assert_eq!(seen.len(), 17);
        assert!(!seen[16].allow_tools);
        fs::remove_dir_all(ws).unwrap();
    }

    #[tokio::test]
    async fn retries_transient_errors_but_not_others() {
        let ws = workspace();
        let (engine, llm) = scripted(vec![
            Err(LlmError::Overloaded("busy".into())),
            Ok(vec![Part::Text("ok".into())]),
        ]);
        let out = engine
            .run_text(&request(ws.clone(), ToolAccess::None), recorder().0)
            .await
            .unwrap();
        assert_eq!(out, "ok");
        assert_eq!(llm.seen.lock().unwrap().len(), 2);
        assert!(llm.seen.lock().unwrap()[0].tools.is_empty());

        let (engine, _) = scripted(vec![Err(LlmError::UsageLimit("limit".into()))]);
        let err = engine
            .run_text(&request(ws.clone(), ToolAccess::None), recorder().0)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("usage limit"));

        fs::remove_dir_all(ws).unwrap();
    }

    /// Answers every permission prompt the same way; no terminals.
    struct FakeHost(Permission);

    impl crate::engine::Host for FakeHost {
        fn ask<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Permission> {
            Box::pin(async move { self.0 })
        }
        fn run_in_terminal<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
            _: &'a std::path::Path,
            _: Duration,
            _: &'a CancellationToken,
        ) -> Option<BoxFuture<'a, Result<CommandOutput>>> {
            None
        }
    }

    #[tokio::test]
    async fn agent_work_edits_files_and_runs_allowed_commands() {
        let ws = workspace();
        let script = |network: bool| {
            vec![
                Ok(vec![
                    Part::Text("Renaming the crate.".into()),
                    call(
                        "e1",
                        "edit",
                        json!({"path": "Cargo.toml", "old_string": "demo", "new_string": "neuron"}),
                    ),
                ]),
                Ok(vec![call(
                    "b1",
                    "bash",
                    json!({"command": "grep -c neuron Cargo.toml", "network": network, "justification": "test"}),
                )]),
                Ok(vec![Part::Text("Renamed it.".into())]),
            ]
        };
        let (engine, llm) = scripted(script(false));
        let mut req = request(ws.clone(), ToolAccess::Full);
        req.ctx.host = Some(Arc::new(FakeHost(Permission::Once)));
        let (sink, log) = recorder();
        assert_eq!(engine.run_text(&req, sink).await.unwrap(), "Renamed it.");
        assert!(
            fs::read_to_string(ws.join("Cargo.toml"))
                .unwrap()
                .contains("neuron")
        );
        let log = log.lock().unwrap().clone();
        assert_eq!(log[0], "text Renaming the crate.");
        assert_eq!(log[1..4], ["start Edit Cargo.toml", "update", "end true"]);
        assert_eq!(log[4], "start Run `grep -c neuron Cargo.toml`");
        {
            let seen = llm.seen.lock().unwrap();
            let Message::ToolResults(results) = seen[2].messages.last().unwrap() else {
                panic!("expected tool results");
            };
            assert!(
                results[0].content.starts_with("Exit code 0 (")
                    && results[0].content.ends_with("\n1"),
                "{}",
                results[0].content
            );
        }

        // Sandboxed, it runs without asking: a "no" isn't even consulted.
        if sandbox::available().is_some() {
            fs::write(ws.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
            let (engine, llm) = scripted(script(false));
            req.ctx.host = Some(Arc::new(FakeHost(Permission::Denied)));
            engine.run_text(&req, recorder().0).await.unwrap();
            let seen = llm.seen.lock().unwrap();
            let Message::ToolResults(results) = seen[2].messages.last().unwrap() else {
                panic!("expected tool results");
            };
            assert!(!results[0].is_error, "{}", results[0].content);
        }

        // Network asks; declined, the command doesn't run and the model is
        // told so.
        fs::write(ws.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        let (engine, llm) = scripted(script(true));
        req.ctx.host = Some(Arc::new(FakeHost(Permission::Denied)));
        engine.run_text(&req, recorder().0).await.unwrap();
        let seen = llm.seen.lock().unwrap();
        let Message::ToolResults(results) = seen[2].messages.last().unwrap() else {
            panic!("expected tool results");
        };
        assert!(results[0].is_error && results[0].content.contains("declined"));
        fs::remove_dir_all(ws).unwrap();
    }

    /// A client with file access: records writes instead of touching disk.
    #[derive(Default)]
    struct FileHost(Mutex<Vec<(PathBuf, String)>>);

    impl crate::engine::Host for FileHost {
        fn ask<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Permission> {
            Box::pin(async { Permission::Once })
        }
        fn run_in_terminal<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
            _: &'a std::path::Path,
            _: Duration,
            _: &'a CancellationToken,
        ) -> Option<BoxFuture<'a, Result<CommandOutput>>> {
            None
        }
        fn read_file<'a>(
            &'a self,
            path: &'a std::path::Path,
        ) -> Option<BoxFuture<'a, Result<String>>> {
            Some(Box::pin(async move { Ok(fs::read_to_string(path)?) }))
        }
        fn write_file<'a>(
            &'a self,
            path: &'a std::path::Path,
            content: &'a str,
        ) -> Option<BoxFuture<'a, Result<()>>> {
            self.0
                .lock()
                .unwrap()
                .push((path.to_path_buf(), content.to_string()));
            Some(Box::pin(async { Ok(()) }))
        }
    }

    #[tokio::test]
    async fn edits_go_through_the_client_when_it_offers_files() {
        let ws = workspace();
        let (engine, _) = scripted(vec![
            Ok(vec![call(
                "e1",
                "edit",
                json!({"path": "Cargo.toml", "old_string": "demo", "new_string": "neuron"}),
            )]),
            Ok(vec![Part::Text("Done.".into())]),
        ]);
        let host = Arc::new(FileHost::default());
        let mut req = request(ws.clone(), ToolAccess::Full);
        req.ctx.host = Some(host.clone());
        engine.run_text(&req, recorder().0).await.unwrap();
        let writes = host.0.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert!(writes[0].0.ends_with("Cargo.toml") && writes[0].1.contains("neuron"));
        // The client applies it; Pathfinder didn't write the file itself.
        assert!(
            fs::read_to_string(ws.join("Cargo.toml"))
                .unwrap()
                .contains("demo")
        );
        fs::remove_dir_all(ws).unwrap();
    }

    #[test]
    fn cap_keeps_the_start_and_the_end() {
        let long = format!("START\n{}\nERROR at the end", "x\n".repeat(60_000));
        let out = cap(long);
        assert!(out.len() < MAX_TOOL_OUTPUT + 200);
        assert!(out.starts_with("START") && out.ends_with("ERROR at the end"));
        assert!(out.contains("cut from the middle"));
    }

    #[tokio::test]
    async fn broken_arguments_and_repeats_go_back_to_the_model() {
        let ws = workspace();
        let bad = call(
            "1",
            "write",
            json!({crate::llm::INVALID_ARGS: "EOF while parsing"}),
        );
        let ls = || Ok(vec![call("l", "ls", json!({}))]);
        let (engine, llm) = scripted(vec![
            Ok(vec![bad]),
            ls(),
            ls(),
            ls(),
            ls(),
            ls(),
            Ok(vec![Part::Text("done".into())]),
        ]);
        let mut req = request(ws.clone(), ToolAccess::Full);
        req.ctx.host = Some(Arc::new(FakeHost(Permission::Once)));
        let out = engine.run_text(&req, recorder().0).await.unwrap();
        assert_eq!(out, "done");
        let seen = llm.seen.lock().unwrap();
        let Message::ToolResults(first) = &seen[1].messages[2] else {
            panic!()
        };
        assert!(first[0].is_error && first[0].content.contains("weren't valid JSON"));
        // The third identical `ls` is flagged; the fifth stops the tools.
        let Message::ToolResults(third) = &seen[4].messages[8] else {
            panic!()
        };
        assert!(third[0].content.contains("exact call"));
        assert!(!seen[6].allow_tools);
        fs::remove_dir_all(ws).unwrap();
    }

    #[tokio::test]
    async fn an_empty_answer_gets_one_nudge() {
        let ws = workspace();
        let (engine, llm) = scripted(vec![
            Ok(vec![Part::Opaque(
                json!({"type": "thinking", "signature": "s"}),
            )]),
            Ok(vec![Part::Text("answer".into())]),
        ]);
        let out = engine
            .run_text(&request(ws.clone(), ToolAccess::ReadOnly), recorder().0)
            .await
            .unwrap();
        assert_eq!(out, "answer");
        assert!(!llm.seen.lock().unwrap()[1].allow_tools);
        fs::remove_dir_all(ws).unwrap();
    }

    #[test]
    fn old_tool_output_is_cleared_past_the_budget() {
        let result = |s: &str| ToolResult {
            call_id: "c".into(),
            content: s.repeat(100 * 1024),
            is_error: false,
            images: vec![],
        };
        let mut messages: Vec<Message> = (0..5)
            .map(|_| Message::ToolResults(vec![result("a")]))
            .collect();
        prune(&mut messages);
        let cleared: Vec<bool> = messages
            .iter()
            .map(|m| matches!(m, Message::ToolResults(r) if r[0].content.starts_with("[output cleared")))
            .collect();
        assert_eq!(cleared, [true, true, false, false, false]);
    }

    #[tokio::test]
    async fn command_output_survives_a_timeout() {
        let ws = workspace();
        let cmd = "echo before; echo oops >&2; sleep 5";
        let out = run_local(
            "t",
            cmd,
            vec!["/bin/sh".into(), "-c".into(), format!("{{ {cmd}\n}} 2>&1")],
            &ws,
            Duration::from_millis(500),
            &CancellationToken::new(),
            &recorder().0,
        )
        .await
        .unwrap();
        assert_eq!(out.exit_code, None);
        assert!(
            out.output.contains("before") && out.output.contains("oops"),
            "{}",
            out.output
        );
        fs::remove_dir_all(ws).unwrap();
    }

    #[tokio::test]
    async fn a_failing_answer_is_fixed_in_the_same_conversation() {
        let ws = workspace();
        let (engine, llm) = scripted(vec![
            Ok(vec![call("1", "read", json!({"path": "Cargo.toml"}))]),
            Ok(vec![Part::Text("The crate is demo.".into())]),
            Ok(vec![Part::Text(r#"{"reply":"demo"}"#.into())]),
        ]);
        let check = |raw: &str| -> std::result::Result<(), String> {
            serde_json::from_str::<serde_json::Value>(raw)
                .map(|_| ())
                .map_err(|e| format!("- invalid_json: {e}"))
        };
        let (sink, log) = recorder();
        let out = engine
            .run_checked(&request(ws.clone(), ToolAccess::ReadOnly), sink, &check)
            .await
            .unwrap();
        assert_eq!(out, r#"{"reply":"demo"}"#);
        let seen = llm.seen.lock().unwrap();
        // The fix sees the earlier read and the violations, and can't call tools.
        let fix = &seen[2];
        assert!(!fix.allow_tools);
        assert!(matches!(&fix.messages[2], Message::ToolResults(_)));
        let Message::User(note) = fix.messages.last().unwrap() else {
            panic!()
        };
        assert!(note.contains("invalid_json"));
        // The fixed answer isn't streamed a second time.
        let texts = log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with("text"))
            .count();
        assert_eq!(texts, 1);
        fs::remove_dir_all(ws).unwrap();
    }
}
