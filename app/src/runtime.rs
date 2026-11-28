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

mod cite;
mod commit;
mod concepts;
pub mod eval;
mod evidence;
mod gate;
mod handoff;
mod onboarding;
mod path;
mod provenance;
mod record;
mod refer;
mod review;
mod skips;

pub struct TurnInput {
    pub text: String,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct TurnOutput {
    pub reply: String,
    /// What the reply is, for a frontend that shows kinds apart.
    pub shown: Shown,
}

/// What a turn's reply is. `reply` holds it as markdown either way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Shown {
    /// The model's words, or a local view such as the map.
    #[default]
    Reply,
    /// A source `/open` showed: its path, the part shown
    /// (`lines 20–24 of 391`) and the numbered block in markdown.
    Source {
        title: String,
        range: String,
        body: String,
    },
    /// `/find` hits, the best first, and how many lines matched in all.
    Search {
        query: String,
        hits: Vec<crate::sources::Hit>,
        total: usize,
    },
    /// A local notice: nothing found, an error, how to use a command.
    Notice,
}

impl TurnOutput {
    fn text(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            shown: Shown::Reply,
        }
    }

    fn notice(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            shown: Shown::Notice,
        }
    }
}

pub const COMMANDS_HELP: &str = "Commands: `/find <terms>`, `/open <file> [page|line]`, `/web <terms>`, `/tell <question>`, `/check <your work>`, `/plan`, `/summary <text>`, `/handoff [notes]`, `/approve`, `/object <reason>`, `/known`, `/delegate [notes]`, `/accept`, `/reject`, `/replan [what changed]`, `/next`, `/focus <id|number|prefix>`, `/map`, `/why [--all]`, `/revisit <concept>`, `/coverage`.";

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

tokio::task_local! {
    /// The model list a turn read at its start (see [`Runtime::models`]).
    static TURN_CATALOG: Vec<ModelEntry>;
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
#[derive(Debug, Clone)]
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

/// What a review-checks level is called on screen. `normal` is the highest
/// level, so it reads as `full`; the stored id stays `normal`.
pub fn checks_label(id: &str) -> &str {
    match id {
        "normal" => "full",
        other => other,
    }
}

/// Main model effort levels offered in the picker.
pub const EFFORTS: [&str; 4] = ["default", "low", "medium", "high"];

/// A value from one of the thread's pickers or switches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionValue {
    Id(String),
    Bool(bool),
}

enum ValidatedOption {
    FastModel(String),
    MainModel(String),
    FastEffort(Option<String>),
    MainEffort(Option<String>),
    Decisions(bool),
    DecisionModel(String),
    Checks(Option<String>),
}

