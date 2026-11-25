//! Thin clients for model APIs. Each speaks one wire protocol, streams
//! `LlmEvent`s while it runs, and returns the full assistant message,
//! including opaque parts (signed thinking, encrypted reasoning) that must be
//! replayed verbatim on the next turn.

pub mod anthropic;
pub mod chat;
pub mod responses;
pub mod sse;

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{RequestBuilder, Response, StatusCode};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::engine::BoxFuture;
use sse::SseParser;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments.
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
    /// Images the tool returns for the model to see (`view`).
    pub images: Vec<Attachment>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    ToolCall(ToolCall),
    /// Provider-specific item replayed unchanged to the same protocol
    /// (Anthropic thinking blocks, Responses reasoning items).
    Opaque(Value),
}

/// A file sent to the model as content: an image or a PDF.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub name: String,
    /// `image/png`, `image/jpeg`, `image/gif`, `image/webp` or
    /// `application/pdf`.
    pub media_type: String,
    /// Base64 of the file's bytes.
    pub data: String,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.media_type.starts_with("image/")
    }

    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.media_type, self.data)
    }
}

/// The media type Pathfinder sends a file as, by extension.
pub fn attachable_type(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    User(String),
    /// A user message with images or PDFs.
    UserWith(String, Vec<Attachment>),
    Assistant(Vec<Part>),
    ToolResults(Vec<ToolResult>),
}

#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub model: String,
    pub instructions: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    /// Reasoning effort (`low`, `medium`, `high`, ...).
    pub effort: Option<String>,
    /// False forces a final answer: tools stay declared but can't be called.
    pub allow_tools: bool,
    /// Stable per-session key for provider-side prompt caching.
    pub cache_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlmEvent {
    Reasoning(String),
    Text(String),
}

pub type EventSink<'a> = &'a (dyn Fn(LlmEvent) + Send + Sync);

#[derive(Debug, Error, PartialEq)]
pub enum LlmError {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("rate limited: {message}")]
    RateLimited {
        retry_after: Option<Duration>,
        message: String,
    },
    #[error("provider overloaded: {0}")]
    Overloaded(String),
    #[error("usage limit reached: {0}")]
    UsageLimit(String),
    #[error("server error {status}: {message}")]
    Server { status: u16, message: String },
    #[error("request rejected ({status}): {message}")]
    Request { status: u16, message: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    Protocol(String),
    #[error("cancelled")]
    Cancelled,
}

impl LlmError {
    /// Safe to retry when nothing has been shown to the user yet.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Overloaded(_) | Self::Server { .. } | Self::Network(_)
        )
    }

    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

pub trait Llm: Send + Sync {
    fn complete<'a>(
        &'a self,
        req: &'a LlmRequest,
        events: EventSink<'a>,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<Part>, LlmError>>;
}

/// Pulls a human-readable message out of a provider error body.
fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("message"))
                .or_else(|| v.get("detail"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(500).collect())
}

async fn status_error(resp: Response) -> LlmError {
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(Duration::from_secs_f64);
    let body = resp.text().await.unwrap_or_default();
    let message = error_message(&body);
    // Structured codes (e.g. ChatGPT plan limits) say more than the status.
    if let Ok(v) = serde_json::from_str::<Value>(&body) {
        if v.pointer("/error/code")
            .and_then(Value::as_str)
            .is_some_and(|c| c.starts_with("subscription_sharing"))
        {
            return responses::failure(&v);
        }
    }
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => LlmError::Auth(message),
        StatusCode::TOO_MANY_REQUESTS => LlmError::RateLimited {
            retry_after,
            message,
        },
        s if s.as_u16() == 529 || s == StatusCode::SERVICE_UNAVAILABLE => {
            LlmError::Overloaded(message)
        }
        s if s.is_server_error() => LlmError::Server {
            status: s.as_u16(),
            message,
        },
        s => LlmError::Request {
            status: s.as_u16(),
            message,
        },
    }
}

/// Sends a streaming request and feeds each SSE event to `on_event`
/// (event name, data). `on_event` returns true to stop reading.
async fn send_sse(
    builder: RequestBuilder,
    cancel: &CancellationToken,
    mut on_event: impl FnMut(Option<&str>, &str) -> Result<bool, LlmError>,
) -> Result<(), LlmError> {
    let send = builder.header("accept", "text/event-stream").send();
    let resp = tokio::select! {
        _ = cancel.cancelled() => return Err(LlmError::Cancelled),
        r = send => r.map_err(|e| LlmError::Network(e.to_string()))?,
    };
    if !resp.status().is_success() {
        return Err(status_error(resp).await);
    }
    let mut parser = SseParser::default();
    let mut body = resp.bytes_stream();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(LlmError::Cancelled),
            c = body.next() => c,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|e| LlmError::Network(e.to_string()))?;
        for event in parser.push(&chunk) {
            if on_event(event.event.as_deref(), &event.data)? {
                return Ok(());
            }
        }
    }
    for event in parser.finish() {
        if on_event(event.event.as_deref(), &event.data)? {
            return Ok(());
        }
    }
    Ok(())
}

/// The concatenated text of an assistant message.
pub fn text_of(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
pub(crate) mod testing {
    //! A one-shot HTTP server that returns a canned SSE body and records the
    //! request, so clients are tested over real HTTP.

    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub struct Captured {
        pub head: String,
        pub body: serde_json::Value,
    }

    pub async fn serve_once(
        status: &str,
        response_body: &str,
    ) -> (String, Arc<Mutex<Option<Captured>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let captured = Arc::new(Mutex::new(None));
        let slot = captured.clone();
        let status = status.to_string();
        let response_body = response_body.to_string();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let (head_end, content_length) = loop {
                let n = sock.read(&mut tmp).await.unwrap();
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map(|v| v.trim().parse::<usize>().unwrap())
                        .unwrap_or(0);
                    break (pos + 4, len);
                }
            };
            while buf.len() < head_end + content_length {
                let n = sock.read(&mut tmp).await.unwrap();
                buf.extend_from_slice(&tmp[..n]);
            }
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let body = serde_json::from_slice(&buf[head_end..head_end + content_length])
                .unwrap_or(serde_json::Value::Null);
            *slot.lock().unwrap() = Some(Captured { head, body });
            let reply = format!(
                "HTTP/1.1 {status}\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{response_body}"
            );
            sock.write_all(reply.as_bytes()).await.unwrap();
            sock.shutdown().await.ok();
        });
        (url, captured)
    }
}
