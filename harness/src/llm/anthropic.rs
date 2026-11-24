//! Anthropic Messages API (`POST /v1/messages`, streaming).

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{EventSink, Llm, LlmError, LlmEvent, LlmRequest, Message, Part, ToolCall, send_sse};
use crate::engine::BoxFuture;

const API_VERSION: &str = "2026-10-21";
const MAX_TOKENS: u32 = 64_000;

pub struct Anthropic {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl Anthropic {
    pub fn new(http: reqwest::Client, api_key: String, base_url: Option<String>) -> Self {
        Self {
            http,
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string()),
        }
    }
}

pub fn request_body(req: &LlmRequest) -> Value {
    let messages: Vec<Value> = req.messages.iter().map(message_json).collect();
    let mut body = json!({
        "model": req.model,
        "max_tokens": MAX_TOKENS,
        "stream": true,
        // Caches the growing conversation prefix automatically.
        "cache_control": {"type": "ephemeral"},
        "system": [{"type": "text", "text": req.instructions}],
        "messages": messages,
        // Summarized so thinking can stream to the user; replayed unchanged.
        "thinking": {"type": "adaptive", "display": "summarized"},
    });
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                    "eager_input_streaming": true,
                })
            })
            .collect();
        if !req.allow_tools {
            body["tool_choice"] = json!({"type": "none"});
        }
    }
    if let Some(effort) = &req.effort {
        body["output_config"] = json!({"effort": effort});
    }
    body
}

fn message_json(m: &Message) -> Value {
    match m {
        Message::User(text) => json!({"role": "user", "content": [{"type": "text", "text": text}]}),
        Message::Assistant(parts) => {
            let content: Vec<Value> = parts
                .iter()
                .filter_map(|p| match p {
                    Part::Opaque(v) => Some(v.clone()),
                    Part::Text(t) if t.is_empty() => None,
                    Part::Text(t) => Some(json!({"type": "text", "text": t})),
                    Part::ToolCall(c) => Some(json!({
                        "type": "tool_use", "id": c.id, "name": c.name, "input": c.args
                    })),
                })
                .collect();
            json!({"role": "assistant", "content": content})
        }
        Message::ToolResults(results) => {
            let content: Vec<Value> = results
                .iter()
                .map(|r| {
                    json!({
                        "type": "tool_result",
                        "tool_use_id": r.call_id,
                        "content": r.content,
                        "is_error": r.is_error,
                    })
                })
                .collect();
            json!({"role": "user", "content": content})
        }
    }
}

#[derive(Debug)]
enum Block {
    Text(String),
    Thinking {
        thinking: String,
        signature: String,
    },
    Opaque(Value),
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
}

/// Turns stream events into the assistant message.
#[derive(Default)]
pub struct StreamState {
    blocks: Vec<Block>,
    stop_reason: Option<String>,
    done: bool,
}

impl StreamState {
    /// Handles one event; returns true at the end of the message.
    pub fn handle(&mut self, data: &str, events: EventSink<'_>) -> Result<bool, LlmError> {
        let v: Value = serde_json::from_str(data)
            .map_err(|e| LlmError::Protocol(format!("bad event JSON: {e}")))?;
        match v.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                let block = &v["content_block"];
                self.blocks
                    .push(match block.get("type").and_then(Value::as_str) {
                        Some("text") => Block::Text(String::new()),
                        Some("thinking") => Block::Thinking {
                            thinking: String::new(),
                            signature: String::new(),
                        },
                        Some("tool_use") => Block::ToolUse {
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name: block["name"].as_str().unwrap_or_default().to_string(),
                            json: String::new(),
                        },
                        _ => Block::Opaque(block.clone()),
                    });
            }
            Some("content_block_delta") => {
                let delta = &v["delta"];
                let text = |k: &str| delta[k].as_str().unwrap_or_default();
                match (self.blocks.last_mut(), delta["type"].as_str()) {
                    (Some(Block::Text(t)), Some("text_delta")) => {
                        t.push_str(text("text"));
                        events(LlmEvent::Text(text("text").to_string()));
                    }
                    (Some(Block::Thinking { thinking, .. }), Some("thinking_delta")) => {
                        thinking.push_str(text("thinking"));
                        events(LlmEvent::Reasoning(text("thinking").to_string()));
                    }
                    (Some(Block::Thinking { signature, .. }), Some("signature_delta")) => {
                        signature.push_str(text("signature"));
                    }
                    (Some(Block::ToolUse { json, .. }), Some("input_json_delta")) => {
                        json.push_str(text("partial_json"));
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(r.to_string());
                }
            }
            Some("message_stop") => {
                self.done = true;
                return Ok(true);
            }
            Some("error") => {
                let kind = v
                    .pointer("/error/type")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let message = v
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
                    .to_string();
                return Err(match kind {
                    "overloaded_error" => LlmError::Overloaded(message),
                    "rate_limit_error" => LlmError::RateLimited {
                        retry_after: None,
                        message,
                    },
                    "authentication_error" | "permission_error" => LlmError::Auth(message),
                    "api_error" => LlmError::Server {
                        status: 500,
                        message,
                    },
                    _ => LlmError::Protocol(message),
                });
            }
            _ => {}
        }
        Ok(false)
    }

    pub fn finish(self) -> Result<Vec<Part>, LlmError> {
        if !self.done {
            return Err(LlmError::Network("stream ended before message_stop".into()));
        }
        if self.stop_reason.as_deref() == Some("refusal") {
            return Err(LlmError::Request {
                status: 200,
                message: "the model declined this request".into(),
            });
        }
        self.blocks
            .into_iter()
            .map(|b| {
                Ok(match b {
                    Block::Text(t) => Part::Text(t),
                    Block::Thinking {
                        thinking,
                        signature,
                    } => Part::Opaque(
                        json!({"type": "thinking", "thinking": thinking, "signature": signature}),
                    ),
                    Block::Opaque(v) => Part::Opaque(v),
                    Block::ToolUse { id, name, json } => {
                        let args = if json.trim().is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(&json).map_err(|e| {
                                LlmError::Protocol(format!("bad tool input for {name}: {e}"))
                            })?
                        };
                        Part::ToolCall(ToolCall { id, name, args })
                    }
                })
            })
            .collect()
    }
}

