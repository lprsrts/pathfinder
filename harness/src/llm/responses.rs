//! OpenAI Responses API (`POST /v1/responses`, streaming, `store: false`).

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{EventSink, Llm, LlmError, LlmEvent, LlmRequest, Message, Part, ToolCall, send_sse};
use crate::engine::BoxFuture;

pub struct Responses {
    http: reqwest::Client,
    bearer: String,
    base_url: String,
}

impl Responses {
    pub fn new(http: reqwest::Client, bearer: String, base_url: Option<String>) -> Self {
        Self {
            http,
            bearer,
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
        }
    }
}

pub fn request_body(req: &LlmRequest) -> Value {
    let input: Vec<Value> = req.messages.iter().flat_map(input_items).collect();
    let mut body = json!({
        "model": req.model,
        "instructions": req.instructions,
        "input": input,
        "stream": true,
        // Nothing is stored server-side; reasoning is carried in the history.
        "store": false,
        "include": ["reasoning.encrypted_content"],
        "reasoning": {"summary": "auto"},
    });
    if let Some(effort) = &req.effort {
        body["reasoning"]["effort"] = json!(effort);
    }
    if let Some(key) = &req.cache_key {
        body["prompt_cache_key"] = json!(key);
    }
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
        body["tool_choice"] = json!(if req.allow_tools { "auto" } else { "none" });
        body["parallel_tool_calls"] = json!(true);
    }
    body
}

fn input_items(m: &Message) -> Vec<Value> {
    match m {
        Message::User(text) => vec![json!({"role": "user", "content": text})],
        Message::Assistant(parts) => parts
            .iter()
            .filter_map(|p| match p {
                Part::Opaque(v) => Some(v.clone()),
                Part::Text(t) if t.is_empty() => None,
                Part::Text(t) => Some(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": t}],
                })),
                Part::ToolCall(c) => Some(json!({
                    "type": "function_call",
                    "call_id": c.id,
                    "name": c.name,
                    "arguments": c.args.to_string(),
                })),
            })
            .collect(),
        Message::ToolResults(results) => results
            .iter()
            .map(|r| {
                let output = if r.is_error {
                    format!("Error: {}", r.content)
                } else {
                    r.content.clone()
                };
                json!({"type": "function_call_output", "call_id": r.call_id, "output": output})
            })
            .collect(),
    }
}

fn failure(response: &Value) -> LlmError {
    let code = response
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = response
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("the response failed")
        .to_string();
    match code {
        c if c.contains("usage_limit") || c.contains("usage_unavailable") => {
            LlmError::UsageLimit(message)
        }
        "rate_limit_exceeded" => LlmError::RateLimited {
            retry_after: None,
            message,
        },
        "server_error" => LlmError::Server {
            status: 500,
            message,
        },
        _ => LlmError::Protocol(message),
    }
}

#[derive(Default)]
pub struct StreamState {
    items: Vec<Value>,
    done: bool,
}

impl StreamState {
    pub fn handle(&mut self, data: &str, events: EventSink<'_>) -> Result<bool, LlmError> {
        if data == "[DONE]" {
            return Ok(true);
        }
        let v: Value = serde_json::from_str(data)
            .map_err(|e| LlmError::Protocol(format!("bad event JSON: {e}")))?;
        let delta = || v["delta"].as_str().unwrap_or_default().to_string();
        match v.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => events(LlmEvent::Text(delta())),
            Some("response.reasoning_summary_text.delta") => events(LlmEvent::Reasoning(delta())),
            Some("response.reasoning_summary_part.done") => {
                events(LlmEvent::Reasoning("\n\n".into()))
            }
            Some("response.output_item.done") => self.items.push(v["item"].clone()),
            Some("response.completed") => {
                if self.items.is_empty() {
                    if let Some(output) = v.pointer("/response/output").and_then(Value::as_array) {
                        self.items = output.clone();
                    }
                }
                self.done = true;
                return Ok(true);
            }
            Some("response.failed") => return Err(failure(&v["response"])),
            Some("response.incomplete") => {
                let reason = v
                    .pointer("/response/incomplete_details/reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                return Err(LlmError::Protocol(format!("response incomplete: {reason}")));
            }
            Some("error") => return Err(failure(&json!({"error": v}))),
            _ => {}
        }
        Ok(false)
    }

