//! The loop from `docs/theory.md`. Section numbers refer to that document.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::config::{Config, ModelEntry};
use crate::decide::{Decide, Decision, Provider, Question};
use crate::domain::{
    CandidateProposal, Intent, JudgmentVerdict, LogReply, Map, MapChange, MapProposal,
    OnboardingDecision, Part, PartStatus, PendingCandidate, Session, SessionMode,
};
use crate::engine::{
    Engine, EngineRequest, EngineTier, StreamEvent, StreamSink, TierModels, ToolAccess, run_gated,
    run_gated_streamed,
};
use crate::i18n;
use crate::secrets::Secret;
use crate::store::Store;
use crate::understanding::EventKind;
use crate::validate::{
    check_candidate, check_change, check_log_reply, check_map, check_onboarding, check_verdict,
};
use crate::workspace::WorkspaceSnapshot;

mod concepts;
pub mod eval;
mod evidence;
mod gate;
mod handoff;
mod onboarding;
mod provenance;
mod record;
mod review;
mod skips;

pub struct TurnInput {
    pub text: String,
    pub files: Vec<PathBuf>,
}

pub struct TurnOutput {
    pub reply: String,
    pub plan_changed: bool,
}

impl TurnOutput {
    fn text(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            plan_changed: false,
        }
    }

    fn changed(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            plan_changed: true,
        }
    }
}

pub const COMMANDS_HELP: &str = "Commands: `/plan`, `/summary <text>`, `/handoff [notes]`, `/approve`, `/object <reason>`, `/known`, `/delegate [notes]`, `/accept`, `/reject`, `/replan [what changed]`, `/next`, `/focus <id|number|prefix>`, `/map`, `/why`, `/coverage`.";

/// No decision model: the LLM decides each message's step itself.
struct NoDecider;

impl Decide for NoDecider {
    fn decide<'a>(
        &'a self,
        _input: &'a str,
        _questions: &'a [Question],
        _prefer: Option<&'a str>,
    ) -> futures_util::future::BoxFuture<'a, Result<Option<Decision>>> {
        Box::pin(async { Ok(None) })
    }
}

/// Decides whether the next candidate is flawed (§5.3).
fn random_flaw() -> bool {
    uuid::Uuid::new_v4().as_bytes()[0] & 1 == 1
}

pub struct Runtime {
    store: Arc<Store>,
    engine: Arc<dyn Engine>,
    flaw_coin: fn() -> bool,
    /// Rolls whether a review is checked (A8).
    check_roll: fn() -> f64,
    /// Read on every use, so config edits apply without a restart.
    config: fn() -> Config,
    /// First pass over chat messages (see decide.rs).
    decider: Arc<dyn Decide>,
}

/// The model pickers' state for one session.
pub struct ModelChoices {
    pub catalog: Vec<ModelEntry>,
    pub system1: Option<String>,
    pub system2: Option<String>,
    /// Main model effort: `default`, `low`, `medium` or `high`.
    pub effort: String,
    /// Fast model effort, the same levels.
    pub fast_effort: String,
    /// Decision services with a key, in auto order.
    pub decision_available: Vec<Provider>,
    pub decisions_on: bool,
    /// The service in use when on: the thread's pick, else the first available.
    pub decision_model: Option<Provider>,
    /// Review checks: `off`, `light` or `normal` (§13).
    pub checks: String,
}

pub const CHECKS: [&str; 3] = ["off", "light", "normal"];

/// Main model effort levels offered in the picker.
pub const EFFORTS: [&str; 4] = ["default", "low", "medium", "high"];

/// A value from one of the thread's pickers or switches.
pub enum OptionValue {
    Id(String),
    Bool(bool),
}

pub const NO_MODEL: &str = "No model is set up yet. Sign in with your ChatGPT account \
     (`pathfinder auth login chatgpt` in a terminal), or add an API key (`pathfinder auth set`).";

impl Runtime {
    pub fn new(store: Arc<Store>, engine: Arc<dyn Engine>) -> Self {
        Self {
            store,
            engine,
            flaw_coin: random_flaw,
            check_roll: review::roll,
            config: Config::load,
            decider: Arc::new(NoDecider),
        }
    }

    pub fn with_decider(mut self, decider: Arc<dyn Decide>) -> Self {
        self.decider = decider;
        self
    }

