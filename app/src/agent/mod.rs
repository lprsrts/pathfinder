//! Pathfinder's own agent loop: calls a model API, runs tools, repeats until
//! the model answers. Sits behind the `Engine` trait, so the theory loop in
//! runtime.rs doesn't know or care which route ran.

pub mod confine;
pub mod sandbox;
pub mod shell;
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
    // In-process, with the OS time zone: Windows has no `date` program.
    // Omit HH:MM so the system prompt prefix stays cacheable across turns.
    let date =
        jiff::fmt::strtime::format("%Y-%m-%d (%A)", &jiff::ZonedDateTime::now()).unwrap_or_default();
    let access = match req.tools {
        ToolAccess::None => "none: answer from what you are given",
        ToolAccess::ReadOnly => "read-only: you can read and search, not change anything",
        ToolAccess::Full => {
            "full: you can edit files and run commands (the user approves commands)"
        }
    };
    // The tool is called `bash` everywhere; where that isn't sh in a
    // sandbox, say what it really runs.
    let shell_line = if req.tools == ToolAccess::Full && cfg!(windows) {
        let shell = shell::current();
        let note = match shell {
            shell::Shell::PowerShell(_) => {
                "commands are PowerShell, not POSIX sh; there is no sandbox here, so the user approves every command"
            }
            _ => "there is no sandbox here, so the user approves every command",
        };
        format!("\nShell: {} ({note})", shell.name())
    } else {
        String::new()
    };
    let mut env = format!(
        "\n\n<environment>\nWorkspace: {}\nToday: {date}\nPlatform: {}{shell_line}\nTool access: {access}\n</environment>",
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

fn message_size(m: &Message) -> usize {
    match m {
        Message::User(t) => t.len(),
        Message::UserWith(t, files) => {
            t.len() + files.iter().map(|f| f.data.len()).sum::<usize>()
        }
        Message::Assistant(parts) => parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => t.len(),
                Part::ToolCall(c) => c.name.len() + c.args.to_string().len(),
                Part::Opaque(_) => 0,
            })
            .sum(),
        Message::ToolResults(rs) => rs
            .iter()
            .map(|r| r.content.len() + r.images.iter().map(|i| i.data.len()).sum::<usize>())
            .sum(),
    }
}

/// Clears old tool results oldest-first once context adds up past
/// `PRUNE_ABOVE`, keeping the last `KEEP_RECENT` tool-result steps and
/// stopping as soon as total size is back under budget. Returns the message
/// indices whose results were cleared.
fn prune(messages: &mut [Message]) -> Vec<usize> {
    let mut total: usize = messages.iter().map(message_size).sum();
    if total <= PRUNE_ABOVE {
        return Vec::new();
    }
    let steps: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| matches!(m, Message::ToolResults(_)))
        .map(|(i, _)| i)
        .collect();
    let mut cleared = Vec::new();
    let eligible = steps.len().saturating_sub(KEEP_RECENT);
    for &i in &steps[..eligible] {
        if total <= PRUNE_ABOVE {
            break;
        }
        if let Message::ToolResults(rs) = &mut messages[i] {
            let mut any = false;
            for r in rs {
                if r.content.len() > 400 || !r.images.is_empty() {
                    let before = r.content.len()
                        + r.images.iter().map(|img| img.data.len()).sum::<usize>();
                    let replacement =
                        "[output cleared to save room; re-read only if you still need it]";
                    r.content = replacement.into();
                    r.images.clear();
                    total = total.saturating_sub(before) + replacement.len();
                    any = true;
                }
            }
            if any {
                cleared.push(i);
            }
        }
    }
    cleared
}

/// Canonical JSON serialization (sorted keys) with normalized `read`/`view`
/// arguments (`path` trimmed, single-page `"24-24"` collapsed to `"24"`).
fn canonical_call_key(call: &crate::llm::ToolCall) -> String {
    fn write_canonical(v: &serde_json::Value, out: &mut String) {
        match v {
            serde_json::Value::Object(map) => {
                out.push('{');
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(k).unwrap_or_default());
                    out.push(':');
                    write_canonical(&map[*k], out);
                }
                out.push('}');
            }
            serde_json::Value::Array(arr) => {
                out.push('[');
                for (i, item) in arr.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_canonical(item, out);
                }
                out.push(']');
            }
            other => {
                out.push_str(&other.to_string());
            }
        }
    }

    let mut args = call.args.clone();
    if matches!(call.name.as_str(), "read" | "view")
        && let Some(obj) = args.as_object_mut()
    {
        if let Some(p) = obj.get("path").and_then(serde_json::Value::as_str) {
            let trimmed = p.trim();
            let norm = trimmed.strip_prefix("./").unwrap_or(trimmed);
            obj.insert("path".into(), serde_json::Value::String(norm.to_string()));
        }
        if let Some(pages) = obj.get("pages").and_then(serde_json::Value::as_str) {
            let trimmed = pages.trim();
            let norm = if let Some((a, b)) = trimmed.split_once('-') {
                let (a, b) = (a.trim(), b.trim());
                if !a.is_empty() && a == b {
                    a.to_string()
                } else {
                    format!("{a}-{b}")
                }
            } else {
                trimmed.to_string()
            };
            obj.insert("pages".into(), serde_json::Value::String(norm));
        }
    }
    let mut out = format!("{}:", call.name);
    write_canonical(&args, &mut out);
    out
}

