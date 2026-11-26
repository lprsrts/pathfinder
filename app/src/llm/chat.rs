//! OpenAI-compatible Chat Completions (`/chat/completions`): the protocol
//! self-hosted servers speak (vLLM, Ollama, llama.cpp, LM Studio) and hosted
//! open-model services (OpenRouter and the like). Streaming, function tools,
//! images; reasoning deltas are shown but not replayed, since these servers
//! don't need them back.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{EventSink, Llm, LlmError, LlmEvent, LlmRequest, Message, Part, ToolCall, send_sse};
use crate::engine::BoxFuture;

pub struct Chat {
    http: reqwest::Client,
    base_url: String,
    /// None for servers without auth (a local Ollama or llama.cpp).
    key: Option<String>,
    /// Merged into every request body (the endpoint's `extra`).
    extra: Option<Value>,
}

impl Chat {
    pub fn new(http: reqwest::Client, base_url: String, key: Option<String>) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            key,
            extra: None,
        }
    }

    pub fn with_extra(mut self, extra: Option<Value>) -> Self {
        self.extra = extra;
        self
    }
}

fn image_part(data_url: String) -> Value {
    json!({"type": "image_url", "image_url": {"url": data_url}})
}

fn messages(req: &LlmRequest) -> Vec<Value> {
    let mut out = vec![json!({"role": "system", "content": req.instructions})];
    for m in &req.messages {
        match m {
            Message::User(text) => out.push(json!({"role": "user", "content": text})),
            Message::UserWith(text, files) => {
                let mut content = vec![json!({"type": "text", "text": text})];
                for f in files {
                    if f.is_image() {
                        content.push(image_part(f.data_url()));
                    } else {
                        // These servers take no PDFs: the model reads them
                        // with its tools.
                        content.push(json!({
                            "type": "text",
                            "text": format!("({} is a PDF: read it with `read`, or see its pages with `view`.)", f.name),
                        }));
                    }
                }
                out.push(json!({"role": "user", "content": content}));
            }
            Message::Assistant(parts) => {
                let text: String = parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect();
                let calls: Vec<Value> = parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::ToolCall(c) => Some(json!({
                            "id": c.id,
                            "type": "function",
                            "function": {"name": c.name, "arguments": c.args.to_string()},
                        })),
                        _ => None,
                    })
                    .collect();
                let mut msg = json!({"role": "assistant", "content": text});
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                }
                out.push(msg);
            }
            Message::ToolResults(results) => {
                for r in results {
                    let content = if r.is_error {
                        format!("Error: {}", r.content)
                    } else {
                        r.content.clone()
                    };
                    out.push(
                        json!({"role": "tool", "tool_call_id": r.call_id, "content": content}),
                    );
                }
                // Tool messages are text: images follow as a user message.
                for r in results.iter().filter(|r| !r.images.is_empty()) {
                    let mut content = vec![json!({
                        "type": "text",
                        "text": format!("Images returned by tool call {}:", r.call_id),
                    })];
                    content.extend(r.images.iter().map(|f| image_part(f.data_url())));
                    out.push(json!({"role": "user", "content": content}));
                }
            }
        }
    }
    out
}

pub fn request_body(req: &LlmRequest) -> Value {
    let mut body = json!({
        "model": req.model,
        "messages": messages(req),
        "stream": true,
    });
    if !req.tools.is_empty() {
        body["tools"] = json!(
            req.tools
                .iter()
                .map(|t| json!({
                    "type": "function",
                    "function": {"name": t.name, "description": t.description, "parameters": t.parameters},
                }))
                .collect::<Vec<_>>()
        );
        body["tool_choice"] = json!(if req.allow_tools { "auto" } else { "none" });
    }
    body
}

/// A tool call as it streams in, by index.
#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct StreamState {
    text: String,
    calls: BTreeMap<u64, PendingCall>,
    done: bool,
}

impl StreamState {
    fn handle(&mut self, data: &str, events: EventSink<'_>) -> Result<bool, LlmError> {
        let data = data.trim();
        if data == "[DONE]" {
            self.done = true;
            return Ok(true);
        }
        let v: Value = serde_json::from_str(data)
            .map_err(|e| LlmError::Protocol(format!("bad stream chunk: {e}")))?;
        if let Some(err) = v.get("error") {
            let message = err["message"]
                .as_str()
                .unwrap_or("stream error")
                .to_string();
            return Err(LlmError::Server {
                status: err["code"].as_u64().unwrap_or(500) as u16,
                message,
            });
        }
        let Some(choice) = v["choices"].get(0) else {
            return Ok(false);
        };
        let delta = &choice["delta"];
        // vLLM and llama.cpp send `reasoning_content`, OpenRouter `reasoning`.
        for key in ["reasoning_content", "reasoning"] {
            if let Some(r) = delta[key].as_str().filter(|r| !r.is_empty()) {
                events(LlmEvent::Reasoning(r.to_string()));
            }
        }
        if let Some(t) = delta["content"].as_str().filter(|t| !t.is_empty()) {
            self.text.push_str(t);
            events(LlmEvent::Text(t.to_string()));
        }
        for c in delta["tool_calls"].as_array().into_iter().flatten() {
            let index = c["index"].as_u64().unwrap_or(0);
            let pending = self.calls.entry(index).or_default();
            if let Some(id) = c["id"].as_str().filter(|s| !s.is_empty()) {
                pending.id = id.to_string();
            }
            if let Some(name) = c["function"]["name"].as_str().filter(|s| !s.is_empty()) {
                pending.name = name.to_string();
            }
            if let Some(args) = c["function"]["arguments"].as_str() {
                pending.arguments.push_str(args);
            }
        }
        if choice["finish_reason"].is_string() {
            self.done = true;
        }
        Ok(false)
    }