    fn models(&self, session: &Session) -> Result<TierModels> {
        let config = (self.config)();
        let pick = |choice: &Option<String>, system2| {
            config
                .pick(choice.as_deref(), system2)
                .ok_or_else(|| anyhow!(NO_MODEL))
        };
        let mut system1 = pick(&session.system1, false)?;
        let mut system2 = pick(&session.system2, true)?;
        if let Some(effort) = &session.fast_effort {
            system1.effort = Some(effort.clone());
        }
        if let Some(effort) = &session.effort {
            system2.effort = Some(effort.clone());
        }
        Ok(TierModels { system1, system2 })
    }

    pub fn model_choices(&self, session: &Session) -> ModelChoices {
        let config = (self.config)();
        let available = self.decider.available();
        let decision_model = session
            .decision_model
            .as_deref()
            .and_then(Provider::from_id)
            .filter(|p| available.contains(p))
            .or_else(|| available.first().copied());
        ModelChoices {
            catalog: config.catalog(),
            system1: config.pick(session.system1.as_deref(), false).map(|m| m.id),
            system2: config.pick(session.system2.as_deref(), true).map(|m| m.id),
            effort: session.effort.clone().unwrap_or_else(|| "default".into()),
            fast_effort: session
                .fast_effort
                .clone()
                .unwrap_or_else(|| "default".into()),
            decisions_on: !session.decisions_off && decision_model.is_some(),
            decision_available: available,
            decision_model,
            checks: session.checks.clone().unwrap_or_else(|| "light".into()),
        }
    }

    /// Applies one picker or switch from the thread's toolbar.
    pub fn set_option(
        &self,
        session_id: &str,
        option: &str,
        value: OptionValue,
    ) -> Result<Session> {
        let mut session = self
            .store
            .load_session(session_id)?
            .ok_or_else(|| anyhow!("unknown session `{session_id}`"))?;
        match (option, value) {
            (crate::options::FAST_MODEL | crate::options::MAIN_MODEL, OptionValue::Id(id)) => {
                if (self.config)().entry(&id).is_none() {
                    return Err(anyhow!("unknown model `{id}`"));
                }
                let slot = if option == crate::options::MAIN_MODEL {
                    &mut session.system2
                } else {
                    &mut session.system1
                };
                *slot = Some(id);
            }
            (crate::options::MAIN_EFFORT | crate::options::FAST_EFFORT, OptionValue::Id(level)) => {
                if !EFFORTS.contains(&level.as_str()) {
                    return Err(anyhow!("unknown effort `{level}`"));
                }
                let slot = if option == crate::options::MAIN_EFFORT {
                    &mut session.effort
                } else {
                    &mut session.fast_effort
                };
                *slot = (level != "default").then_some(level);
            }
            // A switch, or an Off/On dropdown where switches aren't supported.
            (crate::options::DECISIONS, OptionValue::Bool(on)) => session.decisions_off = !on,
            (crate::options::DECISIONS, OptionValue::Id(v)) if v == "on" || v == "off" => {
                session.decisions_off = v == "off";
            }
            (crate::options::DECISION_MODEL, OptionValue::Id(id)) => {
                if Provider::from_id(&id).is_none() {
                    return Err(anyhow!("unknown decision model `{id}`"));
                }
                session.decision_model = Some(id);
                session.decisions_off = false;
            }
            (crate::options::CHECKS, OptionValue::Id(level))
                if CHECKS.contains(&level.as_str()) =>
            {
                session.checks = (level != "light").then_some(level);
            }
            (other, _) => return Err(anyhow!("unknown option or value for `{other}`")),
        }
        self.store.save_session(&session)?;
        Ok(session)
    }

    /// Sets a session's System 1 or System 2 model from the picker.
    #[cfg(test)]
    pub fn set_model(&self, session_id: &str, system2: bool, entry_id: &str) -> Result<Session> {
        let option = if system2 {
            crate::options::MAIN_MODEL
        } else {
            crate::options::FAST_MODEL
        };
        self.set_option(session_id, option, OptionValue::Id(entry_id.into()))
    }

    pub fn create_session(&self, id: String, cwd: PathBuf) -> Result<Session> {
        let session = Session::new(id, cwd);
        self.store.save_session(&session)?;
        Ok(session)
    }

    pub fn load_session(&self, id: &str) -> Result<Option<Session>> {
        self.store.load_session(id)
    }