/// A tool call's fingerprint with its result, to notice repetition.
fn fingerprint(call: &crate::llm::ToolCall, result: &ToolResult) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    canonical_call_key(call).hash(&mut h);
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
            let mut seen_prints: std::collections::HashMap<u64, usize> =
                std::collections::HashMap::new();
            let mut active_reads: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
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

                // Reads run together (deduplicating identical reads that are
                // already in context or repeated in the same step); edits and
                // commands run in order and invalidate active read tracking.
                let mut results: Vec<Option<ToolResult>> = vec![None; calls.len()];
                let mut batch_reads: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let mut repeats: usize = 0;
                let mut executed_read: Vec<bool> = vec![false; calls.len()];
                let mut i = 0;
                while i < calls.len() {
                    if read_only_tool(&calls[i].name) || req.tools != ToolAccess::Full {
                        let mut j = i;
                        while j < calls.len()
                            && (read_only_tool(&calls[j].name) || req.tools != ToolAccess::Full)
                        {
                            j += 1;
                        }
                        let mut to_run: Vec<usize> = Vec::new();
                        for idx in i..j {
                            let c = &calls[idx];
                            if read_only_tool(&c.name) {
                                let key = canonical_call_key(c);
                                let in_context = active_reads.contains_key(&key);
                                if in_context || !batch_reads.insert(key.clone()) {
                                    let count = seen_prints
                                        .entry(fingerprint(c, &ToolResult {
                                            call_id: String::new(),
                                            content: key.clone(),
                                            is_error: false,
                                            images: Vec::new(),
                                        }))
                                        .and_modify(|n| *n += 1)
                                        .or_insert(1);
                                    repeats = repeats.max(*count);
                                    let why = if in_context {
                                        "already read in this conversation and still in context above"
                                    } else {
                                        "already requested in this step"
                                    };
                                    results[idx] = Some(ToolResult {
                                        call_id: c.id.clone(),
                                        content: format!(
                                            "({why}; use that result instead of calling `{}` again with the same arguments.)",
                                            c.name
                                        ),
                                        is_error: false,
                                        images: Vec::new(),
                                    });
                                    continue;
                                }
                            }
                            executed_read[idx] = read_only_tool(&c.name);
                            to_run.push(idx);
                        }
                        let ran = join_all(
                            to_run
                                .iter()
                                .map(|&idx| run_call(&calls[idx], &run, &root, req, &sink)),
                        )
                        .await;
                        for (idx, res) in to_run.into_iter().zip(ran) {
                            results[idx] = Some(res);
                        }
                        i = j;
                    } else {
                        let res = run_call(&calls[i], &run, &root, req, &sink).await;
                        active_reads.clear();
                        batch_reads.clear();
                        results[i] = Some(res);
                        i += 1;
                    }
                }
                let mut results: Vec<ToolResult> =
                    results.into_iter().map(|r| r.expect("all calls resolved")).collect();

                // Record successful reads in `active_reads` and flag repeated
                // identical calls immediately on the second occurrence.
                let results_msg_idx = request.messages.len();
                for (idx, (call, result)) in calls.iter().zip(results.iter_mut()).enumerate() {
                    if executed_read[idx] {
                        if !result.is_error {
                            active_reads.insert(canonical_call_key(call), results_msg_idx);
                        }
                        let print = fingerprint(call, result);
                        let before = seen_prints.get(&print).copied().unwrap_or(0);
                        seen_prints.insert(print, before + 1);
                        if before >= 1 {
                            repeats = repeats.max(before);
                            result.content.push_str(
                                "\n\n(You have made this exact call with this exact result before. Use what you have, or try something different.)",
                            );
                        }
                    } else if !read_only_tool(&call.name) {
                        let print = fingerprint(call, result);
                        let before = seen_prints.get(&print).copied().unwrap_or(0);
                        seen_prints.insert(print, before + 1);
                        if before >= 1 {
                            repeats = repeats.max(before);
                            result.content.push_str(
                                "\n\n(You have made this exact call with this exact result before. Use what you have, or try something different.)",
                            );
                        }
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
                if repeats >= 2 {
                    request.allow_tools = false;
                    request.messages.push(Message::User(
                        "You are repeating the same tool calls. Stop using tools and answer now with what you have, in the requested format.".into(),
                    ));
                }
                let cleared = prune(&mut request.messages);
                if !cleared.is_empty() {
                    active_reads.retain(|_, idx| !cleared.contains(idx));
                }
            }
            bail!("the model kept going past its step budget")
        }
    }
}