    fn parts(self) -> Result<Vec<Part>, LlmError> {
        if !self.done {
            return Err(LlmError::Network("the stream ended early".into()));
        }
        let mut parts = Vec::new();
        if !self.text.is_empty() {
            parts.push(Part::Text(self.text));
        }
        for (i, c) in self.calls {
            let args = crate::llm::parse_args(&c.arguments);
            parts.push(Part::ToolCall(ToolCall {
                id: if c.id.is_empty() {
                    format!("call_{i}")
                } else {
                    c.id
                },
                name: c.name,
                args,
            }));
        }
        Ok(parts)
    }
}

impl Llm for Chat {
    fn complete<'a>(
        &'a self,
        req: &'a LlmRequest,
        events: EventSink<'a>,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<Part>, LlmError>> {
        Box::pin(async move {
            let mut body = request_body(req);
            crate::config::merge_extra(&mut body, self.extra.as_ref());
            let mut builder = self
                .http
                .post(format!("{}/chat/completions", self.base_url))
                .json(&body);
            if let Some(key) = &self.key {
                builder = builder.bearer_auth(key);
            }
            let mut state = StreamState::default();
            send_sse(builder, cancel, |_, data| state.handle(data, events)).await?;
            state.parts()
        })
    }
}

/// The models a server lists (`GET /models`).
pub async fn list_models(
    http: &reqwest::Client,
    base_url: &str,
    key: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let mut req = http.get(format!("{}/models", base_url.trim_end_matches('/')));
    if let Some(key) = key {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        anyhow::bail!("{base_url}/models returned {status}");
    }
    Ok(body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::llm::testing::serve_once;
    use crate::llm::{Attachment, ToolResult, ToolSpec};

    fn request() -> LlmRequest {
        LlmRequest {
            model: "qwen-test".into(),
            instructions: "Be brief.".into(),
            messages: vec![Message::User("Read the notes.".into())],
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "Read a file.".into(),
                parameters: json!({"type": "object"}),
            }],
            effort: None,
            allow_tools: true,
            cache_key: None,
        }
    }

    const STREAM: &str = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Look first.\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"Reading.\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"pa\"}}]}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"th\\\": \\\"notes.md\\\"}\"}}]}}]}\n\n\
data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
data: [DONE]\n\n";

    #[tokio::test]
    async fn streams_text_reasoning_and_chunked_tool_calls() {
        let (url, captured) = serve_once("200 OK", STREAM).await;
        let llm = Chat::new(reqwest::Client::new(), url, None);
        let seen = Mutex::new(Vec::new());
        let sink = |e: LlmEvent| seen.lock().unwrap().push(e);
        let parts = llm
            .complete(&request(), &sink, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(parts[0], Part::Text("Reading.".into()));
        assert_eq!(
            parts[1],
            Part::ToolCall(ToolCall {
                id: "c1".into(),
                name: "read".into(),
                args: json!({"path": "notes.md"}),
            })
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0], LlmEvent::Reasoning("Look first.".into()));
        let c = captured.lock().unwrap();
        let c = c.as_ref().unwrap();
        assert!(c.head.starts_with("POST /chat/completions"));
        assert!(
            !c.head.to_lowercase().contains("authorization"),
            "no key, no header"
        );
        assert_eq!(c.body["messages"][0]["role"], "system");
        assert_eq!(c.body["tools"][0]["function"]["name"], "read");
    }

    #[tokio::test]
    async fn a_cut_stream_is_an_error() {
        let stream = "data: {\"choices\":[{\"delta\":{\"content\":\"Half\"}}]}\n\n";
        let (url, _) = serve_once("200 OK", stream).await;
        let llm = Chat::new(reqwest::Client::new(), url, Some("k".into()));
        let err = llm
            .complete(&request(), &|_| {}, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Network(_)));
    }

    #[test]
    fn history_tools_and_images_map_to_chat_messages() {
        let img = Attachment {
            name: "p1".into(),
            media_type: "image/png".into(),
            data: "AAA=".into(),
        };
        let mut req = request();
        req.messages = vec![
            Message::UserWith(
                "Explain.".into(),
                vec![
                    img.clone(),
                    Attachment {
                        name: "sheet.pdf".into(),
                        media_type: "application/pdf".into(),
                        data: "BBB=".into(),
                    },
                ],
            ),
            Message::Assistant(vec![Part::ToolCall(ToolCall {
                id: "c1".into(),
                name: "view".into(),
                args: json!({"path": "sheet.pdf"}),
            })]),
            Message::ToolResults(vec![ToolResult {
                call_id: "c1".into(),
                content: "page 1".into(),
                is_error: false,
                images: vec![img],
            }]),
        ];
        let m = messages(&req);
        assert_eq!(
            m[1]["content"][1]["image_url"]["url"],
            "data:image/png;base64,AAA="
        );
        assert!(
            m[1]["content"][2]["text"]
                .as_str()
                .unwrap()
                .contains("sheet.pdf is a PDF")
        );
        assert_eq!(m[2]["tool_calls"][0]["function"]["name"], "view");
        assert_eq!(m[3]["role"], "tool");
        assert_eq!(m[4]["role"], "user");
        assert_eq!(m[4]["content"][1]["type"], "image_url");
    }
}
