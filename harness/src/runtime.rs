//! The loop from `docs/theory.md`. Section numbers refer to that document.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::config::{Config, ModelEntry};
use crate::decide::{Decide, Decision, Question};
use crate::domain::{
    CandidateProposal, Intent, JudgmentVerdict, LogReply, Map, MapChange, MapProposal,
    OnboardingDecision, Part, PartStatus, PendingCandidate, Session, SessionMode,
};
use crate::engine::{
    Engine, EngineRequest, EngineTier, RunContext, StreamEvent, StreamSink, TierModels, ToolAccess,
    run_gated, run_gated_streamed,
};
use crate::store::Store;
use crate::validate::{
    check_candidate, check_change, check_log_reply, check_map, check_onboarding, check_verdict,
};
use crate::workspace::WorkspaceSnapshot;

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

pub const COMMANDS_HELP: &str = "Commands: `/plan`, `/summary <text>`, `/handoff [notes]`, `/approve`, `/object <reason>`, `/known`, `/delegate [notes]`, `/accept`, `/reject`, `/replan [what changed]`, `/next`, `/focus <id|number|prefix>`, `/map`.";

/// No decision model: the LLM decides each message's step itself.
struct NoDecider;

impl Decide for NoDecider {
    fn decide<'a>(
        &'a self,
        _input: &'a str,
        _questions: &'a [Question],
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
}

pub const NO_MODEL: &str =
    "No model is configured. Add models to `~/.config/harness/config.json` (see the README).";

impl Runtime {
    pub fn new(store: Arc<Store>, engine: Arc<dyn Engine>) -> Self {
        Self {
            store,
            engine,
            flaw_coin: random_flaw,
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
        Ok(TierModels {
            system1: pick(&session.system1, false)?,
            system2: pick(&session.system2, true)?,
        })
    }

    pub fn model_choices(&self, session: &Session) -> ModelChoices {
        let config = (self.config)();
        ModelChoices {
            catalog: config.catalog(),
            system1: config.pick(session.system1.as_deref(), false).map(|m| m.id),
            system2: config.pick(session.system2.as_deref(), true).map(|m| m.id),
        }
    }

    /// Sets a session's System 1 or System 2 model from the picker.
    pub fn set_model(&self, session_id: &str, system2: bool, entry_id: &str) -> Result<Session> {
        let mut session = self
            .store
            .load_session(session_id)?
            .ok_or_else(|| anyhow!("unknown session `{session_id}`"))?;
        if (self.config)().entry(entry_id).is_none() {
            return Err(anyhow!("unknown model `{entry_id}`"));
        }
        let slot = if system2 {
            &mut session.system2
        } else {
            &mut session.system1
        };
        *slot = Some(entry_id.to_string());
        self.store.save_session(&session)?;
        Ok(session)
    }

    pub fn create_session(&self, id: String, cwd: PathBuf) -> Result<Session> {
        let session = Session::new(id, cwd);
        self.store.save_session(&session)?;
        Ok(session)
    }

    pub fn load_session(&self, id: &str) -> Result<Option<Session>> {
        self.store.load_session(id)
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

        let text = input.text.trim();
        let files = &input.files;
        let output = if text.starts_with('/') {
            self.handle_command(&mut session, text, files, sink).await?
        } else {
            match session.mode {
                SessionMode::Onboarding => {
                    self.handle_onboarding(&mut session, text, files, sink)
                        .await?
                }
                SessionMode::Working => self.handle_chat(&mut session, text, files, sink).await?,
                SessionMode::Done => TurnOutput::text(
                    "Every part is resolved. `/replan <what changed>` to extend the map, or start a new session.",
                ),
            }
        };

        self.store.save_session(&session)?;
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
            "/approve" => self.handle_approve(session),
            "/known" => self.handle_known(session),
            "/delegate" => self.handle_delegate(session, arg, files, sink).await,
            "/accept" => self.handle_accept(session),
            "/reject" => Ok(match session.pending_change.take() {
                Some(_) => TurnOutput::text("Map change rejected."),
                None => TurnOutput::text("No map change is waiting."),
            }),
            "/replan" => self.handle_replan(session, arg, files, sink).await,
            "/next" => Ok(self.handle_next(session)),
            "/focus" => Ok(self.handle_focus(session, arg)),
            _ => Ok(TurnOutput::text(COMMANDS_HELP)),
        }
    }

    // --- §3 Onboarding and the map ---

    async fn handle_onboarding(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        session.push_turn("user", text);
        let snapshot = WorkspaceSnapshot::scan(&session.cwd).render();
        let req = EngineRequest {
            tier: EngineTier::System1,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: ONBOARDING_SYSTEM.to_string(),
            prompt: format!(
                "Workspace snapshot:\n{snapshot}\n\nConversation so far:\n{}\n\nRead any files you need first, then respond with JSON only.",
                format_history(session)
            ),
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: RunContext::default(),
        };
        let (decision, shown): (OnboardingDecision, _) = run_gated_streamed(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "onboarding",
            &req,
            sink.clone(),
            check_onboarding,
            Some("reply"),
        )
        .await?;
        flush(&sink, &decision.reply, &shown);

        if let Some(goal) = decision.goal.filter(|g| !g.trim().is_empty()) {
            session.goal = Some(goal);
        }
        session.push_turn("assistant", &decision.reply);
        if decision.ready_to_map {
            sink(StreamEvent::TextDelta("\n\n".to_string()));
            return self.build_map(session, files, sink).await;
        }
        Ok(TurnOutput::text(String::new()))
    }

    async fn build_map(
        &self,
        session: &mut Session,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let snapshot = WorkspaceSnapshot::scan(&session.cwd).render();
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: MAP_SYSTEM.to_string(),
            prompt: format!(
                "Goal: {}\n\nWorkspace snapshot:\n{snapshot}\n\nOnboarding conversation:\n{}\n\nRead any files you need first, then respond with JSON only.",
                session
                    .goal
                    .as_deref()
                    .unwrap_or("(infer from the conversation)"),
                format_history(session)
            ),
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: RunContext::default(),
        };
        let (proposal, shown): (MapProposal, _) = run_gated_streamed(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "map",
            &req,
            sink.clone(),
            check_map,
            Some("brief"),
        )
        .await?;
        flush(&sink, &proposal.brief, &shown);

        session.map = proposal.to_map();
        session.active = None;
        session.pending_change = None;
        session.pending_candidate = None;
        session.advance();