impl Llm for Anthropic {
    fn complete<'a>(
        &'a self,
        req: &'a LlmRequest,
        events: EventSink<'a>,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<Part>, LlmError>> {
        Box::pin(async move {
            let builder = self
                .http
                .post(format!("{}/v1/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .json(&request_body(req));
            let mut state = StreamState::default();
            send_sse(builder, cancel, |_, data| state.handle(data, events)).await?;
            state.finish()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::llm::testing::serve_once;
    use crate::llm::{ToolResult, ToolSpec};

    const STREAM: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Check the file."}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig=="}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Reading."}}

event: content_block_start
data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"read","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\": \"Car"}}

event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"go.toml\"}"}}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}

event: message_stop
data: {"type":"message_stop"}

"#;

    fn request() -> LlmRequest {
        LlmRequest {
            model: "claude-sonnet-5-5".into(),
            instructions: "Be brief.".into(),
            messages: vec![Message::User("What is the crate name?".into())],
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "Read a file".into(),
                parameters: json!({"type": "object"}),
            }],
            effort: Some("low".into()),
            allow_tools: true,
            cache_key: None,
        }
    }

    #[tokio::test]
    async fn streams_thinking_text_and_tool_call_over_http() {
        let (url, captured) = serve_once("200 OK", STREAM).await;
        let llm = Anthropic::new(reqwest::Client::new(), "sk-test".into(), Some(url));
        let seen = Mutex::new(Vec::new());
        let sink = |e: LlmEvent| seen.lock().unwrap().push(e);
        let parts = llm
            .complete(&request(), &sink, &CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(
            parts,
            vec![
                Part::Opaque(
                    json!({"type": "thinking", "thinking": "Check the file.", "signature": "sig=="})
                ),
                Part::Text("Reading.".into()),
                Part::ToolCall(ToolCall {
                    id: "toolu_1".into(),
                    name: "read".into(),
                    args: json!({"path": "Cargo.toml"}),
                }),
            ]
        );
        assert_eq!(
            *seen.lock().unwrap(),
            [
                LlmEvent::Reasoning("Check the file.".into()),
                LlmEvent::Text("Reading.".into())
            ]
        );

        let cap = captured.lock().unwrap().take().unwrap();
        assert!(cap.head.contains("x-api-key: sk-test"));
        assert!(cap.head.contains("anthropic-version: 2026-10-21"));
        assert_eq!(cap.body["thinking"]["type"], "adaptive");
        assert_eq!(cap.body["output_config"]["effort"], "low");
        assert_eq!(cap.body["tools"][0]["input_schema"]["type"], "object");
        assert!(cap.body.get("temperature").is_none());
    }

    #[test]
    fn replays_thinking_and_tool_results_unchanged() {
        let thinking = json!({"type": "thinking", "thinking": "t", "signature": "s"});
        let mut req = request();
        req.messages.push(Message::Assistant(vec![
            Part::Opaque(thinking.clone()),
            Part::ToolCall(ToolCall {
                id: "toolu_1".into(),
                name: "read".into(),
                args: json!({"path": "a"}),
            }),
        ]));
        req.messages.push(Message::ToolResults(vec![ToolResult {
            call_id: "toolu_1".into(),
            content: "data".into(),
            is_error: false,
        }]));
        req.allow_tools = false;
        let body = request_body(&req);
        assert_eq!(body["messages"][1]["content"][0], thinking);
        assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(body["tool_choice"]["type"], "none");
    }

    #[tokio::test]
    async fn maps_errors_and_truncated_streams() {
        let (url, _) = serve_once(
            "529 Overloaded",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
        )
        .await;
        let llm = Anthropic::new(reqwest::Client::new(), "k".into(), Some(url));
        let err = llm
            .complete(&request(), &|_| {}, &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err, LlmError::Overloaded("busy".into()));
        assert!(err.retryable());

        let truncated = STREAM.split("event: message_stop").next().unwrap();
        let (url, _) = serve_once("200 OK", truncated).await;
        let llm = Anthropic::new(reqwest::Client::new(), "k".into(), Some(url));
        let err = llm
            .complete(&request(), &|_| {}, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Network(_)));
    }
}