    pub fn finish(self) -> Result<Vec<Part>, LlmError> {
        if !self.done {
            return Err(LlmError::Network(
                "stream ended before response.completed".into(),
            ));
        }
        let mut parts = Vec::new();
        for item in self.items {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    let text: String = item["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["text"].as_str())
                        .collect();
                    parts.push(Part::Text(text));
                }
                Some("function_call") => {
                    let name = item["name"].as_str().unwrap_or_default().to_string();
                    let raw = item["arguments"].as_str().unwrap_or("{}");
                    let args = serde_json::from_str(if raw.trim().is_empty() { "{}" } else { raw })
                        .map_err(|e| {
                            LlmError::Protocol(format!("bad arguments for {name}: {e}"))
                        })?;
                    parts.push(Part::ToolCall(ToolCall {
                        id: item["call_id"].as_str().unwrap_or_default().to_string(),
                        name,
                        args,
                    }));
                }
                Some("reasoning") => parts.push(Part::Opaque(item)),
                _ => {}
            }
        }
        Ok(parts)
    }
}

impl Llm for Responses {
    fn complete<'a>(
        &'a self,
        req: &'a LlmRequest,
        events: EventSink<'a>,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<Part>, LlmError>> {
        Box::pin(async move {
            let builder = self
                .http
                .post(format!("{}/responses", self.base_url))
                .bearer_auth(&self.bearer)
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

    const STREAM: &str = r#"event: response.created
data: {"type":"response.created","response":{"id":"resp_1"}}

event: response.reasoning_summary_text.delta
data: {"type":"response.reasoning_summary_text.delta","delta":"Need the manifest."}

event: response.output_item.done
data: {"type":"response.output_item.done","item":{"id":"rs_1","type":"reasoning","summary":[],"encrypted_content":"enc"}}

event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"Checking."}

event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Checking."}]}}

event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"read","arguments":"{\"path\":\"Cargo.toml\"}"}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp_1","status":"completed"}}

"#;

    fn request() -> LlmRequest {
        LlmRequest {
            model: "gpt-6.1-sol".into(),
            instructions: "Be brief.".into(),
            messages: vec![Message::User("Crate name?".into())],
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "Read a file".into(),
                parameters: json!({"type": "object"}),
            }],
            effort: Some("high".into()),
            allow_tools: true,
            cache_key: Some("session-1".into()),
        }
    }

    #[tokio::test]
    async fn streams_reasoning_text_and_function_call_over_http() {
        let (url, captured) = serve_once("200 OK", STREAM).await;
        let llm = Responses::new(reqwest::Client::new(), "sk-test".into(), Some(url));
        let seen = Mutex::new(Vec::new());
        let sink = |e: LlmEvent| seen.lock().unwrap().push(e);
        let parts = llm
            .complete(&request(), &sink, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts[0],
            Part::Opaque(
                json!({"id":"rs_1","type":"reasoning","summary":[],"encrypted_content":"enc"})
            )
        );
        assert_eq!(parts[1], Part::Text("Checking.".into()));
        assert_eq!(
            parts[2],
            Part::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                args: json!({"path": "Cargo.toml"})
            })
        );
        assert_eq!(seen.lock().unwrap().len(), 2);

        let cap = captured.lock().unwrap().take().unwrap();
        assert!(
            cap.head
                .to_lowercase()
                .contains("authorization: bearer sk-test")
        );
        assert_eq!(cap.body["store"], false);
        assert_eq!(cap.body["stream"], true);
        assert_eq!(cap.body["reasoning"]["effort"], "high");
        assert_eq!(cap.body["include"][0], "reasoning.encrypted_content");
        assert_eq!(cap.body["prompt_cache_key"], "session-1");
        assert_eq!(cap.body["tools"][0]["type"], "function");
    }

    #[test]
    fn replays_reasoning_calls_and_outputs() {
        let reasoning = json!({"id":"rs_1","type":"reasoning","encrypted_content":"enc"});
        let mut req = request();
        req.messages.push(Message::Assistant(vec![
            Part::Opaque(reasoning.clone()),
            Part::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                args: json!({"path": "a"}),
            }),
        ]));
        req.messages.push(Message::ToolResults(vec![ToolResult {
            call_id: "call_1".into(),
            content: "no such file".into(),
            is_error: true,
        }]));
        let body = request_body(&req);
        assert_eq!(body["input"][1], reasoning);
        assert_eq!(body["input"][2]["type"], "function_call");
        assert_eq!(body["input"][2]["arguments"], "{\"path\":\"a\"}");
        assert_eq!(body["input"][3]["output"], "Error: no such file");
    }

    #[tokio::test]
    async fn failed_response_maps_usage_limit() {
        let stream = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\",\"message\":\"limit\"}}}\n\n";
        let (url, _) = serve_once("200 OK", stream).await;
        let llm = Responses::new(reqwest::Client::new(), "k".into(), Some(url));
        let err = llm
            .complete(&request(), &|_| {}, &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err, LlmError::UsageLimit("limit".into()));
        assert!(!err.retryable());
    }
}