pub const NO_MODEL: &str = "No model is set up yet. Run `pathfinder auth login` in a terminal.";

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

    /// Reads settings from `config` instead of `config.json` (for tests).
    pub fn with_config(mut self, config: fn() -> Config) -> Self {
        self.config = config;
        self
    }

    pub fn with_decider(mut self, decider: Arc<dyn Decide>) -> Self {
        self.decider = decider;
        self
    }

    /// The thread's models. Inside a turn they come from the model list the
    /// turn read at its start, so a list refreshed in the background
    /// meanwhile applies from the next turn, never between two calls of one.
    fn models(&self, session: &Session) -> Result<TierModels> {
        let config = (self.config)();
        let catalog = TURN_CATALOG
            .try_with(Clone::clone)
            .unwrap_or_else(|_| config.catalog());
        let pick = |choice: &Option<String>, system2| {
            config
                .pick_in(&catalog, choice.as_deref(), system2)
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

    /// The settings this runtime reads (on every call, so edits apply).
    pub fn config(&self) -> Config {
        (self.config)()
    }

    /// Fast and main model: what the thread or config asks for and what
    /// runs, read fresh so a refreshed model list applies from the next turn.
    pub fn tier_picks(&self, session: &Session) -> [crate::config::TierPick; 2] {
        (self.config)().tiers(session.system1.as_deref(), session.system2.as_deref())
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
        match self.validate_option(option, value)? {
            ValidatedOption::FastModel(id) => session.system1 = Some(id),
            ValidatedOption::MainModel(id) => session.system2 = Some(id),
            ValidatedOption::FastEffort(level) => session.fast_effort = level,
            ValidatedOption::MainEffort(level) => session.effort = level,
            ValidatedOption::Decisions(on) => session.decisions_off = !on,
            ValidatedOption::DecisionModel(id) => {
                session.decision_model = Some(id);
                session.decisions_off = false;
            }
            ValidatedOption::Checks(level) => session.checks = level,
        }
        self.store.save_session(&session)?;
        Ok(session)
    }

    /// Saves one picker or switch as the default in `config.json` for future threads.
    pub fn set_default_option(&self, option: &str, value: OptionValue) -> Result<()> {
        let defaults = self.default_settings_for_option(option, value)?;
        Config::save_defaults(&defaults)
    }

    /// Saves the session's current model, effort, decision, and check settings
    /// as defaults in `config.json` for future threads.
    pub fn save_session_defaults(&self, session: &Session) -> Result<()> {
        let defaults = crate::config::DefaultSettings {
            system1: session.system1.as_ref().map(|s| Some(s.clone())),
            system2: session.system2.as_ref().map(|s| Some(s.clone())),
            effort: Some(session.effort.clone()),
            fast_effort: Some(session.fast_effort.clone()),
            decisions_on: Some(Some(!session.decisions_off)),
            decision_model: Some(session.decision_model.clone()),
            checks: Some(session.checks.clone()),
        };
        Config::save_defaults(&defaults)
    }

    fn default_settings_for_option(
        &self,
        option: &str,
        value: OptionValue,
    ) -> Result<crate::config::DefaultSettings> {
        let mut d = crate::config::DefaultSettings::default();
        match self.validate_option(option, value)? {
            ValidatedOption::FastModel(id) => d.system1 = Some(Some(id)),
            ValidatedOption::MainModel(id) => d.system2 = Some(Some(id)),
            ValidatedOption::FastEffort(level) => d.fast_effort = Some(level),
            ValidatedOption::MainEffort(level) => d.effort = Some(level),
            ValidatedOption::Decisions(on) => d.decisions_on = Some(Some(on)),
            ValidatedOption::DecisionModel(id) => {
                d.decision_model = Some(Some(id));
                d.decisions_on = Some(Some(true));
            }
            ValidatedOption::Checks(level) => d.checks = Some(level),
        }
        Ok(d)
    }

    fn validate_option(&self, option: &str, value: OptionValue) -> Result<ValidatedOption> {
        match (option, value) {
            (crate::options::FAST_MODEL | crate::options::MAIN_MODEL, OptionValue::Id(id)) => {
                if (self.config)().entry(&id).is_none() {
                    return Err(anyhow!("unknown model `{id}`"));
                }
                if option == crate::options::MAIN_MODEL {
                    Ok(ValidatedOption::MainModel(id))
                } else {
                    Ok(ValidatedOption::FastModel(id))
                }
            }
            (crate::options::MAIN_EFFORT | crate::options::FAST_EFFORT, OptionValue::Id(level)) => {
                if !EFFORTS.contains(&level.as_str()) {
                    return Err(anyhow!("unknown effort `{level}`"));
                }
                let val = (level != "default").then_some(level);
                if option == crate::options::MAIN_EFFORT {
                    Ok(ValidatedOption::MainEffort(val))
                } else {
                    Ok(ValidatedOption::FastEffort(val))
                }
            }
            // A switch, or an Off/On dropdown where switches aren't supported.
            (crate::options::DECISIONS, OptionValue::Bool(on)) => {
                Ok(ValidatedOption::Decisions(on))
            }
            (crate::options::DECISIONS, OptionValue::Id(v)) if v == "on" || v == "off" => {
                Ok(ValidatedOption::Decisions(v == "on"))
            }
            (crate::options::DECISION_MODEL, OptionValue::Id(id)) => {
                if Provider::from_id(&id).is_none() {
                    return Err(anyhow!("unknown decision model `{id}`"));
                }
                Ok(ValidatedOption::DecisionModel(id))
            }
            (crate::options::CHECKS, OptionValue::Id(level))
                if CHECKS.contains(&level.as_str()) =>
            {
                Ok(ValidatedOption::Checks((level != "light").then_some(level)))
            }
            (other, _) => Err(anyhow!("unknown option or value for `{other}`")),
        }
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
        let config = (self.config)();
        let mut session = Session::new(id, cwd);
        session.effort = config.effort;
        session.fast_effort = config.fast_effort;
        session.decisions_off = config.decisions_on == Some(false);
        session.decision_model = config.decision_model;
        session.checks = config.checks.filter(|c| c != "light");
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

    /// The threads in `cwd` with the seconds since each was last saved,
    /// the most recent first.
    pub fn threads_in(&self, cwd: &std::path::Path) -> Vec<(Session, u64)> {
        let mut all: Vec<(Session, u64)> = self
            .store
            .all_sessions_aged()
            .unwrap_or_default()
            .into_iter()
            .filter(|(s, _)| s.cwd == cwd)
            .collect();
        all.reverse();
        all
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
        // Thoughts and tool calls, kept with the reply they came before.
        let steps = Arc::new(std::sync::Mutex::new(crate::domain::StepLog::default()));
        let turns_before = session.history.len();
        let sink: StreamSink = {
            let tools = tools.clone();
            let steps = steps.clone();
            let inner = sink;
            Arc::new(move |event: StreamEvent| {
                {
                    let mut log = steps.lock().unwrap();
                    match &event {
                        StreamEvent::ToolStart(t) => {
                            *tools.lock().unwrap().entry(tool_kind(t.kind)).or_default() += 1;
                            log.tool_start(&t.id, &t.title);
                        }
                        StreamEvent::Thought(t) => log.thought(t),
                        StreamEvent::ToolUpdate { id, content } => {
                            log.tool_detail(id, content.summary())
                        }
                        StreamEvent::ToolEnd { id, ok } => log.tool_end(id, *ok),
                        StreamEvent::TextDelta(_) => {}
                    }
                }
                inner(event)
            })
        };
        let catalog = (self.config)().catalog();
        let (output, counters) = TURN_CATALOG
            .scope(
                catalog.clone(),
                crate::telemetry::with_turn_counters(self.dispatch_turn(
                    &mut session,
                    text,
                    files,
                    sink,
                )),
            )
            .await;
        // What a confirmation ran, if anything: counted as that step.
        let ran = session.ran_commitment.take();
        if crate::telemetry::enabled() {
            crate::telemetry::set_locale(session.language.as_deref().unwrap_or(""));
            let (route, model) = TURN_CATALOG
                .sync_scope(catalog, || self.models(&session))
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
            // A plain message answered with a fixed notice (a show-back, an
            // ask, an undo): no step of the loop ran yet.
            let notice = matches!(&output, Ok(o) if o.shown == Shown::Notice);
            let step = if notice && !text.starts_with('/') && ran.is_none() {
                pathfinder_schema::Step::Other
            } else {
                turn_step(text, &session, had_candidate, ran)
            };
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
        if output.is_err() {
            // A failed message still counts as a step: a claim is undoable
            // only right away, never past a message sent after it (§15).
            // The failed turn's own changes are not kept.
            if let Ok(Some(mut stored)) = self.store.load_session(session_id) {
                stored.turn += 1;
                let _ = self.store.save_session(&stored);
            }
        }
        let output = output?;
        keep_steps(&mut session, turns_before, steps.lock().unwrap().take());

        self.retire_stale_secrets(&session);
        // Counted once the turn is done, so a claim made in turn n is
        // undoable while `turn` is n + 1: in the next turn, and in a
        // frontend between the two (`Session::undoable`).
        session.turn += 1;
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
        follow_language(session, text);
        // A question back to you lasts one message: this one may answer
        // it; otherwise it is dropped and nothing was recorded (§15).
        let ask = session.pending_ask.take();
        if let Some(ask) = ask
            && !text.starts_with('/')
            && session.mode == SessionMode::Working
            && let Some(out) = self
                .answer_ask(session, ask, text, files, sink.clone())
                .await?
        {
            return Ok(out);
        }
        // "What do I actually understand now?" asks about your record: the
        // whole folder's, every idea you have shown so far, not only the
        // part you're on; never sources or a model (§8, A10).
        if !text.starts_with('/') && refer::asks_record(text) {
            session.push_turn("user", text);
            let why = self.folder_record(session);
            session.push_turn("assistant", &why);
            return Ok(TurnOutput::text(why));
        }
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
        let cmd = words.next().unwrap_or("").to_ascii_lowercase();
        let arg = words.next().unwrap_or("").trim();

        // Something waits for your OK (§15): a command that only looks
        // leaves it waiting; any other drops it with a "not run" line, as a
        // plain message does, so it never runs against a state it wasn't
        // shown back for.
        let looks = matches!(
            cmd.as_str(),
            "/confirm" | "/cancel" | "/why" | "/map" | "/coverage" | "/find" | "/open" | "/web"
        );
        let dropped = if looks {
            None
        } else {
            self.drop_pending(session)
        };
        if let Some(line) = &dropped {
            sink(StreamEvent::TextDelta(format!("{line}\n\n")));
        }
        let before = session.history.len();
        let out = self.run_command(session, &cmd, arg, files, sink).await;
        if let Some(line) = dropped
            && let Some(reply) = session
                .history
                .iter_mut()
                .skip(before)
                .find(|t| t.role == "assistant")
        {
            reply.content = format!("{line}\n\n{}", reply.content);
        }
        out
    }

    async fn run_command(
        &self,
        session: &mut Session,
        cmd: &str,
        arg: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        match cmd {
            "/map" => Ok(TurnOutput::text(render_map_and_pending(session))),
            "/plan" => {
                if !arg.is_empty() {
                    session.push_turn("user", arg);
                }
                self.build_map(session, files, sink).await
            }
            "/summary" => self.handle_summary(session, arg, files, sink).await,
            // A part you hand over yourself replaces an older queued request.
            "/handoff" => {
                Self::drop_queue(session);
                self.handle_handoff(session, arg, files, sink).await
            }
            // Catching a planted check finishes the part: the queue goes on.
            "/object" => {
                let out = self
                    .handle_object(session, arg, files, sink.clone())
                    .await?;
                self.run_queue(session, out, sink).await
            }
            "/approve" => {
                let out = self.handle_approve(session).await?;
                self.run_queue(session, out, sink).await
            }
            "/known" => self.handle_known(session, Some(arg)),
            "/delegate" => {
                Self::drop_queue(session);
                self.handle_delegate(session, arg, files, sink).await
            }
            "/accept" => self.handle_accept(session).await,
            "/why" => Ok(TurnOutput::text(self.why(session, arg))),
            "/revisit" => Ok(self.handle_revisit(session, arg)),
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
            "/tell" => self.handle_tell(session, arg, files, sink).await,
            "/check" => self.handle_check(session, arg, files, sink).await,
            "/find" => Ok(self.handle_find(session, arg).await),
            "/open" => Ok(self.handle_open(session, arg).await),
            "/web" => Ok(self.handle_web(session, arg, sink).await),
            "/undo" => self.undo(session),
            "/confirm" => self.confirm(session, sink).await,
            "/cancel" => self.cancel(session, sink).await,
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
                // The first question carries the exam's lead before its
                // header.
                .map(|t| {
                    let lead = i18n::line(session.language.as_deref(), i18n::Line::ExamLead);
                    t.content
                        .strip_prefix(lead)
                        .map_or(t.content.as_str(), str::trim_start)
                })
                .filter(|c| i18n::opens_with(c, i18n::Line::ExamQuestionFor))
                .filter_map(|c| {
                    let (_, q) = c.split_once("\n\n")?;
                    let q = q.rsplit_once("\n\n").map_or(q, |(q, _)| q);
                    Some(q.trim().to_string())
                }),
        )
        .collect()
}

/// The decision model's step for a verdict on a waiting candidate.
const VERDICT: &str = "verdict";

/// The Router's step for "let the agent do it" (§15): code picks the part
/// named, and a handoff or a delegation from whether your review of it
/// would count (§12), never a model.
const AGENT: &str = "agent";

/// The Router's step for "just tell me" (§11, §15).
const TELL: &str = "tell";

/// The Router's step for taking back the last claim (§15).
const UNDO: &str = "undo";

/// The Router's step for agreeing to what waits for your OK (§15).
const CONFIRM: &str = "confirm";

/// The Router's steps for your verdict on the agent's result in plain
/// words (§7): shown back for Enter, then run as `/approve` or `/object`.
const APPROVE: &str = "approve";
const OBJECT: &str = "object";

/// The steps that change what the record says about you (§15).
const COMMITMENTS: [&str; 4] = [VERDICT, "known", AGENT, TELL];

/// An unsure Router asks which you meant once this much of its weight is
/// on commitments (§15).
const UNSURE_COMMITMENT: f64 = 0.4;

/// An unsure Router also asks when it weighs any one commitment this much
/// (§15, A3: a commitment with meaningful probability).
const UNSURE_ONE: f64 = 0.25;

/// While a candidate waits, a log, move or skip needs this much to skip
/// the judge.
const CONFIDENT_STEP: f64 = 0.9;

/// Below this, the decision model's step is ignored and the LLM decides.
const DECISION_THRESHOLD: f64 = 0.6;

const CANDIDATE_HIDDEN: &str = "(candidate hidden while the user judges it)";
const REVIEW_HIDDEN: &str = "(the agent's changes, hidden while the user reviews them)";

pub(crate) const REVIEW_PROMPT: &str = "\n\nReview the result: say it looks good (`/approve`), or what's wrong and why to send it back (`/object <what's wrong and why>`).";

/// Keeps a turn's steps with the first reply it added to the history.
fn keep_steps(session: &mut Session, turns_before: usize, steps: Vec<crate::domain::Step>) {
    if steps.is_empty() {
        return;
    }
    if let Some(reply) = session
        .history
        .iter_mut()
        .skip(turns_before)
        .find(|t| t.role == "assistant")
    {
        reply.steps = steps;
    }
}

/// Makes sure the user has seen `full`: if streaming didn't show it, send it
/// now, so it stays ahead of anything that follows.
fn flush(sink: &StreamSink, full: &str, shown: &str) {
    if shown.is_empty() || shown.trim() != full.trim() {
        sink(StreamEvent::TextDelta(full.to_string()));
    }
}

/// A part as you start on it. The plain-words hint shows only with the
/// first part. The note is shown only as a pointer into the workspace
/// ("where to read"), and only when no next step points somewhere (§16:
/// one pointer, never two).
fn render_part(
    part: &Part,
    hint: bool,
    read_at: bool,
    lang: Option<&str>,
    cwd: &std::path::Path,
) -> String {
    let mut out = format!("**{}** (`{}`)", part.title, part.id);
    if read_at && let Some(at) = crate::sources::pointer_note(cwd, &part.note) {
        out.push_str(&format!("\n{} {at}", i18n::line(lang, i18n::Line::ReadAt)));
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

/// The conversation's language follows your own messages: a message
/// whose language is clear sets it (a command's argument counts, the
/// command doesn't); a short or mixed one leaves it as it was. Never the
/// system locale, and never a model's guess once it is set.
fn follow_language(session: &mut Session, text: &str) {
    let own = match text.strip_prefix('/') {
        Some(rest) => rest.split_once(char::is_whitespace).map_or("", |(_, a)| a),
        None => text,
    };
    if let Some(lang) = i18n::detect(own) {
        session.language = Some(lang);
    }
}

/// A system prompt for a model that writes text you read, with the
/// conversation's language spelled out.
fn in_language(system: &str, session: &Session) -> String {
    format!("{system}\n\n{}", i18n::rule(session.language.as_deref()))
}

impl Runtime {
    /// The recent conversation for a model's prompt, with a candidate waiting
    /// for your verdict hidden: it may be flawed on purpose, and a model that
    /// can see it tends to vouch for it (A2). A review still waiting for your
    /// verdict is hidden too, checked or not, so its planted hunk (if any)
    /// never reaches a model that may not see it (A2, §13). The one helper for
    /// every prompt but the Judge's, which is given the candidate on purpose.
    fn format_history(&self, session: &Session) -> String {
        mask_history(session, &self.waiting_reviews(session))
    }

    /// The turns that show a review still waiting for your verdict. Every
    /// review gets a secret, so this reads the same whether a review is
    /// checked or not.
    fn waiting_reviews(&self, session: &Session) -> Vec<usize> {
        self.store
            .active_secrets(&session.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.kind == "planted_flaw" || s.kind == "review")
            .map(|s| s.opened_at_turn)
            .collect()
    }
}

/// `format_history`, given the turns that show a review still waiting.
fn mask_history(session: &Session, reviews: &[usize]) -> String {
    let start = session.history.len().saturating_sub(12);
    let turns: Vec<crate::domain::TurnRecord> = session.history[start..]
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut t = t.clone();
            if t.role == "assistant" && reviews.contains(&(start + i)) {
                t.content = REVIEW_HIDDEN.to_string();
            }
            t
        })
        .collect();
    let turns = format_turns(&turns);
    match &session.pending_candidate {
        Some(p) if !p.candidate.trim().is_empty() => turns.replace(&p.candidate, CANDIDATE_HIDDEN),
        _ => turns,
    }
}

fn format_turns(turns: &[crate::domain::TurnRecord]) -> String {
    const MAX_TURN_CHARS: usize = 4000;
    const MAX_SOURCES: usize = 8;

    if turns.is_empty() {
        return "(none)".to_string();
    }
    turns
        .iter()
        .map(|t| {
            let content = if t.content.chars().count() > MAX_TURN_CHARS {
                let kept: String = t.content.chars().take(MAX_TURN_CHARS).collect();
                format!("{kept}…")
            } else {
                t.content.clone()
            };
            let sources = if t.role == "assistant" && t.content != REVIEW_HIDDEN {
                let mut seen = Vec::new();
                for s in &t.steps {
                    if let crate::domain::Step::Tool {
                        title,
                        ok: Some(true),
                        ..
                    } = s
                        && (title.starts_with("Read ")
                            || title.starts_with("View ")
                            || title.starts_with("Grep ")
                            || title.starts_with("List ")
                            || title.starts_with("Glob "))
                        && !seen.contains(title)
                    {
                        seen.push(title.clone());
                        if seen.len() >= MAX_SOURCES {
                            break;
                        }
                    }
                }
                if seen.is_empty() {
                    String::new()
                } else {
                    format!(" [sources: {}]", seen.join(", "))
                }
            } else {
                String::new()
            };
            format!("{}{sources}: {content}", t.role)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const ONBOARDING_SYSTEM: &str = r#"You run onboarding for Pathfinder, a work agent that makes sure the user understands a task before the work is handed to an agent.

Onboarding only sets the goal. Right after it, Pathfinder sketches a rough map of what the task depends on. The map has no size limit and grows and changes during the work, so scope, depth and details are settled there, not here. Do not plan, teach or do the task.

First decide what kind of message this is, by what the user means, in any wording or language; the examples below only illustrate. Pathfinder is above all the user's everyday assistant; most messages are questions, and answering is the default:
- A question or request (set `question` true, `ready_to_map` false, `goal` null, `reply` ""): anything they want answered, explained, summarised or looked up ("what is a phasor?", "explain this slide", "how do I derive this?", "what do I need to know from slide set 4?", "give me the solution to exercise 1a", "what's the capital of Peru?"). Also their own work shown for review ("I solved exercise 2, is this right?", a photo of their solution). Asking for a solution is a question, not a task: the user decides. Set `offer_check` to true when it's about understanding something, false for lookups and chit-chat.
- A quick check (`check` and `ready_to_map` true): the user signals, however they put it, that they now understand what was just explained (e.g. "ah, now it clicks"). That claim is the moment to check it. The goal starts with "Check" (in their language: "Prüfen, ...") and names the topic just explained, narrowly. Thanks or an acknowledgement without any claim of understanding is chit-chat: a question with `offer_check` false.
- An exam (`exam` and `ready_to_map` true): they want to be tested or examined on a topic, in any form (e.g. practising for an oral exam, being quizzed on a chapter). The goal starts with "Examine" (in their language: "Abfragen, ...") and names the topic: the one they name, else the one just discussed.
- A task (`ready_to_map` true, `check` and `exam` false): only when they want to work through something themselves ("I want to solve exercise 1 myself, help me without giving it away", "ich will das Kapitel richtig lernen") or want work done in the workspace (build, fix, change something). Not for questions about a topic, however big. Writing or saving something in the workspace (a letter, a file) is a task: never reply that you can't write or that access is read-only.

Always set `language` to the two-letter code of the language the user writes in (e.g. "de", "en"), and write `goal` and `reply` in that language.

Otherwise it's onboarding for a task. Aim to finish in this turn:
- If the user has named something to do, build, fix, understand or learn, the goal is clear: set `ready_to_map` to true and write `goal` as one sentence starting with a verb, in their words but not in first person ("Learn how neurons in neural networks work"). Breadth or vagueness ("learn how X works") is fine; the map handles it.
- Ask one short question only when you can't tell what the task is at all (e.g. "orient yourself", "help me with my thesis"). If the workspace has content, ground it: say in one sentence what the workspace is (from the snapshot and any files worth reading, such as a README), then ask what they want to do there. If it's empty, just ask. Never ask the user to confirm, choose a scope, or say what they want to be able to explain.
- If the user says what they already know or don't ("I'm new to this", "I know the basics"), keep it in `goal` (e.g. "... starting from the basics").

`reply`: when ready, just the goal in one sentence (the map is shown right after it; don't announce it). When asking, at most two sentences. Write math as plain text or Unicode (x², √, ≤, μᵢ), never LaTeX.

You get a workspace snapshot. If a file could answer something, read it before writing your JSON, since `reply` ends your turn. Don't read files when the snapshot and the user's words already settle it.

Return ONLY a JSON object:
{"reply": "...", "ready_to_map": true, "goal": "...", "question": false, "offer_check": false, "check": false, "exam": false, "language": "en"}"#;

const MAP_SYSTEM: &str = r#"You sketch the map for Pathfinder: what the task depends on.

- A part is something the task depends on: a piece of the work, or of the understanding needed to judge the work. It is not a lesson and not an assignment: the user decides during the work which parts they do themselves and which go to an agent.
- No size limit. Use as many parts as the task needs and no more; a small task can be a single part.
- `depends_on` lists parts that must be understood or done before this one makes sense. No cycles.
- `id`: a short descriptive slug the user types in commands: lowercase words joined by hyphens, at most 12 characters (e.g. `weights`, `fwd-pass`, `loss`). Unique. It names what the part is actually about, not a broad field: `colliders`, not `probability`. Users skip parts by their names.
- Start where the user is: if the goal or conversation says what they already know or don't, shape the parts for that (e.g. include the basics for a newcomer). Keep parts they say they know on the map; they can mark them known themselves.
- `note`: where to read for this part: a workspace file and page or line (e.g. `slides-4.pdf p. 12`, `notes.md:40`), or "" if the workspace has nothing for it. Never an explanation or a hint about the content. `brief`: what the map covers, in one or two sentences. In the brief, titles and labels, write math as plain text or Unicode (x², √, ≤, μᵢ), never LaTeX.
- A goal that is a check of a topic just explained in the conversation ("Check ...", "Prüfen, ...") gets one part for that topic, two at most: the user explains it and judges a candidate.
- Never give results away: no answers, computed values or intermediate results (a cutoff frequency, a derivative, "≈ 0.157") in the brief, notes or titles. The user works them out; for an exercise, say what each part is about, not what it comes to.
- `concepts`: the ideas the parts rest on, the record Pathfinder keeps of what the user has shown they understand. Each has an `id` (a lowercase hyphenated slug naming the idea itself, e.g. `d-separation`, `chain-rule`), a short `label`, a short topic `domain` tag (e.g. `electromagnetism`, `rust-async`, `probability` — never a file path or repo name) to keep homonyms in different fields apart, and `needs`: the concept ids it builds on. Include a concept the task needs even if no part covers it (a prerequisite the user may lack). When the prompt lists concepts already recorded, a part that rests on one of those ideas lists that id exactly (and you don't redeclare it), even if you'd word it differently: the user's checks count only under the same id. Only reuse an id when it is the same idea in the same domain; declare a new concept for an idea none of them is.
- Each part lists its 1–4 core `concepts` (ids from `concepts`): what someone must understand to judge an agent's work on that part. Usually one or two.

Return ONLY a JSON object:
{"brief": "...", "concepts": [{"id": "weighted-sum", "label": "Weighted sum of inputs", "domain": "neural-networks", "needs": []}], "parts": [{"id": "weights", "title": "...", "note": "...", "depends_on": [], "concepts": ["weighted-sum"]}]}"#;

const CHANGE_SYSTEM: &str = r#"You propose a change to Pathfinder's map of what the task depends on, because something changed. The user accepts or rejects it.

- `parts`: parts to add, or existing parts to update (same id), each with id, title, note, depends_on and its 1–4 core `concepts` (concept ids). New concepts go in `concepts` (id, label, domain, needs), as in the map. Reuse the id of a concept on the map or already recorded (listed in the prompt) when a part rests on that same idea in the same domain; declare a new one only for an idea none of them is. New ids: `id`: a short descriptive slug the user types in commands: lowercase words joined by hyphens, at most 12 characters (e.g. `weights`, `fwd-pass`, `loss`). Unique.
- `remove`: ids of parts to drop. Only parts with no work or outcome can be removed.
- No size limit. No cycles.

- `goal`: only when what changed alters what the task is (a new purpose or scope): the new goal as one sentence starting with a verb, not in first person. Otherwise null.
`brief`: one sentence to the user, saying "you", on what changes and why. In the brief and titles, write math as plain text or Unicode (x², √, ≤, μᵢ), never LaTeX.

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
- `agent`: they ask for the agent to do this part or another one (put its id in `part`), or for something to be written, saved or changed in their files ("do it for me", "hand it off", "write the letter"). You can't write files and the agent can, so such a request is always `agent`, never `log` or `tell`.
- `tell`: they ask to be told the answer or an explanation outright, rather than pointed to sources.
For anything but `log`, set `reply` to "" and `map_change` to null: Pathfinder takes the step.

For `log`, set `reply` to "ok". Pathfinder never answers in its own words: a question gets pointers into the sources, found separately. For `log`, also write, in the user's language:
- `ack`: for a log entry (not a question), one short sentence, at most 20 words, saying "you", that names what they just did in their own terms ("You linked Maxwell's equations to the Helmholtz equation."). It adds nothing: no fact, no explanation, no correction, no praise, no judgment of whether they are right, no hint. "" for a question.
- `missing`: for a log entry (not a question), if the part still has uncovered aspects or their answer left something out, one short sentence, at most 25 words, naming what is still missing or unaddressed for this part — without giving the answer, judging correctness, or hinting at a waiting candidate. "" for a question or if nothing is missing.
- `question`: one open question for their next step, at most 25 words, written from this part and the place in its sources they should look next (the part's note, the file and page you read). It makes them think further about what they wrote, never contains or hints at the answer, and reads as a natural, grammatical sentence (don't paste a long name into a template). "" if you can't write one.
- In `ack`, `missing` and `question`, write math as plain text or Unicode (x², √, ≤, μᵢ), never LaTeX.

If a `log` message reveals the map is wrong (a missing dependency, a part that doesn't belong, a wrong edge, or a new constraint that changes the task's scope), propose a change in `map_change`; the user accepts or rejects it. Otherwise set it to null. A change has `brief` (one sentence to the user, saying "you"), `goal` (only when the message changes what the task is, e.g. a new purpose or scope: the new goal as one sentence starting with a verb, not in first person; otherwise null), `parts` (to add, or update by id: id, title, note, depends_on; new ids are short hyphenated slugs of at most 12 characters) and `remove` (ids of parts with no work yet).

If the message shows a gap in something this part depends on, especially a part they delegated or claimed to know (see the map's statuses), propose a small part for it in `map_change` so the gap is on the map. The change's `brief` names the gap; it doesn't explain it.

For `log`, also record what the message is about, without judging it:
- `concepts`: the concepts it touches, as concept ids from the lists (from any part, not part ids). A short name only for an idea on neither list. Empty if none.
- In `misconception`, `concept` is likewise a concept id from the lists when one fits.
- `misconception`: only if the message plainly states a wrong belief (not a question, not a guess they're testing): `{"concept": id or name, "statement": the belief in one sentence}`. Otherwise null. A separate check confirms it; it never changes what you reply.

Call `read` before writing your JSON if a file matters: `reply` ends your turn. If they ask about a figure, diagram, slide or page, open the file and look at that page first (with `view`, or your file tool for PDFs and images); never say you can't see a file that is attached or in the workspace.

Return ONLY a JSON object:
{"intent": "log", "part": null, "reply": "ok", "ack": "...", "missing": "...", "question": "...", "map_change": null, "concepts": [], "misconception": null}
or, with a change:
{"intent": "log", "part": null, "reply": "ok", "ack": "...", "missing": "...", "question": "...", "map_change": {"brief": "...", "goal": null, "parts": [], "remove": []}, "concepts": [], "misconception": null}"#;

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

`feedback`: reveal the key and say what their judgment got right or missed, in the language the user writes in. Write math as plain text or Unicode (x², √, ≤, μᵢ), never LaTeX.

`misconception`: when they fail because they hold a wrong belief (not just a slip or "don't know"), state that belief in one sentence; otherwise null.

If the message isn't a verdict on the candidate at all (a question about it or the part, a thought for their log, a request to skip or move on), set `is_judgment` to false and `feedback` to "": it is handled as chat and the candidate keeps waiting. "I don't know" or "not sure" is a verdict: it fails.

Return ONLY a JSON object:
{"is_judgment": true, "pass": false, "feedback": "...", "misconception": null}"#;

const AGENT_SYSTEM: &str = r#"You do the work for one part of a task in the user's workspace, using the tools available.

Work in the open: before each step, say in one short sentence what you're about to do and why, then do it. Read and search before changing files; change them with `edit` (or `write` for new files); run builds, tests, installs and git with `bash`. The user approves each command and watches it run. If they decline one, find another way or explain what's blocked.

Stay within this part. End with a report of what you changed or found, concrete enough for the user to review. In what you say and in the report, write math as plain text or Unicode (x², √, ≤, μᵢ), never LaTeX. Files you write keep the notation they already use."#;

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

/// What kind of turn this was, for usage events. `pending`: what waited
/// for your OK when the turn began, so `/confirm` counts as what it ran.
fn turn_step(
    text: &str,
    session: &Session,
    had_candidate: bool,
    ran: Option<crate::domain::CommitKind>,
) -> pathfinder_schema::Step {
    use crate::domain::CommitKind;
    use pathfinder_schema::Step;
    // A confirmation, typed or in plain words, counts as what it ran; one
    // that ran nothing (stale, or nothing waiting) is no step of the loop.
    match ran {
        Some(CommitKind::Verdict) => return Step::Verdict,
        Some(CommitKind::Handoff) => return Step::Handoff,
        Some(CommitKind::Delegate) => return Step::Delegate,
        Some(CommitKind::Approve | CommitKind::Object) => return Step::Other,
        None => {}
    }
    if let Some(cmd) = text.strip_prefix('/') {
        let word = cmd
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        return match word.as_str() {
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
mod format_tests {
    use super::*;
    use crate::domain::{Step, TurnRecord};

    #[test]
    fn format_turns_includes_read_sources_and_respects_hidden_reviews() {
        let turns = vec![
            TurnRecord {
                role: "user".into(),
                content: "Explain slide 24".into(),
                steps: vec![],
            },
            TurnRecord {
                role: "assistant".into(),
                content: "You noted the boundary condition.".into(),
                steps: vec![
                    Step::Tool {
                        id: "1".into(),
                        title: "Read FUB_Folien1.pdf (page 24)".into(),
                        ok: Some(true),
                        detail: String::new(),
                    },
                    Step::Tool {
                        id: "2".into(),
                        title: "Read FUB_Folien1.pdf (page 24)".into(),
                        ok: Some(true),
                        detail: String::new(),
                    },
                    Step::Tool {
                        id: "3".into(),
                        title: "Read missing.pdf".into(),
                        ok: Some(false),
                        detail: String::new(),
                    },
                ],
            },
            TurnRecord {
                role: "assistant".into(),
                content: REVIEW_HIDDEN.into(),
                steps: vec![Step::Tool {
                    id: "4".into(),
                    title: "Read secret.rs".into(),
                    ok: Some(true),
                    detail: String::new(),
                }],
            },
        ];
        let out = format_turns(&turns);
        assert!(out.contains("assistant [sources: Read FUB_Folien1.pdf (page 24)]: You noted"));
        assert!(!out.contains("missing.pdf"));
        assert!(!out.contains("secret.rs"));
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod command_tests {
    use super::*;
    use crate::engine::{BoxFuture, noop_sink};

    struct UnusedEngine;

    impl Engine for UnusedEngine {
        fn run_text<'a>(
            &'a self,
            _req: &'a EngineRequest,
            _sink: StreamSink,
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { unreachable!("command should not call engine") })
        }
    }

    fn cfg_with_defaults() -> Config {
        Config {
            models: vec![
                ModelEntry {
                    id: "fast".into(),
                    label: "Fast".into(),
                    route: crate::config::Route::Anthropic,
                    model: "fast".into(),
                    effort: None,
                    endpoint: None,
                },
                ModelEntry {
                    id: "main".into(),
                    label: "Main".into(),
                    route: crate::config::Route::Anthropic,
                    model: "main".into(),
                    effort: None,
                    endpoint: None,
                },
            ],
            system1: Some("fast".into()),
            system2: Some("main".into()),
            effort: Some("high".into()),
            fast_effort: Some("low".into()),
            decisions_on: Some(false),
            decision_model: Some("openai".into()),
            checks: Some("normal".into()),
            discover: false,
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn runtime_commands_are_case_insensitive() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let mut rt = Runtime::new(store, Arc::new(UnusedEngine));
        rt.config = cfg_with_defaults;
        let s = rt.create_session("s1".into(), PathBuf::from(".")).unwrap();
        assert_eq!(s.effort.as_deref(), Some("high"));
        assert_eq!(s.fast_effort.as_deref(), Some("low"));
        assert!(s.decisions_off);
        assert_eq!(s.decision_model.as_deref(), Some("openai"));
        assert_eq!(s.checks.as_deref(), Some("normal"));

        let out_map = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "/MAP".into(),
                    files: vec![],
                },
                noop_sink(),
            )
            .await
            .unwrap();
        assert!(!out_map.reply.contains("Commands:"));

        let out_why = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "/Why".into(),
                    files: vec![],
                },
                noop_sink(),
            )
            .await
            .unwrap();
        assert!(!out_why.reply.contains("Commands:"));

        assert_eq!(
            turn_step("/MAP", &s, false, None),
            pathfinder_schema::Step::Map
        );
        assert_eq!(
            turn_step("/Summary", &s, false, None),
            pathfinder_schema::Step::Summary
        );
    }

    #[test]
    fn default_settings_conversion_validates_options() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let mut rt = Runtime::new(store, Arc::new(UnusedEngine));
        rt.config = cfg_with_defaults;

        let d = rt
            .default_settings_for_option(
                crate::options::MAIN_EFFORT,
                OptionValue::Id("default".into()),
            )
            .unwrap();
        assert_eq!(d.effort, Some(None));

        let d = rt
            .default_settings_for_option(
                crate::options::DECISION_MODEL,
                OptionValue::Id("openai".into()),
            )
            .unwrap();
        assert_eq!(d.decision_model, Some(Some("openai".into())));
        assert_eq!(d.decisions_on, Some(Some(true)));

        assert!(
            rt.default_settings_for_option(
                crate::options::MAIN_MODEL,
                OptionValue::Id("missing".into()),
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn open_pairs_history_and_does_not_pollute_concepts() {
        let dir = std::env::temp_dir().join(format!("pf-open-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("my notes.md"), "alpha\nbeta\ngamma\n").unwrap();

        let store = Arc::new(Store::open_in_memory().unwrap());
        let rt = Runtime::new(store.clone(), Arc::new(UnusedEngine));
        let s = rt.create_session("s-open".into(), dir.clone()).unwrap();

        let out = rt
            .handle_turn(
                "s-open",
                TurnInput {
                    text: r#"/open "my notes.md":2"#.into(),
                    files: vec![],
                },
                crate::engine::noop_sink(),
            )
            .await
            .unwrap();
        assert!(out.reply.contains("    2  beta"), "{}", out.reply);
        // A frontend can show it as a source: path, range, numbered block.
        let Shown::Source { title, range, body } = &out.shown else {
            panic!("{:?}", out.shown);
        };
        assert_eq!(title, "my notes.md");
        assert_eq!(range, "lines 1–3 of 3");
        assert!(body.starts_with("```markdown numbered\n"), "{body}");

        let reloaded = rt.load_session("s-open").unwrap().unwrap();
        assert_eq!(reloaded.history.len(), 2);
        assert_eq!(reloaded.history[0].role, "user");
        assert_eq!(reloaded.history[1].role, "assistant");
        assert_eq!(reloaded.history[1].content, out.reply);

        // Error case also keeps user/assistant paired.
        let err_out = rt
            .handle_turn(
                "s-open",
                TurnInput {
                    text: "/open missing.md".into(),
                    files: vec![],
                },
                crate::engine::noop_sink(),
            )
            .await
            .unwrap();
        assert!(err_out.reply.contains("missing.md"), "{}", err_out.reply);
        assert_eq!(err_out.shown, Shown::Notice, "an error isn't a reply");

        let reloaded = rt.load_session("s-open").unwrap().unwrap();
        assert_eq!(reloaded.history.len(), 4);
        assert_eq!(reloaded.history[3].role, "assistant");

        // Raw filename does not pollute `concepts` table.
        let ws = crate::understanding::workspace_key(&s.cwd);
        assert!(store.concepts(&ws).unwrap().is_empty());
        // Read event is still recorded under `sources`.
        let events = store.events(&ws, &["sources".to_string()]).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Read);
        assert_eq!(events[0].reference.as_deref(), Some("my notes.md#2"));

        // `/find`: the hits and how many lines matched in all.
        let found = rt
            .handle_turn(
                "s-open",
                TurnInput {
                    text: "/find beta".into(),
                    files: vec![],
                },
                crate::engine::noop_sink(),
            )
            .await
            .unwrap();
        let Shown::Search { query, hits, total } = &found.shown else {
            panic!("{:?}", found.shown);
        };
        assert_eq!((query.as_str(), hits.len(), *total), ("beta", 1, 1));
        assert!(found.reply.starts_with("**1** for"), "{}", found.reply);
        let none = rt
            .handle_turn(
                "s-open",
                TurnInput {
                    text: "/find zzzqqq".into(),
                    files: vec![],
                },
                crate::engine::noop_sink(),
            )
            .await
            .unwrap();
        assert_eq!(none.shown, Shown::Notice);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
