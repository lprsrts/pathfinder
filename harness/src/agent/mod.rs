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

use crate::config::{ModelEntry, Route};
use crate::engine::{
    BoxFuture, Engine, EngineRequest, EngineTier, StreamEvent, StreamSink, ToolAccess,
};
use crate::llm::anthropic::Anthropic;
use crate::llm::responses::Responses;
use crate::llm::{Llm, LlmError, LlmEvent, LlmRequest, Message, Part, ToolResult, text_of};
use crate::stream::describe_tool;
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

    /// Clients backed by API keys from the environment.
    pub fn from_env() -> Self {
        let http = reqwest::Client::new();
        Self::new(Arc::new(
            move |entry: &ModelEntry| -> Result<Arc<dyn Llm>> {
                let key = |var: &str| {
                    env::var(var)
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                        .ok_or_else(|| {
                            anyhow!("{} needs an API key: set {var}", entry.route.label())
                        })
                };
                let base = |var: &str| env::var(var).ok().filter(|v| !v.trim().is_empty());
                Ok(match entry.route {
                    Route::Anthropic => Arc::new(Anthropic::new(
                        http.clone(),
                        key("ANTHROPIC_API_KEY")?,
                        base("HARNESS_ANTHROPIC_BASE_URL"),
                    )),
                    Route::Openai => Arc::new(Responses::new(
                        http.clone(),
                        key("OPENAI_API_KEY")?,
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

/// The first user message: the prompt plus small attached files inlined.
fn first_message(req: &EngineRequest) -> String {
    let mut text = req.prompt.clone();
    for file in &req.files {
        let Ok(meta) = std::fs::metadata(file) else {
            continue;
        };
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
    text
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
            sink(StreamEvent::Thought(format!(
                "{} via {} ({})...\n",
                req.tier.label(),
                model.route.label(),
                model.model
            )));
            if req.tools == ToolAccess::Full {
                bail!(
                    "Agent work (/handoff, /delegate) on {} isn't available yet; pick a Claude Code or Codex main model for it.",
                    model.route.label()
                );
            }
            let llm = (self.factory)(model)?;
            let specs = match req.tools {
                ToolAccess::None => Vec::new(),
                _ => tools::read_only_specs(),
            };
            let run = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
            let root: PathBuf = req.cwd.clone();
            let mut request = LlmRequest {
                model: model.model.clone(),
                instructions: format!("{}{}", req.system_prompt, PROTOCOL_NOTE),
                messages: vec![Message::User(first_message(req))],
                tools: specs,
                effort: model.effort.clone(),
                allow_tools: req.tools != ToolAccess::None,
                cache_key: None,
            };
            let steps = max_steps(req);

            for step in 0..=steps {
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
                // Read-only tools run concurrently; order is kept for results.
                let results = join_all(calls.iter().map(|call| {
                    let sink = sink.clone();
                    let env = &env;
                    let id = format!("{run}-{}", call.id);
                    async move {
                        sink(StreamEvent::ToolStart(describe_tool(
                            id.clone(),
                            &call.name,
                            &call.args,
                            &req.cwd,
                        )));
                        let outcome = tools::run(&call.name, &call.args, env);
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
                        }
                    }
                }))
                .await;
                request.messages.push(Message::ToolResults(results));
            }
            unreachable!("the final step always returns")
        })
    }
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
                sink(StreamEvent::Thought(format!(
                    "{err} — retrying in {}s...\n",
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
                    events(LlmEvent::Text(text_of(parts)));
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
            ctx: RunContext::default(),
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

        let (engine, _) = scripted(vec![]);
        assert!(
            engine
                .run_text(&request(ws.clone(), ToolAccess::Full), recorder().0)
                .await
                .unwrap_err()
                .to_string()
                .contains("Claude Code or Codex")
        );
        fs::remove_dir_all(ws).unwrap();
    }
}
