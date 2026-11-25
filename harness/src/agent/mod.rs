//! Pathfinder's own agent loop: calls a model API, runs tools, repeats until
//! the model answers. Sits behind the `Engine` trait, so the theory loop in
//! runtime.rs doesn't know or care which route ran.

pub mod confine;
pub mod tools;

use std::env;
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
    BoxFuture, CommandOutput, Engine, EngineRequest, EngineTier, Permission, StreamEvent,
    StreamSink, ToolAccess,
};
use crate::llm::anthropic::Anthropic;
use crate::llm::chat::Chat;
use crate::llm::responses::{Bearer, Responses};
use crate::llm::{Llm, LlmError, LlmEvent, LlmRequest, Message, Part, ToolResult, text_of};
use crate::stream::{ToolContent, describe_tool};
use tools::ToolEnv;

const MAX_TOOL_OUTPUT: usize = 30 * 1024;
const MAX_ATTEMPTS: u32 = 3;
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
                let base = |var: &str| env::var(var).ok().filter(|v| !v.trim().is_empty());
                if entry.route == Route::Chatgpt {
                    let session = creds.chatgpt();
                    if !session.signed_in() {
                        return Err(ChatGptError::NotSignedIn.into());
                    }
                    return Ok(Arc::new(Responses::with_bearer(
                        http.clone(),
                        Bearer::ChatGpt(session),
                        base("HARNESS_OPENAI_BASE_URL"),
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
                                "endpoint `{id}` needs the key `{name}`: run `harness auth import {name}` or `harness auth set {name}`"
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
                        base("HARNESS_ANTHROPIC_BASE_URL"),
                    )),
                    Route::Openai => Arc::new(Responses::new(
                        http.clone(),
                        key,
                        base("HARNESS_OPENAI_BASE_URL"),
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
        if meta.len() <= MAX_ATTACHED_INLINE {
            if let Ok(content) = std::fs::read_to_string(file) {
                let _ = write!(
                    text,
                    "\n\n<attached path=\"{}\">\n{content}\n</attached>",
                    file.display()
                );
                continue;
            }
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

fn cap(mut s: String) -> String {
    if s.len() > MAX_TOOL_OUTPUT {
        let mut cut = MAX_TOOL_OUTPUT;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n... (output truncated)");
    }
    s
}

impl Engine for ApiEngine {
    fn run_text<'a>(
        &'a self,
        req: &'a EngineRequest,
        sink: StreamSink,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
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
                instructions: format!("{}{}", req.system_prompt, PROTOCOL_NOTE),
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
                cache_key: None,
            };
            let steps = max_steps(req);

            for step in 0..=steps {
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
                let parts = complete_with_retry(llm.as_ref(), &request, req, &sink).await?;
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
                        bail!("the model returned no answer");
                    }
                    return Ok(text);
                }

                let env = ToolEnv {
                    root: &root,
                    attached: &req.files,
                };
                // Read-only tools run concurrently; agent work runs in order,
                // since edits and commands depend on each other.
                let results = if req.tools == ToolAccess::Full {
                    let mut out = Vec::new();
                    for call in &calls {
                        out.push(run_call(call, &run, &env, req, &sink).await);
                    }
                    out
                } else {
                    join_all(
                        calls
                            .iter()
                            .map(|call| run_call(call, &run, &env, req, &sink)),
                    )
                    .await
                };
                request.messages.push(Message::ToolResults(results));
            }
            unreachable!("the final step always returns")
        })
    }
}