    /// The threads opened in `cwd`, most recently active first.
    pub fn sessions_in(&self, cwd: &std::path::Path) -> Vec<Session> {
        let mut all: Vec<Session> = self
            .store
            .all_sessions()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.cwd == cwd)
            .collect();
        all.reverse();
        all
    }

    /// Saves what the client was shown this turn (serialized updates).
    pub fn record(&self, id: &str, updates: &[String]) -> Result<()> {
        self.store.append_transcript(id, updates)
    }

    pub fn transcript(&self, id: &str) -> Result<Vec<String>> {
        self.store.load_transcript(id)
    }

    pub async fn handle_turn(
        &self,
        session_id: &str,
        input: TurnInput,
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let mut session = self
            .store
            .load_session(session_id)?
            .ok_or_else(|| anyhow!("unknown session `{session_id}`"))?;

        self.migrate_pending_key(&mut session);
        let text = input.text.trim();
        let files = &input.files;

        // Usage events: what kind of turn, how long, which tools (by kind).
        let started = std::time::Instant::now();
        let had_candidate = session.pending_candidate.is_some();
        let tools = Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::<
            pathfinder_schema::Tool,
            u32,
        >::new()));
        let sink: StreamSink = {
            let tools = tools.clone();
            let inner = sink;
            Arc::new(move |event: StreamEvent| {
                if let StreamEvent::ToolStart(t) = &event {
                    *tools.lock().unwrap().entry(tool_kind(t.kind)).or_default() += 1;
                }
                inner(event)
            })
        };
        let (output, counters) = crate::telemetry::with_turn_counters(self.dispatch_turn(
            &mut session,
            text,
            files,
            sink,
        ))
        .await;
        if crate::telemetry::enabled() {
            crate::telemetry::set_locale(session.language.as_deref().unwrap_or(""));
            let (route, model) = self
                .models(&session)
                .map(|m| {
                    let route = serde_json::to_value(m.system2.route)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default();
                    let model = if m.system2.model.is_empty() {
                        m.system2.id.clone()
                    } else {
                        m.system2.model.clone()
                    };
                    (route, model)
                })
                .unwrap_or_default();
            let model = if pathfinder_schema::is_token(&model, 96) {
                model
            } else {
                "other".into()
            };
            let step = turn_step(text, &session, had_candidate);
            crate::telemetry::emit(pathfinder_schema::Event::Turn {
                route,
                model,
                ms: started.elapsed().as_millis() as u64,
                step,
                steps: counters
                    .model_calls
                    .load(std::sync::atomic::Ordering::Relaxed),
                tools: tools.lock().unwrap().clone(),
                retries: counters.retries.load(std::sync::atomic::Ordering::Relaxed),
                failed: output.is_err(),
            });
            if let Err(e) = &output
                && let Some(code) = crate::telemetry::error_code(e)
            {
                crate::telemetry::emit(pathfinder_schema::Event::Error { code });
            }
        }
        let output = output?;

        self.retire_stale_secrets(&session);
        self.store.save_session(&session)?;
        Ok(output)
    }

    /// One message, by the thread's mode or the command it starts with.
    async fn dispatch_turn(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let output = if text.starts_with('/') {
            self.handle_command(session, text, files, sink).await?
        } else {
            match session.mode {
                SessionMode::Onboarding => {
                    self.handle_onboarding(session, text, files, sink).await?
                }
                SessionMode::Working => self.handle_chat(session, text, files, sink).await?,
                SessionMode::Done => {
                    self.handle_opening(session, text, files, sink, false)
                        .await?
                }
            }
        };
        Ok(output)
    }

    async fn handle_command(
        &self,
        session: &mut Session,
        raw: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let mut words = raw.splitn(2, char::is_whitespace);
        let cmd = words.next().unwrap_or("");
        let arg = words.next().unwrap_or("").trim();

        match cmd {
            "/map" => Ok(TurnOutput::text(render_map_and_pending(session))),
            "/plan" => {
                if !arg.is_empty() {
                    session.push_turn("user", arg);
                }
                self.build_map(session, files, sink).await
            }
            "/summary" => self.handle_summary(session, arg, files, sink).await,
            "/handoff" => self.handle_handoff(session, arg, files, sink).await,
            "/object" => self.handle_object(session, arg, files, sink).await,
            "/approve" => self.handle_approve(session).await,
            "/known" => self.handle_known(session, Some(arg)),
            "/delegate" => self.handle_delegate(session, arg, files, sink).await,
            "/accept" => self.handle_accept(session).await,
            "/why" => Ok(TurnOutput::text(self.why(session))),
            "/coverage" => Ok(TurnOutput::text(self.coverage_report(session))),
            "/reject" => Ok(match session.pending_change.take() {
                Some(_) => TurnOutput::text(i18n::line(
                    session.language.as_deref(),
                    i18n::Line::ChangeRejected,
                )),
                None => TurnOutput::text(i18n::line(
                    session.language.as_deref(),
                    i18n::Line::NoChangeWaiting,
                )),
            }),
            "/replan" => self.handle_replan(session, arg, files, sink).await,
            "/next" => Ok(self.handle_next(session)),
            "/focus" => Ok(self.handle_focus(session, arg)),
            _ => Ok(TurnOutput::text(COMMANDS_HELP)),
        }
    }
}

