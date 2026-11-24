//! Engines run one model request; `run_gated*` adds shape checks, repair,
//! escalation and field streaming on top.

pub mod cli;
pub mod router;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

use crate::config::ModelEntry;
use crate::store::Store;
use crate::stream::{FieldStreamer, ToolContent, ToolStart};
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
    /// Content for a running tool call: a file diff or text output.
    ToolUpdate {
        id: String,
        content: ToolContent,
    },
    ToolEnd {
        id: String,
        ok: bool,
    },
}

/// Per-request context the runtime passes to engines.
#[derive(Debug, Clone, Default)]
pub struct RunContext {
    /// Cancelled when the user stops the turn.
    pub cancel: CancellationToken,
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
    pub ctx: RunContext,
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
    // What the user has seen so far from earlier attempts of this call.
    let mut seen = String::new();
    // Wraps the sink so raw JSON never reaches the user, only the decoded
    // `field`. A retry that repeats `seen` stays hidden; one that differs is
    // shown in full as a new paragraph. Returns the sink and the attempt's text.
    let field_sink = |seen: &str| -> (StreamSink, Arc<Mutex<String>>) {
        let decoded = Arc::new(Mutex::new(String::new()));
        let Some(field) = field else {
            return (sink.clone(), decoded);
        };
        let streamer = Mutex::new(FieldStreamer::new(field));
        let seen = seen.to_string();
        let diverged = Mutex::new(seen.is_empty());
        let inner = sink.clone();
        let record = decoded.clone();
        let wrapped: StreamSink = Arc::new(move |ev| match ev {
            StreamEvent::TextDelta(t) => {
                let out = streamer.lock().unwrap().push(&t);
                if out.is_empty() {
                    return;
                }
                let mut all = record.lock().unwrap();
                all.push_str(&out);
                let mut diverged = diverged.lock().unwrap();
                if *diverged {
                    inner(StreamEvent::TextDelta(out));
                } else if !seen.starts_with(all.as_str()) {
                    *diverged = true;
                    inner(StreamEvent::TextDelta(format!("\n\n{all}")));
                }
            }
            other => inner(other),
        });
        (wrapped, decoded)
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

        let (attempt_sink, mut shown) = field_sink(&seen);
        let raw = engine.run_text(&req, attempt_sink).await?;
        remember(&mut seen, &shown);
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
            let (repair_sink, repair_shown) = field_sink(&seen);
            let repaired = engine.run_text(&repair, repair_sink).await?;
            remember(&mut seen, &repair_shown);
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

/// Keeps the longest text the user has seen, so a repeat stays hidden.
fn remember(seen: &mut String, attempt: &Mutex<String>) {
    let attempt = attempt.lock().unwrap();
    if !attempt.is_empty() && !seen.starts_with(attempt.as_str()) {
        *seen = attempt.clone();
    }
}

/// Doubles backslashes that don't start a JSON escape, such as LaTeX `\(`
/// that models write inside strings.
fn fix_escapes(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 8);
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u') => {
                out.push(c);
                out.push(chars.next().unwrap());
            }
            _ => out.push_str("\\\\"),
        }
    }
    out
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
    let parsed: T = serde_json::from_str(candidate)
        .or_else(|err| serde_json::from_str(&fix_escapes(candidate)).map_err(|_| err))
        .map_err(|err| {
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
    #[test]
    fn tolerates_latex_backslashes() {
        let raw = r#"{"reply": "sum \( 4(0.5) \) is \"ok\"\n"}"#;
        let v: serde_json::Value = serde_json::from_str(&super::fix_escapes(raw)).unwrap();
        assert_eq!(v["reply"], "sum \\( 4(0.5) \\) is \"ok\"\n");
    }

    use super::*;

    #[test]
    fn extracts_json_from_fenced_block() {
        let raw = "Sure:\n```json\n{\"a\": 1}\n```";
        assert_eq!(extract_json_object(raw), "{\"a\": 1}");
    }
}