        // The map itself shows in Zed's plan panel (and on `/map`).
        let rest = session
            .active_part()
            .map(|part| render_part(part, true))
            .unwrap_or_default();
        session.push_turn("assistant", format!("{}\n\n{rest}", proposal.brief));
        Ok(TurnOutput::changed(format!("\n\n{rest}")))
    }

    /// §3: the model proposes map changes; you accept or reject them.
    async fn handle_replan(
        &self,
        session: &mut Session,
        reason: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        if session.map.parts.is_empty() {
            return Ok(TurnOutput::text("No map yet. `/plan` builds one."));
        }
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: CHANGE_SYSTEM.to_string(),
            prompt: format!(
                "Goal: {}\nWhat changed: {}\n\nCurrent map:\n{}\n\nRecent conversation:\n{}\n\nRead any files you need first, then respond with JSON only.",
                session.goal.as_deref().unwrap_or("(session goal)"),
                if reason.is_empty() {
                    "(infer from the conversation)"
                } else {
                    reason
                },
                session.map.render(session.active.as_deref()),
                format_history(session)
            ),
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: RunContext::default(),
        };
        let current = session.map.clone();
        let change: MapChange = run_gated(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "map_change",
            &req,
            sink,
            |c: &MapChange| check_change(c, &current),
        )
        .await?;
        let reply = render_change(&change, &session.map);
        session.pending_change = Some(change);
        Ok(TurnOutput::text(reply))
    }

    fn handle_accept(&self, session: &mut Session) -> Result<TurnOutput> {
        let Some(change) = session.pending_change.take() else {
            return Ok(TurnOutput::text("No map change is waiting."));
        };
        match session.map.with_change(&change) {
            Ok(map) => {
                session.map = map;
                if let Some(goal) = change.goal.filter(|g| !g.trim().is_empty()) {
                    session.goal = Some(goal);
                }
                if session
                    .active
                    .as_deref()
                    .is_none_or(|id| session.map.get(id).is_none())
                {
                    session.active = None;
                    session.advance();
                } else if session.mode == SessionMode::Done && !session.map.all_resolved() {
                    session.advance();
                }
                Ok(TurnOutput::changed("Map updated."))
            }
            Err(e) => Ok(TurnOutput::text(format!(
                "That change no longer fits the map ({e}). Dropped it."
            ))),
        }
    }

    fn handle_next(&self, session: &mut Session) -> TurnOutput {
        let current = session.active.clone();
        match session.map.next_open(current.as_deref()) {
            Some(id) => {
                session.active = Some(id);
                session.mode = SessionMode::Working;
                session.pending_candidate = None;
                TurnOutput::changed(render_part(session.active_part().unwrap(), false))
            }
            None if session.map.all_resolved() => {
                session.advance();
                TurnOutput::changed("Every part is resolved.")
            }
            None => TurnOutput::text("No other unresolved part."),
        }
    }

    fn handle_focus(&self, session: &mut Session, query: &str) -> TurnOutput {
        if query.is_empty() {
            return TurnOutput::text(
                "Usage: `/focus <id>`, its number in the plan, or the start of its id.",
            );
        }
        let id = match session.map.resolve(query) {
            Ok(id) => id,
            Err(message) => return TurnOutput::text(message),
        };
        session.active = Some(id);
        session.mode = SessionMode::Working;
        session.pending_candidate = None;
        TurnOutput::changed(render_part(session.active_part().unwrap(), false))
    }

    // --- §5 Evidence: log, summary, candidate ---

    /// `decided_by`: the decision model that already classified the message
    /// as a log entry; the LLM then only replies.
    async fn handle_log(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
        decided_by: Option<&str>,
    ) -> Result<TurnOutput> {
        let Some(part) = session.active_part_mut() else {
            return Ok(TurnOutput::text(
                "No part is active. `/focus <id>` picks one; `/map` shows them.",
            ));
        };
        let had_log = !part.log.is_empty();
        part.log.push(text.to_string());
        let part = part.clone();
        session.push_turn("user", text);

        let mut waiting = String::new();
        if let Some(by) = decided_by {
            waiting.push_str(&format!(
                "\n\nDecision: {by} classified this message as a log entry. Set `intent` to \"log\" and reply to it."
            ));
        }
        if let Some(change) = &session.pending_change {
            waiting.push_str(&format!(
                "\n\nMap change waiting for the user's OK:\n{}",
                render_change(change, &session.map)
            ));
        }
        // The waiting candidate is hidden from this call: it may be flawed on
        // purpose, and a model that can see it tends to vouch for it.
        let mut history = format_history(session);
        if let Some(pending) = &session.pending_candidate {
            history = history.replace(&pending.candidate, CANDIDATE_HIDDEN);
            waiting.push_str(
                "\n\nA candidate for this part is waiting for the user's verdict; it's hidden from you. Answer questions about the general concepts only. If they ask about the candidate itself (a claim, step or number in it, or whether it's right), say that's what they're judging. Don't mention that it's hidden from you.",
            );
        }

        let req = EngineRequest {
            tier: EngineTier::System1,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: LOG_SYSTEM.to_string(),
            prompt: format!(
                "Goal: {}\n\n{}\n\nMap:\n{}{waiting}\n\nRecent conversation:\n{}\n\nLatest message: {text}\n\nRead any files you need first, then respond with JSON only.",
                session.goal.as_deref().unwrap_or("(session goal)"),
                describe_part(&part),
                session.map.render(session.active.as_deref()),
                history,
            ),
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: RunContext::default(),
        };
        let current = session.map.clone();
        let change_waiting = session.pending_change.is_some();
        let (decision, shown): (LogReply, _) = run_gated_streamed(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "log",
            &req,
            sink.clone(),
            |r: &LogReply| check_log_reply(r, &current, change_waiting, decided_by.is_some()),
            Some("reply"),
        )
        .await?;

        if decision.intent != Intent::Log && decided_by.is_none() {
            // Not evidence for the log: the message is a step in the loop.
            if let Some(p) = session.map.get_mut(&part.id) {
                p.log.pop();
            }
            return self
                .route_intent(
                    session,
                    decision.intent,
                    decision.part.as_deref(),
                    text,
                    had_log,
                    files,
                    sink,
                )
                .await;
        }
        flush(&sink, &decision.reply, &shown);

        let mut visible = String::new();
        let mut full = decision.reply;
        if let Some(change) = decision.map_change {
            let rendered = format!("\n\n{}", render_change(&change, &session.map));
            visible.push_str(&rendered);
            full.push_str(&rendered);
            session.pending_change = Some(change);
        }
        session.push_turn("assistant", &full);
        Ok(TurnOutput::text(visible))
    }

    /// A chat message while working. The decision model, when configured,
    /// first says which step it is; the LLM then only does that step. With no
    /// decision model, or an unsure one, the LLM decides as part of its reply.
    async fn handle_chat(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        if session.active_part().is_none() {
            return self.handle_log(session, text, files, sink, None).await;
        }
        match self.decide_step(session, text, &sink).await {
            Some((step, _, _)) if step == VERDICT => {
                self.handle_judgment(session, text, files, sink).await
            }
            Some((step, _, by)) if step == "log" => {
                self.handle_log(session, text, files, sink, Some(&by)).await
            }
            Some((step, part, _)) => {
                let intent = match step.as_str() {
                    "summary" => Intent::Summary,
                    "known" => Intent::Known,
                    "move" => Intent::Move,
                    "accept" => Intent::Accept,
                    _ => Intent::Reject,
                };
                let had_log = session.active_part().is_some_and(|p| !p.log.is_empty());
                session.push_turn("user", text);
                self.route_intent(session, intent, part.as_deref(), text, had_log, files, sink)
                    .await
            }
            None if session.pending_candidate.is_some() => {
                self.handle_judgment(session, text, files, sink).await
            }
            None => self.handle_log(session, text, files, sink, None).await,
        }
    }

    /// Asks the decision model which step `text` is: (step, part for a move,
    /// who decided). `None` when there's no decision model, it fails, or it
    /// isn't sure enough.
    async fn decide_step(
        &self,
        session: &Session,
        text: &str,
        sink: &StreamSink,
    ) -> Option<(String, Option<String>, String)> {
        let part = session.active_part()?;
        let mut steps: Vec<(String, String)> = vec![
            ("log".into(), "Their thinking, reasoning, an attempt, or a question (including a question about a waiting candidate).".into()),
            ("summary".into(), "They offer their own explanation of the whole current part, ready to be checked.".into()),
            ("known".into(), "They clearly say they already know this whole part and want to skip it.".into()),
            ("move".into(), "They want to go on to the next part, or to another named part.".into()),
        ];
        if session.pending_change.is_some() {
            steps.push((
                "accept".into(),
                "They agree to the waiting map change.".into(),
            ));
            steps.push((
                "reject".into(),
                "They turn down the waiting map change.".into(),
            ));
        }
        if session.pending_candidate.is_some() {
            steps.push((VERDICT.into(), "They give their verdict on the waiting candidate: whether it's right, ideally why (\"I don't know\" counts).".into()));
        }
        let mut questions = vec![Question {
            name: "step".into(),
            instructions: "What is the user's latest message, given the session state?".into(),
            options: steps,
        }];
        if session.map.parts.len() > 1 {
            let mut options = vec![(
                "next".into(),
                "The next unresolved part, or no particular part.".into(),
            )];
            options.extend(
                session
                    .map
                    .parts
                    .iter()
                    .take(250)
                    .map(|p| (p.id.clone(), p.title.clone())),
            );
            questions.push(Question {
                name: "part".into(),
                instructions: "If they want to go to a part, which one do they name?".into(),
                options,
            });
        }

        let mut input = format!(
            "Goal: {}\nCurrent part: {} (`{}`): {}\nLog entries on this part so far: {}\n",
            session.goal.as_deref().unwrap_or("(none)"),
            part.title,
            part.id,
            part.note,
            part.log.len()
        );
        match &session.pending_change {
            Some(change) => input.push_str(&format!(
                "Map change waiting for the user's OK:\n{}\n",
                render_change(change, &session.map)
            )),
            None => input.push_str("No map change is waiting.\n"),
        }
        match &session.pending_candidate {
            Some(c) => input.push_str(&format!(
                "Candidate waiting for the user's verdict:\n{}\n",
                c.candidate
            )),
            None => input.push_str("No candidate is waiting.\n"),
        }
        let start = session.history.len().saturating_sub(6);
        input.push_str("\nRecent conversation:\n");
        for t in &session.history[start..] {
            input.push_str(&format!("{}: {}\n", t.role, t.content));
        }
        input.push_str(&format!("\nLatest message: {text}"));

        let decision = match self.decider.decide(&input, &questions).await {
            Ok(Some(d)) => d,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!("decision model failed: {e:#}");
                sink(StreamEvent::Thought(format!(
                    "Decision model unavailable ({e}); the LLM decides.\n"
                )));
                return None;
            }
        };
        let step = decision.answers.get("step")?;
        sink(StreamEvent::Thought(format!(
            "{}: {} ({:.0}%)\n",
            decision.by,
            step.choice,
            step.probability * 100.0
        )));
        if step.probability < DECISION_THRESHOLD {
            return None;
        }
        let part = decision.answers.get("part").map(|a| a.choice.clone());
        Some((step.choice.clone(), part, decision.by))
    }

    /// Carries out what a chat message meant, the way its command would.
    async fn route_intent(
        &self,
        session: &mut Session,
        intent: Intent,
        part: Option<&str>,
        text: &str,
        had_log: bool,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        match intent {
            Intent::Summary if had_log => {
                // `/summary` records the message itself.
                session.history.pop();
                self.handle_summary(session, text, files, sink).await
            }
            Intent::Summary => {
                if let Some(p) = session.active_part_mut() {
                    p.log.push(text.to_string());
                }
                let reply = "Noted in your log. The evidence starts there: keep writing your thinking and reasons as you work on this part, then explain it as a whole again for a candidate to judge.";
                session.push_turn("assistant", reply);
                Ok(TurnOutput::text(reply))
            }
            Intent::Known => self.handle_known(session),
            Intent::Move => Ok(match part.filter(|p| !p.is_empty() && *p != "next") {
                Some(id) => self.handle_focus(session, id),
                None => self.handle_next(session),
            }),
            Intent::Accept => self.handle_accept(session),
            Intent::Reject => Ok(match session.pending_change.take() {
                Some(_) => TurnOutput::text("Map change rejected."),
                None => TurnOutput::text("No map change is waiting."),
            }),
            Intent::Log => unreachable!("log entries are answered in handle_log"),
        }
    }

    async fn handle_summary(
        &self,
        session: &mut Session,
        summary: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let Some(part) = session.active_part_mut() else {
            return Ok(TurnOutput::text(
                "No part is active. `/focus <id>` picks one.",
            ));
        };
        if part.status != PartStatus::Open {
            return Ok(TurnOutput::text(format!(
                "**{}** is already {}.",
                part.title,
                part.status.label()
            )));
        }
        if summary.is_empty() {
            return Ok(TurnOutput::text(
                "Usage: `/summary <your explanation of the part>`.",
            ));
        }
        if part.log.is_empty() {
            return Ok(TurnOutput::text(
                "The evidence starts with your log: write your thinking and reasons as you work on this part first, then `/summary`.",
            ));
        }
        part.summary = Some(summary.to_string());
        let part = part.clone();
        session.push_turn("user", format!("/summary {summary}"));

        let flawed = (self.flaw_coin)();
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: CANDIDATE_SYSTEM.to_string(),
            prompt: format!(
                "Goal: {}\n\n{}\n\nMake this candidate {}.\n\nRead any files you need first, then respond with JSON only.",
                session.goal.as_deref().unwrap_or("(session goal)"),
                describe_part(&part),
                if flawed {
                    "FLAWED: exactly one real flaw"
                } else {
                    "CORRECT"
                }
            ),
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: RunContext::default(),
        };
        let header = format!("Candidate for **{}**:\n\n", part.title);
        sink(StreamEvent::TextDelta(header.clone()));
        let (proposal, shown): (CandidateProposal, _) = run_gated_streamed(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "candidate",
            &req,
            sink.clone(),
            check_candidate,
            Some("candidate"),
        )
        .await?;
        flush(&sink, &proposal.candidate, &shown);

        const ASK: &str = "\n\nIs it right? Say why.";
        let visible = ASK.to_string();
        let reply = format!("{header}{}{ASK}", proposal.candidate);
        session.pending_candidate = Some(PendingCandidate {
            part_id: part.id,
            candidate: proposal.candidate,
            key_correct: !flawed,
            key_reason: proposal.key_reason,
        });
        session.push_turn("assistant", &reply);
        Ok(TurnOutput::text(visible))
    }

    async fn handle_judgment(
        &self,
        session: &mut Session,
        judgment: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let pending = session.pending_candidate.take().unwrap();
        let Some(part) = session.map.get(&pending.part_id).cloned() else {
            return Ok(TurnOutput::text(
                "That candidate's part is no longer on the map.",
            ));
        };
        session.push_turn("user", judgment);

        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: JUDGE_SYSTEM.to_string(),
            prompt: format!(
                "Part: {}\n\nCandidate:\n{}\n\nHidden key: the candidate is {}. {}\n\nUser's judgment: {judgment}\n\nRespond with JSON only.",
                part.title,
                pending.candidate,
                if pending.key_correct {
                    "CORRECT"
                } else {
                    "FLAWED"
                },
                pending.key_reason,
            ),
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: RunContext::default(),
        };
        let (verdict, shown): (JudgmentVerdict, _) = run_gated_streamed(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "judgment",
            &req,
            sink.clone(),
            check_verdict,
            Some("feedback"),
        )
        .await?;
        if !verdict.is_judgment {
            // A question or another step: the candidate keeps waiting.
            session.pending_candidate = Some(pending);
            session.history.pop();
            return self.handle_log(session, judgment, files, sink, None).await;
        }
        flush(&sink, &verdict.feedback, &shown);

        let next = if verdict.pass {
            self.set_status(session, &part.id, PartStatus::Understood, &verdict.feedback)?;
            format!(
                "\n\n**{}** understood. Say when to move on (`/next`), or `/handoff` if this part has work for the agent.",
                part.title
            )
        } else {
            "\n\nKeep working through it here, then explain the part again for a new candidate."
                .to_string()
        };
        session.push_turn("assistant", format!("{}{next}", verdict.feedback));
        Ok(TurnOutput::changed(next))
    }

    // --- §6 Skips ---

    fn handle_known(&self, session: &mut Session) -> Result<TurnOutput> {
        let Some(part) = session.active_part().cloned() else {
            return Ok(TurnOutput::text("No part is active."));
        };
        if part.status != PartStatus::Open {
            return Ok(TurnOutput::text(format!(
                "**{}** is already {}.",
                part.title,
                part.status.label()
            )));
        }
        session.pending_candidate = None;
        self.set_status(session, &part.id, PartStatus::Known, "claimed by the user")?;
        Ok(self.after_resolution(
            session,
            format!("**{}** recorded as already known.", part.title),
        ))
    }

    async fn handle_delegate(
        &self,
        session: &mut Session,
        instructions: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let Some(part) = session.active_part().cloned() else {
            return Ok(TurnOutput::text("No part is active."));
        };
        match part.status {
            PartStatus::Open => {}
            PartStatus::Understood => {
                return Ok(TurnOutput::text(
                    "You understood this part, so `/handoff` it: you'll review the result.",
                ));
            }
            other => {
                return Ok(TurnOutput::text(format!(
                    "**{}** is already {}.",
                    part.title,
                    other.label()
                )));
            }
        }
        session.pending_candidate = None;
        let result = self
            .run_agent(session, &part, false, instructions, files, sink)
            .await?;
        if let Some(p) = session.map.get_mut(&part.id) {
            p.result = Some(result.clone());
        }
        self.set_status(session, &part.id, PartStatus::Delegated, &result)?;
        Ok(self.after_resolution(
            session,
            format!(
                "\n\n**{}** handed off without understanding; not reviewed.",
                part.title
            ),
        ))
    }

    // --- §7 Handoff and review ---

    async fn handle_handoff(
        &self,
        session: &mut Session,
        instructions: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let Some(part) = session.active_part().cloned() else {
            return Ok(TurnOutput::text("No part is active."));
        };
        match part.status {
            PartStatus::Understood => {}
            PartStatus::Open => {
                return Ok(TurnOutput::text(
                    "Handoff comes after understanding: log, `/summary`, then judge the candidate. To hand it off without understanding, `/delegate`.",
                ));
            }
            other => {
                return Ok(TurnOutput::text(format!(
                    "**{}** is already {}.",
                    part.title,
                    other.label()
                )));
            }
        }
        let result = self
            .run_agent(session, &part, true, instructions, files, sink)
            .await?;
        if let Some(p) = session.map.get_mut(&part.id) {
            p.result = Some(result);
            p.status = PartStatus::InReview;
        }
        Ok(TurnOutput::changed(REVIEW_PROMPT))
    }

    async fn handle_object(
        &self,
        session: &mut Session,
        reason: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let Some(part) = session.active_part().cloned() else {
            return Ok(TurnOutput::text("No part is active."));
        };
        if part.status != PartStatus::InReview {
            return Ok(TurnOutput::text(
                "Nothing is waiting for your review on this part.",
            ));
        }
        if reason.is_empty() {
            return Ok(TurnOutput::text("Usage: `/object <what's wrong and why>`."));
        }
        if let Some(p) = session.map.get_mut(&part.id) {
            p.objections.push(reason.to_string());
        }
        let part = session.map.get(&part.id).cloned().unwrap();
        let result = self
            .run_agent(session, &part, true, "", files, sink)
            .await?;
        if let Some(p) = session.map.get_mut(&part.id) {
            p.result = Some(result);
        }
        Ok(TurnOutput::text(REVIEW_PROMPT))
    }

    fn handle_approve(&self, session: &mut Session) -> Result<TurnOutput> {
        let Some(part) = session.active_part().cloned() else {
            return Ok(TurnOutput::text("No part is active."));
        };
        if part.status != PartStatus::InReview {
            return Ok(TurnOutput::text(
                "Nothing is waiting for your review on this part.",
            ));
        }
        self.set_status(session, &part.id, PartStatus::Done, "result approved")?;
        Ok(self.after_resolution(session, format!("**{}** done.", part.title)))
    }

    async fn run_agent(
        &self,
        session: &Session,
        part: &Part,
        understood: bool,
        instructions: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<String> {
        let mut prompt = format!(
            "Goal: {}\n\n{}\n\n{}",
            session.goal.as_deref().unwrap_or("(session goal)"),
            describe_part(part),
            if understood {
                "The user understands this part and will review your result."
            } else {
                "The user handed this part off without understanding it."
            }
        );
        if !part.objections.is_empty() {
            prompt.push_str(&format!(
                "\n\nYour previous result:\n{}\n\nThe user objected:\n- {}\n\nAddress every objection.",
                part.result.as_deref().unwrap_or("(none)"),
                part.objections.join("\n- ")
            ));
        }
        if !instructions.is_empty() {
            prompt.push_str(&format!("\n\nUser instructions: {instructions}"));
        }
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::Full,
            cwd: session.cwd.clone(),
            system_prompt: AGENT_SYSTEM.to_string(),
            prompt,
            files: files.to_vec(),
            stream_text: true,
            models: self.models(session)?,
            ctx: RunContext::default(),
        };
        self.engine.run_text(&req, sink).await
    }

    // --- §8 Record ---

    fn set_status(
        &self,
        session: &mut Session,
        part_id: &str,
        status: PartStatus,
        notes: &str,
    ) -> Result<()> {
        let Some(part) = session.map.get_mut(part_id) else {
            return Ok(());
        };
        part.status = status;
        let title = part.title.clone();
        if let Some(record) = status.record() {
            self.store.record_outcome(
                &session.id,
                part_id,
                &title,
                record,
                status.label(),
                Some(notes),
            )?;
        }
        Ok(())
    }

    fn after_resolution(&self, session: &mut Session, lead: String) -> TurnOutput {
        let reply = match session.advance() {
            Some(_) => format!(
                "{lead}\n\n{}",
                render_part(session.active_part().unwrap(), false)
            ),
            None if session.mode == SessionMode::Done => {
                format!("{lead}\n\nEvery part is resolved.")
            }
            None => lead,
        };
        session.push_turn("assistant", &reply);
        TurnOutput::changed(reply)
    }
}