/// Back to plain chat after a quick check: the evidence stays in the record.
fn end_quick_check(session: &mut Session) {
    session.map = Map::default();
    session.active = None;
    session.pending_candidate = None;
    session.pending_change = None;
    session.goal = None;
    session.quick_check = false;
    session.exam = false;
    session.mode = SessionMode::Onboarding;
}

/// The exam questions shown so far in this exam, so the next one differs.
fn exam_questions_asked(session: &Session) -> Vec<String> {
    session
        .pending_candidate
        .iter()
        .map(|p| p.candidate.clone())
        .chain(
            session
                .history
                .iter()
                .filter(|t| t.role == "assistant")
                .filter(|t| {
                    [Some("de"), None].iter().any(|lang| {
                        t.content
                            .starts_with(i18n::line(*lang, i18n::Line::ExamQuestionFor))
                    })
                })
                .filter_map(|t| {
                    let (_, q) = t.content.split_once("**:\n\n")?;
                    let q = q.rsplit_once("\n\n").map_or(q, |(q, _)| q);
                    Some(q.trim().to_string())
                }),
        )
        .collect()
}

/// The decision model's step for a verdict on a waiting candidate.
const VERDICT: &str = "verdict";

/// While a candidate waits, a log, move or skip needs this much to skip
/// the judge.
const CONFIDENT_STEP: f64 = 0.9;

/// Below this, the decision model's step is ignored and the LLM decides.
const DECISION_THRESHOLD: f64 = 0.6;

const CANDIDATE_HIDDEN: &str = "(candidate hidden while the user judges it)";

pub(crate) const REVIEW_PROMPT: &str =
    "\n\nReview the result: `/approve`, or `/object <what's wrong and why>` to send it back.";

/// Makes sure the user has seen `full`: if streaming didn't show it, send it
/// now, so it stays ahead of anything that follows.
fn flush(sink: &StreamSink, full: &str, shown: &str) {
    if shown.is_empty() || shown.trim() != full.trim() {
        sink(StreamEvent::TextDelta(full.to_string()));
    }
}

/// A part as you start on it. The command hint shows only with the first part.
fn render_part(part: &Part, hint: bool, lang: Option<&str>) -> String {
    let mut out = format!("**{}** (`{}`)", part.title, part.id);
    if !part.note.trim().is_empty() {
        out.push_str(&format!("\n{}", part.note.trim()));
    }
    if part.status != PartStatus::Open {
        out.push_str(&format!("\nStatus: {}.", part.status.label()));
    }
    if hint && part.status == PartStatus::Open {
        out.push_str(&format!("\n\n{}", i18n::line(lang, i18n::Line::PartHint)));
    }
    out
}

fn describe_part(part: &Part) -> String {
    let mut out = format!("Part: {} (`{}`)", part.title, part.id);
    if !part.note.trim().is_empty() {
        out.push_str(&format!("\nNote: {}", part.note.trim()));
    }
    if !part.log.is_empty() {
        out.push_str(&format!("\nUser's log:\n- {}", part.log.join("\n- ")));
    }
    if let Some(summary) = &part.summary {
        out.push_str(&format!("\nUser's summary: {summary}"));
    }
    out
}

/// `+` adds a part, `~` updates one already on the map.
fn render_change(change: &MapChange, map: &Map, lang: Option<&str>) -> String {
    let mut lines = vec![format!(
        "{} {}",
        i18n::line(lang, i18n::Line::ProposedChange),
        change.brief
    )];
    if let Some(goal) = change.goal.as_deref().filter(|g| !g.trim().is_empty()) {
        lines.push(format!("Goal → {goal}"));
    }
    for p in &change.parts {
        let deps = if p.depends_on.is_empty() {
            String::new()
        } else {
            format!(
                " ({})",
                i18n::line(lang, i18n::Line::Needs).replace("{deps}", &p.depends_on.join(", "))
            )
        };
        let mark = if map.get(&p.id).is_some() { "~" } else { "+" };
        lines.push(format!("{mark} `{}` {}{deps}", p.id, p.title));
    }
    for id in &change.remove {
        lines.push(format!("- remove `{id}`"));
    }
    lines.push(i18n::line(lang, i18n::Line::AcceptReject).to_string());
    lines.join("\n")
}