/// Runs one tool call, showing it in the thread: diffs for file changes, a
/// live terminal (after the user allows it) for commands.
async fn run_call(
    call: &crate::llm::ToolCall,
    run: &str,
    env: &ToolEnv<'_>,
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
    let mut images = Vec::new();
    let outcome = match call.name.as_str() {
        "edit" | "write" if full => change_file(call, env, req).await.map(|(msg, c)| {
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
        "view" => tools::view(&call.args, env).map(|(text, found)| {
            images = found;
            text
        }),
        _ => tools::run(&call.name, &call.args, env),
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
    let cancel = &req.ctx.cancel;
    let output = match &req.ctx.host {
        Some(host) => {
            // "Always allow" covers later commands with the same program.
            let program = command.split_whitespace().next().unwrap_or(command);
            match host.ask(id, program).await {
                Permission::Denied => {
                    return Err("The user declined to run this command.".into());
                }
                Permission::Once | Permission::Always => {}
            }
            match host.run_in_terminal(id, command, &req.cwd, timeout, cancel) {
                Some(run) => run.await.map_err(|e| e.to_string())?,
                None => run_local(id, command, &req.cwd, timeout, cancel, sink).await?,
            }
        }
        // No client (tests): nobody to ask.
        None => run_local(id, command, &req.cwd, timeout, cancel, sink).await?,
    };
    let status = match output.exit_code {
        Some(0) => "Exit code 0".to_string(),
        Some(code) => format!("Exit code {code}"),
        None if cancel.is_cancelled() => "Stopped by the user".to_string(),
        None => format!("Killed after {}s", timeout.as_secs()),
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
    match output.exit_code {
        Some(0) => Ok(text),
        _ => Err(text),
    }
}

/// Runs a command here and shows its output in the tool call afterwards.
async fn run_local(
    id: &str,
    command: &str,
    cwd: &std::path::Path,
    timeout: Duration,
    cancel: &tokio_util::sync::CancellationToken,
    sink: &StreamSink,
) -> Result<CommandOutput, String> {
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("can't start the command: {e}"))?;
    let finished = tokio::select! {
        out = child.wait_with_output() => Some(out.map_err(|e| e.to_string())?),
        _ = tokio::time::sleep(timeout) => None,
        _ = cancel.cancelled() => None,
    };
    let Some(out) = finished else {
        return Ok(CommandOutput {
            output: String::new(),
            exit_code: None,
            truncated: false,
        });
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let truncated = text.len() > MAX_TOOL_OUTPUT;
    let text = cap(text);
    sink(StreamEvent::ToolUpdate {
        id: id.to_string(),
        content: ToolContent::Text(format!("```\n$ {command}\n{}\n```", text.trim_end())),
    });
    Ok(CommandOutput {
        output: text,
        exit_code: out.status.code(),
        truncated,
    })
}

/// One model call, retried on transient errors while nothing has streamed.
async fn complete_with_retry(
    llm: &dyn Llm,
    request: &LlmRequest,
    req: &EngineRequest,
    sink: &StreamSink,
) -> Result<Vec<Part>> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let emitted = AtomicBool::new(false);
        let on_event = |event: LlmEvent| {
            emitted.store(true, Ordering::Relaxed);
            match event {
                LlmEvent::Reasoning(t) => sink(StreamEvent::Thought(t)),
                LlmEvent::Text(t) if req.stream_text => sink(StreamEvent::TextDelta(t)),
                LlmEvent::Text(_) => {}
            }
        };
        match llm.complete(request, &on_event, &req.ctx.cancel).await {
            Ok(parts) => return Ok(parts),
            Err(err)
                if err.retryable()
                    && attempt < MAX_ATTEMPTS
                    && !emitted.load(Ordering::Relaxed) =>
            {
                let wait = err
                    .retry_after()
                    .unwrap_or(Duration::from_secs(1 << (attempt - 1)))
                    .min(Duration::from_secs(30));
                tracing::warn!("{err}; retrying in {}s", wait.as_secs());
                sink(StreamEvent::Thought(format!(
                    "\n\nThe provider didn't answer; retrying in {}s.\n",
                    wait.as_secs()
                )));
                tokio::time::sleep(wait).await;
            }
            Err(LlmError::Cancelled) => bail!("cancelled"),
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
    use crate::engine::{RunContext, TierModels};
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
            .map(|i| Ok(vec![call(&i.to_string(), "ls", json!({}))]))
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
        let script = || {
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
                    json!({"command": "grep -c neuron Cargo.toml"}),
                )]),
                Ok(vec![Part::Text("Renamed it.".into())]),
            ]
        };
        let (engine, llm) = scripted(script());
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
        let seen = llm.seen.lock().unwrap();
        let Message::ToolResults(results) = seen[2].messages.last().unwrap() else {
            panic!("expected tool results");
        };
        assert!(results[0].content.starts_with("Exit code 0\n1"));
        drop(seen);

        // Declined: the command doesn't run and the model is told so.
        let (engine, llm) = scripted(script());
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
}