/// Why a tool call failed, for its row: the error's first line, at most
/// 160 characters (anything after it, such as nearby lines, stays with the
/// model).
fn why_failed(err: &str) -> String {
    let first = err.lines().next().unwrap_or("").trim();
    match first.char_indices().nth(160) {
        Some((at, _)) => format!("{}…", &first[..at]),
        None => first.to_string(),
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
        "edit" | "write" if full => change_file(call, &env).map(|(msg, c)| {
            sink(StreamEvent::ToolUpdate {
                id: id.clone(),
                content: ToolContent::Diff {
                    path: crate::stream::shown_path(&c.path, root),
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
    // A failed call says why on its row, in one line (the model gets it
    // all): a wrong path or a bad pattern, not just a red mark.
    // A command shows its own output instead.
    if let Err(err) = &outcome
        && call.name != "bash"
    {
        sink(StreamEvent::ToolUpdate {
            id: id.clone(),
            content: ToolContent::Text(why_failed(err)),
        });
    }
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

/// `edit` and `write`.
fn change_file(
    call: &crate::llm::ToolCall,
    env: &ToolEnv<'_>,
) -> Result<(String, tools::Change), String> {
    let (path, full) = tools::change_target(&call.args, env)?;
    let old = tools::read_existing(&full, &path)?;
    let (message, new) = tools::apply_change(&call.name, &call.args, &path, old.as_deref())?;
    tools::write_here(&full, &path, &new)?;
    Ok((
        message,
        tools::Change {
            path: full,
            old,
            new,
        },
    ))
}

/// `bash`: asks the user when it must, then runs here.
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
    #[allow(unused_mut)]
    let mut policy = sandbox::Policy::new(req.cwd.clone(), network);
    // Unit tests run commands too: they mustn't make the folders of
    // whoever runs them.
    #[cfg(test)]
    policy.private.made.clear();
    let ask = jail.is_none() || network || crate::config::Config::load().ask_every_command;
    if ask {
        // Fail closed: a command that needs approval never runs unasked.
        let Some(host) = &req.ctx.host else {
            return Err("This command needs the user's approval (it runs without a sandbox, or with network), and nothing can ask them in this mode.".into());
        };
        let key = always_key(command, jail.is_some(), network);
        if host.ask(id, &key).await == Permission::Denied {
            return Err("The user declined to run this command.".into());
        }
    }
    let started = std::time::Instant::now();
    // Output and errors share one pipe (see `run_local`), so no shell-level
    // `2>&1` is needed and any shell's syntax works.
    let local_argv = match &jail {
        Some(kind) => sandbox::wrap(kind, &policy, command).map_err(|e| {
            format!("The sandbox couldn't be set up, so the command didn't run: {e}")
        })?,
        None => shell::current().argv(command),
    };
    let output = run_local(id, command, local_argv, &req.cwd, timeout, cancel, sink).await?;
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
    use std::io::Read as _;
    // One pipe for both output and errors, so they interleave as written,
    // on every OS and in every shell.
    let (mut reader, writer) =
        std::io::pipe().map_err(|e| format!("can't make an output pipe: {e}"))?;
    let errors = writer
        .try_clone()
        .map_err(|e| format!("can't make an output pipe: {e}"))?;
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(writer)
        .stderr(errors)
        .kill_on_drop(true);
    shell::prepare(&mut cmd);
    let spawned = cmd.spawn();
    // Closes our copies of the write end: the output ends when the
    // command's processes close theirs.
    drop(cmd);
    let mut child = spawned.map_err(|e| format!("can't start the command: {e}"))?;
    let mut tree = shell::Tree::of(&child);
    // The pipe is blocking; a thread (not the runtime's blocking pool, which
    // shutdown would wait on) reads it.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });
    // Enough to keep the start and the end; `cap` cuts the middle.
    const KEEP: usize = 4 * MAX_TOOL_OUTPUT;
    let mut head: Vec<u8> = Vec::new();
    let mut tail: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
    let mut dropped = false;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut finished = true;
    loop {
        let chunk = tokio::select! {
            c = rx.recv() => c,
            _ = tokio::time::sleep_until(deadline) => { finished = false; break }
            _ = cancel.cancelled() => { finished = false; break }
        };
        let Some(chunk) = chunk else {
            break;
        };
        for &b in &chunk {
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
        let code = child.wait().await.ok().and_then(|s| s.code());
        tree.release();
        code
    } else {
        // Everything it started, not only the shell.
        tree.kill();
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

/// What an "Always allow" answer covers. Inside the sandbox it covers later
/// commands with the same program and the same kind of access. Without a
/// sandbox nothing confines the command, so it covers only this exact
/// command line: allowing `ls` must not let `ls && rm -rf ~` through.
fn always_key(command: &str, sandboxed: bool, network: bool) -> String {
    let program = command.split_whitespace().next().unwrap_or(command);
    match (sandboxed, network) {
        (false, _) => format!("unsandboxed:{}", command.trim()),
        (true, true) => format!("network:{program}"),
        (true, false) => program.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn always_key_is_exact_without_a_sandbox() {
        use super::always_key;
        assert_eq!(
            always_key("ls -la", true, false),
            always_key("ls src", true, false)
        );
        assert_ne!(
            always_key("ls", false, false),
            always_key("ls && rm -rf ~", false, false)
        );
        assert_eq!(always_key("ls ", false, false), "unsandboxed:ls");
        assert_eq!(always_key("curl https://x.y", true, true), "network:curl");
    }

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
        dunce::canonicalize(dir).unwrap()
    }

    fn recorder() -> (StreamSink, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let record = log.clone();
        let sink: StreamSink = Arc::new(move |ev| {
            let line = match ev {
                StreamEvent::Thought(_) => return,
                StreamEvent::TextDelta(t) => format!("text {t}"),
                StreamEvent::ToolStart(t) => format!("start {}", t.title),
                StreamEvent::ToolUpdate {
                    content: ToolContent::Text(t),
                    ..
                } => format!("why {t}"),
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
    async fn a_file_change_row_names_the_file_relative_to_the_workspace() {
        let ws = workspace();
        let (engine, _llm) = scripted(vec![
            Ok(vec![call(
                "1",
                "write",
                json!({"path": "letter.txt", "content": "Dear sir,\nthe bill.\n"}),
            )]),
            Ok(vec![Part::Text("Wrote it.".into())]),
        ]);
        let rows = Arc::new(Mutex::new(Vec::new()));
        let sink: StreamSink = {
            let rows = rows.clone();
            Arc::new(move |ev| {
                if let StreamEvent::ToolUpdate { content, .. } = ev {
                    rows.lock().unwrap().push(content.summary());
                }
            })
        };
        engine
            .run_text(&request(ws.clone(), ToolAccess::Full), sink)
            .await
            .unwrap();
        assert_eq!(*rows.lock().unwrap(), vec!["letter.txt: 2 lines, was 0"]);
        assert!(ws.join("letter.txt").exists());
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
        // The failed one says why on its row, in one line.
        let why = log.iter().find(|l| l.starts_with("why ")).unwrap();
        assert!(
            why.contains("`../escape` is outside the workspace"),
            "{why}"
        );
        assert!(!why.contains('\n'), "{why}");

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

    /// Answers every permission prompt the same way.
    struct FakeHost(Permission);

    impl crate::engine::Host for FakeHost {
        fn ask<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Permission> {
            Box::pin(async move { self.0 })
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
        // The second identical `ls` is deduplicated; the third stops the tools.
        let Message::ToolResults(second) = &seen[3].messages[6] else {
            panic!()
        };
        assert!(second[0].content.contains("already read"));
        assert!(!seen[4].allow_tools);
        fs::remove_dir_all(ws).unwrap();
    }

    #[tokio::test]
    async fn duplicate_reads_in_one_step_are_deduplicated() {
        let ws = workspace();
        let (engine, llm) = scripted(vec![
            Ok(vec![
                call("1", "read", json!({"path": "Cargo.toml"})),
                call("2", "read", json!({"path": "./Cargo.toml"})),
                call("3", "read", json!({"path": "Cargo.toml"})),
                call("4", "read", json!({"path": "Cargo.toml"})),
            ]),
            Ok(vec![Part::Text("done".into())]),
        ]);
        let (sink, log) = recorder();
        let out = engine
            .run_text(&request(ws.clone(), ToolAccess::ReadOnly), sink)
            .await
            .unwrap();
        assert_eq!(out, "done");
        // Only the first `read` actually ran.
        let starts = log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with("start "))
            .count();
        assert_eq!(starts, 1);
        let seen = llm.seen.lock().unwrap();
        let Message::ToolResults(results) = &seen[1].messages[2] else {
            panic!()
        };
        assert_eq!(results.len(), 4);
        assert!(results[0].content.contains("name = \"demo\""));
        assert!(results[1].content.contains("already requested in this step"));
        assert!(results[2].content.contains("already requested in this step"));
        assert!(results[3].content.contains("already requested in this step"));
        assert!(!seen[1].allow_tools);
        fs::remove_dir_all(ws).unwrap();
    }

    #[tokio::test]
    async fn edit_invalidates_active_reads_so_file_can_be_reread() {
        let ws = workspace();
        let (engine, llm) = scripted(vec![
            Ok(vec![call("1", "read", json!({"path": "Cargo.toml"}))]),
            Ok(vec![call(
                "2",
                "edit",
                json!({"path": "Cargo.toml", "old_string": "demo", "new_string": "updated"}),
            )]),
            Ok(vec![call("3", "read", json!({"path": "Cargo.toml"}))]),
            Ok(vec![Part::Text("done".into())]),
        ]);
        let mut req = request(ws.clone(), ToolAccess::Full);
        req.ctx.host = Some(Arc::new(FakeHost(Permission::Once)));
        let out = engine.run_text(&req, recorder().0).await.unwrap();
        assert_eq!(out, "done");
        let seen = llm.seen.lock().unwrap();
        let Message::ToolResults(reread) = &seen[3].messages[6] else {
            panic!()
        };
        assert!(reread[0].content.contains("name = \"updated\""));
        fs::remove_dir_all(ws).unwrap();
    }

    #[test]
    fn canonical_call_key_normalizes_args_and_pages() {
        let a = ToolCall {
            id: "1".into(),
            name: "read".into(),
            args: json!({"pages": "24-24", "path": "./FUB_Folien1.pdf"}),
        };
        let b = ToolCall {
            id: "2".into(),
            name: "read".into(),
            args: json!({"path": "FUB_Folien1.pdf", "pages": " 24 "}),
        };
        assert_eq!(canonical_call_key(&a), canonical_call_key(&b));
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

    #[cfg(unix)]
    #[tokio::test]
    async fn command_output_survives_a_timeout() {
        let ws = workspace();
        let cmd = "echo before; echo oops >&2; sleep 5";
        let out = run_local(
            "t",
            cmd,
            shell::current().argv(cmd),
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

    /// A timeout ends what the command started too, not only the shell.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_timeout_kills_the_whole_tree() {
        let ws = workspace();
        let cmd = "sleep 30 & echo $! > child.pid; wait";
        let out = run_local(
            "t",
            cmd,
            shell::current().argv(cmd),
            &ws,
            Duration::from_millis(500),
            &CancellationToken::new(),
            &recorder().0,
        )
        .await
        .unwrap();
        assert_eq!(out.exit_code, None);
        let pid = fs::read_to_string(ws.join("child.pid")).unwrap();
        let pid: i32 = pid.trim().parse().unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        // SAFETY: signal 0 only checks that the process exists.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "the backgrounded sleep survived");
        fs::remove_dir_all(ws).unwrap();
    }

    /// With nothing to ask, a command that needs approval doesn't run.
    #[tokio::test]
    async fn commands_needing_approval_fail_closed_without_a_host() {
        let ws = workspace();
        let req = request(ws.clone(), ToolAccess::Full);
        assert!(req.ctx.host.is_none());
        let args = json!({"command": "touch ran", "network": true, "justification": "test"});
        let err = run_command("b", &args, &req, &recorder().0)
            .await
            .unwrap_err();
        assert!(err.contains("nothing can ask"), "{err}");
        assert!(!ws.join("ran").exists());
        fs::remove_dir_all(ws).unwrap();
    }

    #[test]
    fn the_prompt_knows_the_date() {
        let req = request(std::env::temp_dir(), ToolAccess::ReadOnly);
        let env = environment(&req);
        let today = env.lines().find_map(|l| l.strip_prefix("Today: ")).unwrap();
        assert!(today.len() >= "2026-01-01 (Monday)".len(), "{today}");
        assert!(!today.contains(':'), "date should stay stable across minutes: {today}");
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