fn render_map_and_pending(session: &Session) -> String {
    let lang = session.language.as_deref();
    let mut out = if session.map.parts.is_empty() {
        session.map.render(None)
    } else {
        session.map.mermaid(session.active.as_deref(), |s| {
            i18n::part_status(lang, s).to_string()
        })
    };
    if let Some(change) = &session.pending_change {
        out.push_str("\n\n");
        out.push_str(&render_change(
            change,
            &session.map,
            session.language.as_deref(),
        ));
    }
    out
}

fn format_history(session: &Session) -> String {
    let start = session.history.len().saturating_sub(12);
    format_turns(&session.history[start..])
}

fn format_turns(turns: &[crate::domain::TurnRecord]) -> String {
    if turns.is_empty() {
        return "(none)".to_string();
    }
    turns
        .iter()
        .map(|t| format!("{}: {}", t.role, t.content))
        .collect::<Vec<_>>()
        .join("\n")
}

const ONBOARDING_SYSTEM: &str = r#"You run onboarding for Pathfinder, a work agent that makes sure the user understands a task before the work is handed to an agent.

Onboarding only sets the goal. Right after it, Pathfinder sketches a rough map of what the task depends on. The map has no size limit and grows and changes during the work, so scope, depth and details are settled there, not here. Do not plan, teach or do the task.

First decide what kind of message this is, by what the user means, in any wording or language; the examples below only illustrate. Pathfinder is above all the user's everyday assistant; most messages are questions, and answering is the default:
- A question or request (set `question` true, `ready_to_map` false, `goal` null, `reply` ""): anything they want answered, explained, summarised or looked up ("what is a phasor?", "explain this slide", "how do I derive this?", "what do I need to know from slide set 4?", "give me the solution to exercise 1a", "what's the capital of Peru?"). Also their own work shown for review ("I solved exercise 2, is this right?", a photo of their solution): Pathfinder reviews it. Asking for a solution is a question, not a task: the user decides. Set `offer_check` to true when it's about understanding something, false for lookups and chit-chat.
- A quick check (`check` and `ready_to_map` true): the user signals, however they put it, that they now understand what was just explained (e.g. "ah, now it clicks"). That claim is the moment to check it. The goal starts with "Check" (in their language: "Prüfen, ...") and names the topic just explained, narrowly. Thanks or an acknowledgement without any claim of understanding is chit-chat: a question with `offer_check` false.
- An exam (`exam` and `ready_to_map` true): they want to be tested or examined on a topic, in any form (e.g. practising for an oral exam, being quizzed on a chapter). The goal starts with "Examine" (in their language: "Abfragen, ...") and names the topic: the one they name, else the one just discussed.
- A task (`ready_to_map` true, `check` and `exam` false): only when they want to work through something themselves ("I want to solve exercise 1 myself, help me without giving it away", "ich will das Kapitel richtig lernen") or want work done in the workspace (build, fix, change something). Not for questions about a topic, however big.

Always set `language` to the two-letter code of the language the user writes in (e.g. "de", "en"), and write `goal` and `reply` in that language.

Otherwise it's onboarding for a task. Aim to finish in this turn:
- If the user has named something to do, build, fix, understand or learn, the goal is clear: set `ready_to_map` to true and write `goal` as one sentence starting with a verb, in their words but not in first person ("Learn how neurons in neural networks work"). Breadth or vagueness ("learn how X works") is fine; the map handles it.
- Ask one short question only when you can't tell what the task is at all (e.g. "orient yourself", "help me with my thesis"). If the workspace has content, ground it: say in one sentence what the workspace is (from the snapshot and any files worth reading, such as a README), then ask what they want to do there. If it's empty, just ask. Never ask the user to confirm, choose a scope, or say what they want to be able to explain.
- If the user says what they already know or don't ("I'm new to this", "I know the basics"), keep it in `goal` (e.g. "... starting from the basics").