/// The decision model's step for a verdict on a waiting candidate.
const VERDICT: &str = "verdict";

/// Below this, the decision model's step is ignored and the LLM decides.
const DECISION_THRESHOLD: f64 = 0.6;

const CANDIDATE_HIDDEN: &str = "(candidate hidden while the user judges it)";

const REVIEW_PROMPT: &str =
    "\n\nReview the result: `/approve`, or `/object <what's wrong and why>` to send it back.";

/// Makes sure the user has seen `full`: if streaming didn't show it, send it
/// now, so it stays ahead of anything that follows.
fn flush(sink: &StreamSink, full: &str, shown: &str) {
    if shown.is_empty() || shown.trim() != full.trim() {
        sink(StreamEvent::TextDelta(full.to_string()));
    }
}

/// A part as you start on it. The command hint shows only with the first part.
fn render_part(part: &Part, hint: bool) -> String {
    let mut out = format!("**{}** (`{}`)", part.title, part.id);
    if !part.note.trim().is_empty() {
        out.push_str(&format!("\n{}", part.note.trim()));
    }
    if part.status != PartStatus::Open {
        out.push_str(&format!("\nStatus: {}.", part.status.label()));
    }
    if hint && part.status == PartStatus::Open {
        out.push_str(
            "\n\nTalk it through here as you work: your thinking and reasons are the log. When you can explain the whole part, say so and explain it (`/summary`); say if you already know it (`/known`); `/delegate` hands it off without understanding it.",
        );
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
fn render_change(change: &MapChange, map: &Map) -> String {
    let mut lines = vec![format!("Proposed map change: {}", change.brief)];
    if let Some(goal) = change.goal.as_deref().filter(|g| !g.trim().is_empty()) {
        lines.push(format!("Goal → {goal}"));
    }
    for p in &change.parts {
        let deps = if p.depends_on.is_empty() {
            String::new()
        } else {
            format!(" (needs {})", p.depends_on.join(", "))
        };
        let mark = if map.get(&p.id).is_some() { "~" } else { "+" };
        lines.push(format!("{mark} `{}` {}{deps}", p.id, p.title));
    }
    for id in &change.remove {
        lines.push(format!("- remove `{id}`"));
    }
    lines.push("`/accept` or `/reject`.".to_string());
    lines.join("\n")
}

fn render_map_and_pending(session: &Session) -> String {
    let mut out = session.map.render(session.active.as_deref());
    if let Some(change) = &session.pending_change {
        out.push_str("\n\n");
        out.push_str(&render_change(change, &session.map));
    }
    out
}

fn format_history(session: &Session) -> String {
    if session.history.is_empty() {
        return "(none)".to_string();
    }
    let start = session.history.len().saturating_sub(12);
    session.history[start..]
        .iter()
        .map(|t| format!("{}: {}", t.role, t.content))
        .collect::<Vec<_>>()
        .join("\n")
}

const ONBOARDING_SYSTEM: &str = r#"You run onboarding for Pathfinder, a harness that makes sure the user understands a task before the work is handed to an agent.

Onboarding only sets the goal. Right after it, Pathfinder sketches a rough map of what the task depends on. The map has no size limit and grows and changes during the work, so scope, depth and details are settled there, not here. Do not plan, teach or do the task.

Aim to finish in this turn:
- If the user has named something to do, build, fix, understand or learn, the goal is clear: set `ready_to_map` to true and write `goal` as one sentence starting with a verb, in their words but not in first person ("Learn how neurons in neural networks work"). Breadth or vagueness ("learn how X works") is fine; the map handles it.
- Ask one short question only when you can't tell what the task is at all (e.g. "orient yourself", "help me with my thesis"). If the workspace has content, ground it: say in one sentence what the workspace is (from the snapshot and any files worth reading, such as a README), then ask what they want to do there. If it's empty, just ask. Never ask the user to confirm, choose a scope, or say what they want to be able to explain.
- If the user says what they already know or don't ("I'm new to this", "I know the basics"), keep it in `goal` (e.g. "... starting from the basics").

`reply`: when ready, just the goal in one sentence (the map is shown right after it; don't announce it). When asking, at most two sentences.

You get a workspace snapshot. If a file could answer something, read it before writing your JSON, since `reply` ends your turn. Don't read files when the snapshot and the user's words already settle it.

Return ONLY a JSON object:
{"reply": "...", "ready_to_map": true, "goal": "..."}"#;

const MAP_SYSTEM: &str = r#"You sketch the map for Pathfinder: what the task depends on.

- A part is something the task depends on: a piece of the work, or of the understanding needed to judge the work. It is not a lesson and not an assignment: the user decides during the work which parts they do themselves and which go to an agent.
- No size limit. Use as many parts as the task needs and no more; a small task can be a single part.
- `depends_on` lists parts that must be understood or done before this one makes sense. No cycles.
- `id`: a short descriptive slug the user types in commands: lowercase words joined by hyphens, at most 12 characters (e.g. `weights`, `fwd-pass`, `loss`). Unique.
- Start where the user is: if the goal or conversation says what they already know or don't, shape the parts for that (e.g. include the basics for a newcomer). Keep parts they say they know on the map; they can mark them known themselves.
- `note`: one line on what the part covers. `brief`: what the map covers, in one or two sentences.

Return ONLY a JSON object:
{"brief": "...", "parts": [{"id": "weights", "title": "...", "note": "...", "depends_on": []}]}"#;

const CHANGE_SYSTEM: &str = r#"You propose a change to Pathfinder's map of what the task depends on, because something changed. The user accepts or rejects it.

- `parts`: parts to add, or existing parts to update (same id), each with id, title, note, depends_on. New ids: `id`: a short descriptive slug the user types in commands: lowercase words joined by hyphens, at most 12 characters (e.g. `weights`, `fwd-pass`, `loss`). Unique.
- `remove`: ids of parts to drop. Only parts with no work or outcome can be removed.
- No size limit. No cycles.

- `goal`: only when what changed alters what the task is (a new purpose or scope): the new goal as one sentence starting with a verb, not in first person. Otherwise null.
`brief`: one sentence to the user, saying "you", on what changes and why.

Return ONLY a JSON object:
{"brief": "...", "goal": null, "parts": [], "remove": []}"#;

const LOG_SYSTEM: &str = r#"The user is working on one part of a task and chatting with Pathfinder as they go. Most messages are their log: their thinking and the reasons behind their steps and choices, or a question. The log is evidence of their understanding.

First decide what the latest message is (`intent`):
- `log`: thinking, reasoning, an attempt or a question. The default; when unsure, it's `log`.
- `summary`: they offer their explanation of the part as a whole, ready to be checked ("I think I get it: ...", "so to sum up, ..."). Not a single point or a question.
- `known`: they clearly say they already know this whole part and want to skip it. Knowing some of it isn't enough.
- `move`: they want to go on to the next part, or to a named one (put its id in `part`).
- `accept` / `reject`: they answer a waiting map change ("yes, add it", "no, keep the map").
For anything but `log`, set `reply` to "" and `map_change` to null: Pathfinder takes the step.

For `log`, reply briefly (math as plain text or Unicode, never LaTeX): answer what they ask, or point out what the entry reveals. If they ask whether something is right, say whether it is, even when it belongs to another part. Don't repeat what you already said in the conversation. Do not do the work for them and do not write their explanation of the part for them.

The user decides which parts they do themselves and which go to an agent. Never raise that.

If a `log` message reveals the map is wrong (a missing dependency, a part that doesn't belong, a wrong edge), propose a change in `map_change`; the user accepts or rejects it. Otherwise set it to null. A change has `brief` (one sentence to the user, saying "you"), `goal` (only when the message changes what the task is, e.g. a new purpose or scope: the new goal as one sentence starting with a verb, not in first person; otherwise null), `parts` (to add, or update by id: id, title, note, depends_on; new ids are short hyphenated slugs of at most 12 characters) and `remove` (ids of parts with no work yet).

Call `read` before writing your JSON if a file matters: `reply` ends your turn.

Return ONLY a JSON object:
{"intent": "log", "part": null, "reply": "...", "map_change": null}
or, with a change:
{"intent": "log", "part": null, "reply": "...", "map_change": {"brief": "...", "goal": null, "parts": [], "remove": []}}"#;

const CANDIDATE_SYSTEM: &str = r#"You make a candidate for the user to judge. Their judgment decides whether they understand the part well enough to direct an agent and tell whether its result is right.

A candidate is a concrete approach or result for this part, as an agent might produce it. Pick whichever fits the part. Keep it short enough to judge in a minute or two: a short paragraph, a few steps or a small snippet, under 150 words. Don't restate the user's summary back to them. Write math as plain text or Unicode (z = 2x₁ − x₂), never LaTeX.

You are told whether to make it correct or flawed. A flawed candidate contains exactly one real flaw that matters for the goal and that someone who understands the part would catch. It must look plausible: never hint at the flaw.

`key_reason` is hidden from the user: for a correct candidate, why it is right; for a flawed one, what the flaw is and why it matters.

Return ONLY a JSON object:
{"candidate": "...", "key_reason": "..."}"#;

const JUDGE_SYSTEM: &str = r#"Decide whether the user's judgment of a candidate shows they could judge an agent's work on this part.

You have the candidate, the hidden key (correct or flawed, and why) and the user's judgment.

`pass` is true only if their verdict matches the key and their reason does too: for a flawed candidate they name the actual flaw, not a different one; for a correct one they say why it holds, not just that it looks fine.

`feedback`: reveal the key and say what their judgment got right or missed.

If the message isn't a verdict on the candidate at all (a question about it or the part, a thought for their log, a request to skip or move on), set `is_judgment` to false and `feedback` to "": it is handled as chat and the candidate keeps waiting. "I don't know" or "not sure" is a verdict: it fails.

Return ONLY a JSON object:
{"is_judgment": true, "pass": false, "feedback": "..."}"#;

const AGENT_SYSTEM: &str = r#"You do the work for one part of a task in the user's workspace, using the tools available. End with a report of what you changed or found, concrete enough for the user to review."#;

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::engine::{BoxFuture, noop_sink};

    struct FakeEngine {
        replies: Mutex<VecDeque<(ToolAccess, EngineTier, String)>>,
        prompts: Mutex<Vec<String>>,
    }

    impl FakeEngine {
        fn new(items: Vec<(ToolAccess, EngineTier, &str)>) -> Self {
            Self {
                replies: Mutex::new(
                    items
                        .into_iter()
                        .map(|(t, tier, s)| (t, tier, s.to_string()))
                        .collect(),
                ),
                prompts: Mutex::new(Vec::new()),
            }
        }
    }

    impl Engine for FakeEngine {
        fn run_text<'a>(
            &'a self,
            req: &'a EngineRequest,
            _sink: StreamSink,
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move {
                let (tools, tier, reply) = self
                    .replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected engine call");
                assert_eq!(req.tools, tools, "tool access mismatch");
                assert_eq!(req.tier, tier, "engine tier mismatch");
                self.prompts.lock().unwrap().push(req.prompt.clone());
                // Stream raw text in small chunks, as the CLIs do.
                if req.stream_text {
                    let chars: Vec<char> = reply.chars().collect();
                    for chunk in chars.chunks(3) {
                        _sink(crate::engine::StreamEvent::TextDelta(
                            chunk.iter().collect(),
                        ));
                    }
                }
                Ok(reply)
            })
        }
    }

    use EngineTier::{System1 as S1, System2 as S2};

    /// Two models; the fake engine ignores their routes.
    fn test_config() -> Config {
        let entry = |id: &str| crate::config::ModelEntry {
            id: id.into(),
            label: id.into(),
            route: crate::config::Route::Anthropic,
            model: id.into(),
            effort: None,
        };
        Config {
            models: vec![entry("fast"), entry("main")],
            discover: false,
            ..Config::default()
        }
    }

    fn test_models() -> TierModels {
        let config = test_config();
        TierModels {
            system1: config.pick(None, false).unwrap(),
            system2: config.pick(None, true).unwrap(),
        }
    }

    #[test]
    fn set_model_stores_pick_and_rejects_unknown_ids() {
        let mut rt = Runtime::new(
            Arc::new(Store::open_in_memory().unwrap()),
            Arc::new(FakeEngine::new(vec![])),
        );
        rt.config = test_config;
        rt.create_session("s".into(), PathBuf::from(".")).unwrap();
        assert!(rt.set_model("s", true, "nope").is_err());
        let s = rt.set_model("s", false, "main").unwrap();
        assert_eq!(s.system1.as_deref(), Some("main"));
        let choices = rt.model_choices(&s);
        assert_eq!(choices.system1.as_deref(), Some("main"));
        assert_eq!(choices.system2.as_deref(), Some("main"));
        assert_eq!(choices.catalog.len(), 2);
    }
    use ToolAccess::{Full, ReadOnly};

    const ONBOARD_READY: &str =
        r#"{"reply":"Got it.","ready_to_map":true,"goal":"Add a /status command"}"#;
    const MAP: &str = r#"{"brief":"Two parts.","parts":[
        {"id":"route","title":"Command routing","note":"Where commands dispatch","depends_on":[]},
        {"id":"report","title":"What status reports","depends_on":["route"]}]}"#;

    fn runtime(engine: FakeEngine, flawed: bool) -> (Runtime, Arc<FakeEngine>) {
        let engine = Arc::new(engine);
        let mut rt = Runtime::new(Arc::new(Store::open_in_memory().unwrap()), engine.clone());
        rt.flaw_coin = if flawed { || true } else { || false };
        rt.config = test_config;
        rt.create_session("s".into(), PathBuf::from(".")).unwrap();
        (rt, engine)
    }

    async fn turn(rt: &Runtime, text: &str) -> TurnOutput {
        rt.handle_turn(
            "s",
            TurnInput {
                text: text.into(),
                files: vec![],
            },
            noop_sink(),
        )
        .await
        .unwrap()
    }

    fn status(rt: &Runtime, id: &str) -> PartStatus {
        rt.load_session("s")
            .unwrap()
            .unwrap()
            .map
            .get(id)
            .unwrap()
            .status
    }

    #[tokio::test]
    async fn understood_part_is_handed_off_reviewed_objected_and_approved() {
        let (rt, engine) = runtime(
            FakeEngine::new(vec![
                (ReadOnly, S1, ONBOARD_READY),
                (ReadOnly, S2, MAP),
                (
                    ReadOnly,
                    S1,
                    r#"{"reply":"Right, acp.rs lists them.","map_change":null}"#,
                ),
                (
                    ReadOnly,
                    S2,
                    r#"{"candidate":"Add it only in runtime.rs.","key_reason":"Zed never sees it unless acp.rs advertises it."}"#,
                ),
                (
                    ReadOnly,
                    S2,
                    r#"{"pass":true,"feedback":"Yes: it must be advertised in acp.rs."}"#,
                ),
                (Full, S2, "Added /status."),
                (Full, S2, "Advertised it too."),
            ]),
            true,
        );

        let out = turn(&rt, "add a /status command").await;
        assert!(out.reply.contains("Command routing"));
        // The map lives in Zed's plan panel; replies only introduce the part.
        assert!(!out.reply.contains("Map:"));
        assert!(!out.reply.contains("What status reports"));
        assert_eq!(
            rt.load_session("s").unwrap().unwrap().active.as_deref(),
            Some("route")
        );

        // §5: summary before any log is refused.
        assert!(turn(&rt, "/summary it routes").await.reply.contains("log"));
        // §7: handoff before understanding is refused.
        assert!(turn(&rt, "/handoff").await.reply.contains("/delegate"));

        turn(
            &rt,
            "commands are advertised in acp.rs and dispatched in runtime.rs",
        )
        .await;
        let cand = turn(&rt, "/summary two places: advertise and dispatch").await;
        assert!(cand.reply.contains("Is it right?"));
        let s = rt.load_session("s").unwrap().unwrap();
        assert!(!s.pending_candidate.as_ref().unwrap().key_correct);

        turn(&rt, "Wrong: acp.rs must advertise it or Zed won't show it").await;
        assert_eq!(status(&rt, "route"), PartStatus::Understood);

        turn(&rt, "/handoff").await;
        assert_eq!(status(&rt, "route"), PartStatus::InReview);
        turn(&rt, "/object it isn't advertised").await;
        let s = rt.load_session("s").unwrap().unwrap();
        assert_eq!(s.map.get("route").unwrap().objections.len(), 1);
        assert_eq!(
            s.map.get("route").unwrap().result.as_deref(),
            Some("Advertised it too.")
        );

        let out = turn(&rt, "/approve").await;
        assert_eq!(status(&rt, "route"), PartStatus::Done);
        assert!(out.reply.contains("What status reports"));
        assert!(engine.replies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn non_json_answer_is_reformatted_on_same_tier_before_escalating() {
        let store = Store::open_in_memory().unwrap();
        let engine = FakeEngine::new(vec![
            // S1 reads files, then answers in prose instead of JSON.
            (
                ReadOnly,
                S1,
                "Commands live in `acp.rs`. What should `/status` show?",
            ),
            // Same tier, no tools: re-wraps the same content as JSON.
            (
                ToolAccess::None,
                S1,
                r#"{"reply":"Commands live in `acp.rs`. What should `/status` show?","ready_to_map":false}"#,
            ),
        ]);
        let req = EngineRequest {
            tier: S1,
            tools: ReadOnly,
            cwd: PathBuf::from("."),
            system_prompt: String::new(),
            prompt: "onboard".into(),
            files: vec![],
            stream_text: false,
            models: test_models(),
            ctx: RunContext::default(),
        };
        let d: OnboardingDecision = run_gated(
            &engine,
            &store,
            "s",
            "onboarding",
            &req,
            noop_sink(),
            check_onboarding,
        )
        .await
        .unwrap();
        assert!(d.reply.starts_with("Commands live in"));
        assert!(engine.replies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn streamed_reply_is_shown_once_and_decoded() {
        let (rt, _) = runtime(
            FakeEngine::new(vec![(
                ReadOnly,
                S1,
                r#"{"reply":"What do you want \"status\" to show?","ready_to_map":false}"#,
            )]),
            false,
        );
        let seen = Arc::new(Mutex::new(String::new()));
        let record = seen.clone();
        let sink: StreamSink = Arc::new(move |ev| {
            if let crate::engine::StreamEvent::TextDelta(t) = ev {
                record.lock().unwrap().push_str(&t);
            }
        });
        let out = rt
            .handle_turn(
                "s",
                TurnInput {
                    text: "add /status".into(),
                    files: vec![],
                },
                sink,
            )
            .await
            .unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            r#"What do you want "status" to show?"#
        );
        assert_eq!(out.reply, "");
    }

    /// Runs one onboarding turn whose first answer fails the JSON check and is
    /// reformatted; returns everything the user saw.
    async fn shown_after_repair(repaired_reply: &str) -> String {
        let repaired = format!(r#"{{"reply":"{repaired_reply}","ready_to_map":false}}"#);
        let (rt, engine) = runtime(
            FakeEngine::new(vec![
                (
                    ReadOnly,
                    S1,
                    r#"{"reply":"What should it show?"} {"ready_to_map":false}"#,
                ),
                (ToolAccess::None, S1, &repaired),
            ]),
            false,
        );
        let seen = Arc::new(Mutex::new(String::new()));
        let record = seen.clone();
        let sink: StreamSink = Arc::new(move |ev| {
            if let crate::engine::StreamEvent::TextDelta(t) = ev {
                record.lock().unwrap().push_str(&t);
            }
        });
        let input = TurnInput {
            text: "add /status".into(),
            files: vec![],
        };
        let out = rt.handle_turn("s", input, sink).await.unwrap();
        assert!(engine.replies.lock().unwrap().is_empty());
        let shown = seen.lock().unwrap().clone();
        shown + &out.reply
    }

    #[tokio::test]
    async fn repeated_reply_after_retry_is_shown_once() {
        assert_eq!(
            shown_after_repair("What should it show?").await,
            "What should it show?"
        );
        assert_eq!(
            shown_after_repair("Which status fields?").await,
            "What should it show?\n\nWhich status fields?"
        );
    }

    #[tokio::test]
    async fn failed_judgment_keeps_part_open() {
        let (rt, _) = runtime(
            FakeEngine::new(vec![
                (ReadOnly, S1, ONBOARD_READY),
                (ReadOnly, S2, MAP),
                (ReadOnly, S1, r#"{"reply":"Ok.","map_change":null}"#),
                (ReadOnly, S2, r#"{"candidate":"c","key_reason":"r"}"#),
                (
                    ReadOnly,
                    S2,
                    r#"{"pass":false,"feedback":"It was correct."}"#,
                ),
            ]),
            false,
        );
        turn(&rt, "goal").await;
        turn(&rt, "thinking").await;
        turn(&rt, "/summary s").await;
        let out = turn(&rt, "looks wrong").await;
        assert!(out.reply.contains("explain the part again"));
        let s = rt.load_session("s").unwrap().unwrap();
        assert_eq!(s.map.get("route").unwrap().status, PartStatus::Open);
        assert!(s.pending_candidate.is_none());
    }

    #[tokio::test]
    async fn chat_drives_the_loop() {
        let (rt, engine) = runtime(
            FakeEngine::new(vec![
                (ReadOnly, S1, ONBOARD_READY),
                (ReadOnly, S2, MAP),
                (
                    ReadOnly,
                    S1,
                    r#"{"intent":"log","reply":"Ok.","map_change":null}"#,
                ),
                (
                    ReadOnly,
                    S1,
                    r#"{"intent":"summary","reply":"","map_change":null}"#,
                ),
                (
                    ReadOnly,
                    S2,
                    r#"{"candidate":"w = 0.5 so z = 99","key_reason":"r"}"#,
                ),
                // A question while the candidate waits: not a verdict.
                (
                    ReadOnly,
                    S2,
                    r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
                ),
                (
                    ReadOnly,
                    S1,
                    r#"{"intent":"log","reply":"It means x.","map_change":null}"#,
                ),
                (
                    ReadOnly,
                    S2,
                    r#"{"is_judgment":true,"pass":true,"feedback":"Right."}"#,
                ),
                (
                    ReadOnly,
                    S1,
                    r#"{"intent":"move","part":null,"reply":"","map_change":null}"#,
                ),
                (
                    ReadOnly,
                    S1,
                    r#"{"intent":"known","reply":"","map_change":null}"#,
                ),
            ]),
            false,
        );
        turn(&rt, "goal").await;
        turn(&rt, "thinking").await;
        turn(&rt, "I get it: it routes by name").await;
        let s = rt.load_session("s").unwrap().unwrap();
        assert!(s.pending_candidate.is_some());
        let part = s.map.get("route").unwrap();
        assert_eq!(part.log, vec!["thinking".to_string()]);
        assert_eq!(part.summary.as_deref(), Some("I get it: it routes by name"));

        let out = turn(&rt, "what does dispatch mean here?").await;
        assert!(!out.reply.contains("understood"));
        // The chat reply never sees the waiting candidate.
        let log_prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
        assert!(log_prompt.contains("what does dispatch mean here?"));
        assert!(!log_prompt.contains("z = 99"));
        let s = rt.load_session("s").unwrap().unwrap();
        assert!(s.pending_candidate.is_some());

        turn(&rt, "it's correct because names map to handlers").await;
        let s = rt.load_session("s").unwrap().unwrap();
        assert_eq!(s.map.get("route").unwrap().status, PartStatus::Understood);

        turn(&rt, "ok, next part").await;
        let s = rt.load_session("s").unwrap().unwrap();
        let next = s.active.clone().unwrap();
        assert_ne!(next, "route");

        turn(&rt, "I already know this one").await;
        let s = rt.load_session("s").unwrap().unwrap();
        assert_eq!(s.map.get(&next).unwrap().status, PartStatus::Known);
    }

    /// Scripted decision answers: (step, probability, part).
    struct FakeDecider(Mutex<VecDeque<(&'static str, f64, Option<&'static str>)>>);

    impl Decide for FakeDecider {
        fn decide<'a>(
            &'a self,
            _input: &'a str,
            questions: &'a [Question],
        ) -> futures_util::future::BoxFuture<'a, Result<Option<Decision>>> {
            let next = self.0.lock().unwrap().pop_front();
            let offered: Vec<String> = questions[0].options.iter().map(|o| o.0.clone()).collect();
            Box::pin(async move {
                let (step, probability, part) = next.expect("unexpected decision call");
                assert!(
                    offered.iter().any(|o| o == step),
                    "{step} not offered: {offered:?}"
                );
                let mut answers = std::collections::HashMap::new();
                answers.insert(
                    "step".to_string(),
                    crate::decide::Answer {
                        choice: step.into(),
                        probability,
                    },
                );
                if let Some(part) = part {
                    answers.insert(
                        "part".to_string(),
                        crate::decide::Answer {
                            choice: part.into(),
                            probability: 0.9,
                        },
                    );
                }
                Ok(Some(Decision {
                    by: "Fake".into(),
                    answers,
                }))
            })
        }
    }

    fn with_decisions(
        rt: Runtime,
        steps: Vec<(&'static str, f64, Option<&'static str>)>,
    ) -> Runtime {
        rt.with_decider(Arc::new(FakeDecider(Mutex::new(steps.into()))))
    }

    #[tokio::test]
    async fn decided_steps_skip_the_llm() {
        let (rt, _) = runtime(
            FakeEngine::new(vec![(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, MAP)]),
            false,
        );
        let rt = with_decisions(
            rt,
            vec![("known", 0.95, None), ("move", 0.9, Some("route"))],
        );
        turn(&rt, "goal").await;
        turn(&rt, "I already know routing").await;
        let s = rt.load_session("s").unwrap().unwrap();
        assert_eq!(s.map.get("route").unwrap().status, PartStatus::Known);
        assert_eq!(s.active.as_deref(), Some("report"));
        turn(&rt, "go back to routing").await;
        let s = rt.load_session("s").unwrap().unwrap();
        assert_eq!(s.active.as_deref(), Some("route"));
    }

    #[tokio::test]
    async fn a_question_about_a_waiting_candidate_skips_the_judge() {
        let (rt, engine) = runtime(
            FakeEngine::new(vec![
                (ReadOnly, S1, ONBOARD_READY),
                (ReadOnly, S2, MAP),
                (
                    ReadOnly,
                    S1,
                    r#"{"intent":"log","reply":"Ok.","map_change":null}"#,
                ),
                (ReadOnly, S2, r#"{"candidate":"z = 99","key_reason":"r"}"#),
                // Decided as a log entry: only the reply LLM runs, no judge.
                (
                    ReadOnly,
                    S1,
                    r#"{"intent":"summary","reply":"It means x.","map_change":null}"#,
                ),
                (ReadOnly, S2, r#"{"pass":true,"feedback":"Right."}"#),
            ]),
            false,
        );
        let rt = with_decisions(
            rt,
            vec![
                ("log", 0.9, None),
                ("summary", 0.8, None),
                ("log", 0.85, None),
                ("verdict", 0.9, None),
            ],
        );
        turn(&rt, "goal").await;
        turn(&rt, "thinking").await;
        turn(&rt, "I get it: names map to handlers").await;
        let out = turn(&rt, "what does dispatch mean?").await;
        // The LLM's own intent is ignored once the decision model has spoken.
        assert!(!out.reply.contains("Candidate"));
        let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
        assert!(prompt.contains("Fake classified this message as a log entry"));
        assert!(!prompt.contains("z = 99"));
        assert!(
            rt.load_session("s")
                .unwrap()
                .unwrap()
                .pending_candidate
                .is_some()
        );
        turn(&rt, "right, names map to handlers").await;
        let s = rt.load_session("s").unwrap().unwrap();
        assert_eq!(s.map.get("route").unwrap().status, PartStatus::Understood);
    }

    #[tokio::test]
    async fn an_unsure_decision_leaves_it_to_the_llm() {
        let (rt, _) = runtime(
            FakeEngine::new(vec![
                (ReadOnly, S1, ONBOARD_READY),
                (ReadOnly, S2, MAP),
                (
                    ReadOnly,
                    S1,
                    r#"{"intent":"known","reply":"","map_change":null}"#,
                ),
            ]),
            false,
        );
        let rt = with_decisions(rt, vec![("summary", 0.4, None)]);
        turn(&rt, "goal").await;
        turn(&rt, "hmm, I sort of know this").await;
        let s = rt.load_session("s").unwrap().unwrap();
        assert_eq!(s.map.get("route").unwrap().status, PartStatus::Known);
    }

    #[tokio::test]
    async fn a_summary_needs_a_log_first() {
        let (rt, _) = runtime(
            FakeEngine::new(vec![
                (ReadOnly, S1, ONBOARD_READY),
                (ReadOnly, S2, MAP),
                (
                    ReadOnly,
                    S1,
                    r#"{"intent":"summary","reply":"","map_change":null}"#,
                ),
            ]),
            false,
        );
        turn(&rt, "goal").await;
        let out = turn(&rt, "I get it: it routes by name").await;
        assert!(out.reply.contains("Noted in your log"));
        let s = rt.load_session("s").unwrap().unwrap();
        assert!(s.pending_candidate.is_none());
        assert_eq!(s.map.get("route").unwrap().log.len(), 1);
    }

    #[tokio::test]
    async fn two_kinds_of_skip() {
        let (rt, engine) = runtime(
            FakeEngine::new(vec![
                (ReadOnly, S1, ONBOARD_READY),
                (ReadOnly, S2, MAP),
                (Full, S2, "Wrote the report."),
            ]),
            false,
        );
        turn(&rt, "goal").await;
        turn(&rt, "/known").await;
        assert_eq!(status(&rt, "route"), PartStatus::Known);
        let out = turn(&rt, "/delegate").await;
        assert!(out.reply.contains("without understanding"));
        assert_eq!(status(&rt, "report"), PartStatus::Delegated);
        // No review for delegated parts.
        let s = rt.load_session("s").unwrap().unwrap();
        assert_eq!(s.mode, SessionMode::Done);
        assert!(engine.replies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn map_change_waits_for_your_ok() {
        let (rt, _) = runtime(
            FakeEngine::new(vec![
                (ReadOnly, S1, ONBOARD_READY),
                (ReadOnly, S2, MAP),
                (
                    ReadOnly,
                    S1,
                    r#"{"reply":"That needs the store.","map_change":{"brief":"Status reads the store","parts":[{"id":"store","title":"Session store","depends_on":[]}],"remove":[]}}"#,
                ),
            ]),
            false,
        );
        turn(&rt, "goal").await;
        let out = turn(&rt, "status should show saved sessions").await;
        assert!(out.reply.contains("/accept"));
        assert!(
            rt.load_session("s")
                .unwrap()
                .unwrap()
                .map
                .get("store")
                .is_none()
        );

        turn(&rt, "/accept").await;
        let s = rt.load_session("s").unwrap().unwrap();
        assert!(s.map.get("store").is_some());
        assert!(s.pending_change.is_none());
    }

    #[tokio::test]
    async fn accepted_change_can_reset_the_goal() {
        let (rt, _) = runtime(
            FakeEngine::new(vec![
                (ReadOnly, S1, ONBOARD_READY),
                (ReadOnly, S2, MAP),
                (
                    ReadOnly,
                    S1,
                    r#"{"reply":"That changes the task.","map_change":{"brief":"You need conv layers","goal":"Understand convolutional networks for an image classifier","parts":[{"id":"conv","title":"Conv layers","depends_on":[]}],"remove":[]}}"#,
                ),
            ]),
            false,
        );
        turn(&rt, "goal").await;
        let before = rt.load_session("s").unwrap().unwrap().goal;
        let out = turn(&rt, "I really need this for a CNN").await;
        assert!(out.reply.contains("Goal → Understand convolutional"));
        assert_eq!(rt.load_session("s").unwrap().unwrap().goal, before);

        turn(&rt, "/accept").await;
        let s = rt.load_session("s").unwrap().unwrap();
        assert_eq!(
            s.goal.as_deref(),
            Some("Understand convolutional networks for an image classifier")
        );
    }
}