`reply`: when ready, just the goal in one sentence (the map is shown right after it; don't announce it). When asking, at most two sentences.

You get a workspace snapshot. If a file could answer something, read it before writing your JSON, since `reply` ends your turn. Don't read files when the snapshot and the user's words already settle it.

Return ONLY a JSON object:
{"reply": "...", "ready_to_map": true, "goal": "...", "question": false, "offer_check": false, "check": false, "exam": false, "language": "en"}"#;

const ANSWER_SYSTEM: &str = r#"You are Pathfinder, the user's assistant, answering a question directly. Questions may be about their code, documents, research, their own work, or anything else.

- Answer what they asked, correctly, like a strong tutor or colleague, at the depth they asked for. For a calculation or derivation, go step by step and keep every step. For a concept, give the idea, the precise statement and a short example if it helps. For a broad topic ("how does a transformer work?"), give the core idea and the few relations that matter most, in a screen or two, then name in one line what you could go into next; don't write the whole chapter.
- When they show their own work (a solution, a derivation, code), review it like a tutor marking it: go through it step by step, say which steps are right, and for each mistake say where it is, what's wrong and which principle fixes it. Give the corrected result only after that, briefly.
- Read attached files (documents, papers, slides, screenshots) and workspace files when they bear on the question. If the question is about a figure, diagram, slide or page, open it and look at it before answering; never say you can't see a file that is attached or in the workspace.
- Write math as plain text or Unicode (Z = R + jωL, F(s) = ∫₀^∞ f(t)·e^(−st) dt, ∇·E = ρ/ε₀), never LaTeX: the chat can't render it.
- Reply in the language the user writes in.
- Don't offer to check their understanding or ask follow-up questions unless the question is ambiguous; Pathfinder adds that itself."#;

const MAP_SYSTEM: &str = r#"You sketch the map for Pathfinder: what the task depends on.

- A part is something the task depends on: a piece of the work, or of the understanding needed to judge the work. It is not a lesson and not an assignment: the user decides during the work which parts they do themselves and which go to an agent.
- No size limit. Use as many parts as the task needs and no more; a small task can be a single part.
- `depends_on` lists parts that must be understood or done before this one makes sense. No cycles.
- `id`: a short descriptive slug the user types in commands: lowercase words joined by hyphens, at most 12 characters (e.g. `weights`, `fwd-pass`, `loss`). Unique. It names what the part is actually about, not a broad field: `colliders`, not `probability`. Users skip parts by their names.
- Start where the user is: if the goal or conversation says what they already know or don't, shape the parts for that (e.g. include the basics for a newcomer). Keep parts they say they know on the map; they can mark them known themselves.
- `note`: one line on what the part covers. `brief`: what the map covers, in one or two sentences.
- A goal that is a check of a topic just explained in the conversation ("Check ...", "Prüfen, ...") gets one part for that topic, two at most: the user explains it and judges a candidate.
- Never give results away: no answers, computed values or intermediate results (a cutoff frequency, a derivative, "≈ 0.157") in the brief, notes or titles. The user works them out; for an exercise, say what each part is about, not what it comes to.
- `concepts`: the ideas the parts rest on, the record Pathfinder keeps of what the user has shown they understand. Each has an `id` (a lowercase hyphenated slug naming the idea itself, e.g. `d-separation`, `chain-rule`), a short `label`, and `needs`: the concept ids it builds on. Include a concept the task needs even if no part covers it (a prerequisite the user may lack). Reuse ids of concepts already recorded in this workspace when it's the same idea.
- Each part lists its 1–4 core `concepts` (ids from `concepts`): what someone must understand to judge an agent's work on that part. Usually one or two.

Return ONLY a JSON object:
{"brief": "...", "concepts": [{"id": "weighted-sum", "label": "Weighted sum of inputs", "needs": []}], "parts": [{"id": "weights", "title": "...", "note": "...", "depends_on": [], "concepts": ["weighted-sum"]}]}"#;

const CHANGE_SYSTEM: &str = r#"You propose a change to Pathfinder's map of what the task depends on, because something changed. The user accepts or rejects it.

- `parts`: parts to add, or existing parts to update (same id), each with id, title, note, depends_on and its 1–4 core `concepts` (concept ids). New concepts go in `concepts` (id, label, needs), as in the map. New ids: `id`: a short descriptive slug the user types in commands: lowercase words joined by hyphens, at most 12 characters (e.g. `weights`, `fwd-pass`, `loss`). Unique.
- `remove`: ids of parts to drop. Only parts with no work or outcome can be removed.
- No size limit. No cycles.

- `goal`: only when what changed alters what the task is (a new purpose or scope): the new goal as one sentence starting with a verb, not in first person. Otherwise null.
`brief`: one sentence to the user, saying "you", on what changes and why.

Return ONLY a JSON object:
{"brief": "...", "goal": null, "parts": [], "remove": [], "concepts": []}"#;

const LOG_SYSTEM: &str = r#"The user is working on one part of a task and chatting with Pathfinder as they go. Most messages are their log: their thinking and the reasons behind their steps and choices, or a question. The log is evidence of their understanding.

First decide what the latest message is (`intent`):
- `log`: thinking, reasoning, an attempt or a question. The default; when unsure, it's `log`.
- `summary`: they offer their explanation of the part as a whole, ready to be checked ("I think I get it: ...", "so to sum up, ..."). Not a single point or a question.
- `known`: they clearly say they already know this whole part and want to skip it. Knowing some of it isn't enough.
- `move`: they want to go on to the next part, or to a named one (put its id in `part`). For `known`, also put a part they name to go to in `part`.
- `accept` / `reject`: they answer a waiting map change ("yes, add it", "no, keep the map").
- `replan`: they ask to change the plan, or say something that changes the whole task's scope or purpose (a new constraint or aim).
For anything but `log`, set `reply` to "" and `map_change` to null: Pathfinder takes the step.

For `log`, reply briefly (math as plain text or Unicode, never LaTeX): answer what they ask, or point out what the entry reveals. If they ask whether something is right, say whether it is, even when it belongs to another part. Don't repeat what you already said in the conversation. Do not do the work for them and do not write their explanation of the part for them.

The user decides which parts they do themselves and which go to an agent. Don't suggest it. If they ask you to just do the work, say plainly how: `/handoff` gives an understood part to the agent and they review the result; `/delegate` hands it off without understanding it, recorded as such. Then carry on with the part unless they choose one.

If a `log` message reveals the map is wrong (a missing dependency, a part that doesn't belong, a wrong edge, or a new constraint that changes the task's scope), propose a change in `map_change`; the user accepts or rejects it. Otherwise set it to null. A change has `brief` (one sentence to the user, saying "you"), `goal` (only when the message changes what the task is, e.g. a new purpose or scope: the new goal as one sentence starting with a verb, not in first person; otherwise null), `parts` (to add, or update by id: id, title, note, depends_on; new ids are short hyphenated slugs of at most 12 characters) and `remove` (ids of parts with no work yet).

If the message shows a gap in something this part depends on, especially a part they delegated or claimed to know (see the map's statuses), explain it in plain words, and propose a small part for it in `map_change` so the gap is on the map.

For `log`, also record what the message is about, without judging it:
- `concepts`: the concepts it touches, as concept ids from the lists (from any part, not part ids). A short name only for an idea on neither list. Empty if none.
- In `misconception`, `concept` is likewise a concept id from the lists when one fits.
- `misconception`: only if the message plainly states a wrong belief (not a question, not a guess they're testing): `{"concept": id or name, "statement": the belief in one sentence}`. Otherwise null. A separate check confirms it; it never changes what you reply.

Call `read` before writing your JSON if a file matters: `reply` ends your turn. If they ask about a figure, diagram, slide or page, open the file and look at that page first (with `view`, or your file tool for PDFs and images); never say you can't see a file that is attached or in the workspace.

Return ONLY a JSON object:
{"intent": "log", "part": null, "reply": "...", "map_change": null, "concepts": [], "misconception": null}
or, with a change:
{"intent": "log", "part": null, "reply": "...", "map_change": {"brief": "...", "goal": null, "parts": [], "remove": []}, "concepts": [], "misconception": null}"#;

const CANDIDATE_SYSTEM: &str = r#"You make a candidate for the user to judge. Their judgment decides whether they understand the part well enough to direct an agent and tell whether its result is right.

A candidate is a concrete approach or result for this part, as an agent might produce it. Pick whichever fits the part. Stay within this part: use only ideas from this part, the parts it depends on and the user's log, at the level they're working at. Don't bring in terms or steps from later parts; the user may not have met them yet. Keep it short enough to judge in a minute or two: a short paragraph, a few steps or a small snippet, under 150 words. Don't restate the user's summary back to them. Write math as plain text or Unicode (z = 2x₁ − x₂), never LaTeX.

You are told whether to make it correct or flawed. A flawed candidate contains exactly one real flaw that matters for the goal and that someone who understands the part would catch. It must look plausible: never hint at the flaw.

`key_reason` is hidden from the user: for a correct candidate, why it is right; for a flawed one, what the flaw is and why it matters.

Return ONLY a JSON object:
{"candidate": "...", "key_reason": "..."}"#;

const EXAM_QUESTION_SYSTEM: &str = r#"You test the user's understanding of one part of a topic, the way a good oral examiner would. Ask one question: open, so that the answer needs an explanation in the user's own words (why something holds, how a solution path goes, what happens if something changes, how two ideas connect), not a number or a definition to recite. One question, one or two sentences, answerable in a few minutes of talking. Stay within this part and what it rests on; use the material in the workspace when it helps you ask about what it actually covers. Write in the user's language; math as plain text or Unicode, never LaTeX.

`candidate`: the question. `key_reason`: hidden from the user until they answer: the points a good answer contains, and the mistakes to watch for.

Return ONLY a JSON object:
{"candidate": "...", "key_reason": "..."}"#;

const EXAM_JUDGE_SYSTEM: &str = r#"You grade the user's answer to a test question, the way a fair oral examiner would. You have the question, the hidden key (the points a good answer contains) and the user's answer.

If the message isn't an attempt to answer (a question back, a request to stop or to move on, "I don't know what you mean"), set `is_judgment` to false and `feedback` to "". "I don't know" is an answer: it fails.

`pass` is true when the answer gets the substance right in the user's own words: the key points that matter are there and the reasoning holds. It doesn't need to be complete or polished; a wrong statement or a missing core idea fails it.

`feedback`, in the user's language, as a fair examiner would give it after the answer: what was good, what was missing or wrong and why, then the answer someone who knows the topic well would have given, in a few sentences. Math as plain text or Unicode, never LaTeX.

`misconception`: a wrong belief the answer plainly states, in one sentence, else null.

Return ONLY a JSON object:
{"is_judgment": true, "pass": false, "feedback": "...", "misconception": null}"#;

const JUDGE_SYSTEM: &str = r#"Decide whether the user's judgment of a candidate shows they could judge an agent's work on this part.

You have the candidate, the hidden key (correct or flawed, and why) and the user's judgment.

`pass` is true only if their verdict matches the key and their reason does too: for a flawed candidate they name the actual flaw, not a different one; for a correct one they say why it holds, not just that it looks fine.

`feedback`: reveal the key and say what their judgment got right or missed, in the language the user writes in.

`misconception`: when they fail because they hold a wrong belief (not just a slip or "don't know"), state that belief in one sentence; otherwise null.

If the message isn't a verdict on the candidate at all (a question about it or the part, a thought for their log, a request to skip or move on), set `is_judgment` to false and `feedback` to "": it is handled as chat and the candidate keeps waiting. "I don't know" or "not sure" is a verdict: it fails.

Return ONLY a JSON object:
{"is_judgment": true, "pass": false, "feedback": "...", "misconception": null}"#;

const AGENT_SYSTEM: &str = r#"You do the work for one part of a task in the user's workspace, using the tools available.

Work in the open: before each step, say in one short sentence what you're about to do and why, then do it. Read and search before changing files; change them with `edit` (or `write` for new files); run builds, tests, installs and git with `bash`. The user approves each command and watches it run. If they decline one, find another way or explain what's blocked.

Stay within this part. End with a report of what you changed or found, concrete enough for the user to review."#;

/// The usage-event kind of a tool call.
fn tool_kind(kind: crate::stream::ToolKind) -> pathfinder_schema::Tool {
    use crate::stream::ToolKind as K;
    use pathfinder_schema::Tool as T;
    match kind {
        K::Read => T::Read,
        K::Edit => T::Edit,
        K::Search => T::Search,
        K::Execute => T::Execute,
        K::Fetch => T::Fetch,
        K::Other => T::Other,
    }
}

/// What kind of turn this was, for usage events.
fn turn_step(text: &str, session: &Session, had_candidate: bool) -> pathfinder_schema::Step {
    use pathfinder_schema::Step;
    if let Some(cmd) = text.strip_prefix('/') {
        return match cmd.split_whitespace().next().unwrap_or("") {
            "plan" | "replan" | "map" => Step::Map,
            "summary" => Step::Summary,
            "delegate" => Step::Delegate,
            "handoff" => Step::Handoff,
            _ => Step::Other,
        };
    }
    let has_candidate = session.pending_candidate.is_some();
    match (had_candidate, has_candidate) {
        (true, false) => Step::Verdict,
        (false, true) => Step::Candidate,
        _ => match session.mode {
            SessionMode::Onboarding => Step::Onboarding,
            SessionMode::Working => Step::Log,
            SessionMode::Done => Step::Chat,
        },
    }
}

#[cfg(test)]
mod tests;
