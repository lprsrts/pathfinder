use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::critic::{check_coach, check_onboarding, check_plan_proposal, check_verdict};
use crate::domain::{
    CheckDecision, CheckVerdict, CoachAction, CoachDecision, ConceptStatus, ItemStatus,
    OnboardingDecision, PathItem, PlanGraph, PlanProposal, Session, SessionMode,
};
use crate::engine::{
    Engine, EngineRequest, EngineTier, StreamEvent, StreamSink, ToolAccess, run_gated,
};
use crate::store::Store;

const ESCALATE_AFTER_TURNS: u32 = 4;

pub struct TurnInput {
    pub text: String,
    pub files: Vec<PathBuf>,
}

pub struct TurnOutput {
    pub reply: String,
    pub plan_changed: bool,
}

pub struct Runtime {
    store: Arc<Store>,
    engine: Arc<dyn Engine>,
}

impl Runtime {
    pub fn new(store: Arc<Store>, engine: Arc<dyn Engine>) -> Self {
        Self { store, engine }
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

        let trimmed = input.text.trim();
        let output = if trimmed.starts_with('/') {
            self.handle_command(&mut session, trimmed, &input.files, sink)
                .await?
        } else {
            match session.mode {
                SessionMode::Onboarding => {
                    self.handle_onboarding(&mut session, trimmed, &input.files, sink)
                        .await?
                }
                SessionMode::Working => {
                    self.handle_working(&mut session, trimmed, &input.files, sink)
                        .await?
                }
                SessionMode::Done => TurnOutput {
                    reply: "Everything on this path is resolved. Run `/replan <new goal>` to extend it or start a new session."
                        .to_string(),
                    plan_changed: false,
                },
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
        let mut parts = raw.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();

        match cmd {
            "/map" => Ok(TurnOutput {
                reply: session.plan.render_map(session.active_item.as_deref()),
                plan_changed: false,
            }),
            "/concepts" => Ok(TurnOutput {
                reply: session.render_concepts(),
                plan_changed: false,
            }),
            "/plan" => {
                if !arg.is_empty() {
                    session.push_turn("user", arg);
                }
                self.build_initial_plan(session, files, sink).await
            }
            "/check" => self.handle_check(session, arg, files, sink).await,
            "/next" => self.handle_next(session),
            "/handoff" => self.handle_handoff(session, arg, files, sink).await,
            "/skip" => self.handle_skip(session, arg, files, sink).await,
            "/replan" => self.handle_replan(session, arg, files, sink).await,
            "/focus" => self.handle_focus(session, arg),
            _ => Ok(TurnOutput {
                reply: "Commands: `/plan`, `/check [answer]`, `/next`, `/handoff [notes]`, `/skip [notes]`, `/replan [what changed]`, `/map`, `/concepts`, `/focus <id>`."
                    .to_string(),
                plan_changed: false,
            }),
        }
    }

    async fn handle_onboarding(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        session.push_turn("user", text);

        let folder_summary = summarize_folder(&session.cwd);
        let history = format_history(session);
        let concepts_summary = session.render_concepts();

        let req = EngineRequest {
            tier: EngineTier::System1,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: ONBOARDING_SYSTEM.to_string(),
            prompt: format!(
                "Workspace folder snapshot:\n{folder_summary}\n\nKnown concept state so far:\n{concepts_summary}\n\nConversation so far:\n{history}\n\nRespond with JSON only."
            ),
            files: files.to_vec(),
            stream_text: false,
        };

        let session_snapshot = session.clone();
        let decision: OnboardingDecision = run_gated(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "onboarding",
            &req,
            sink.clone(),
            |d: &OnboardingDecision| check_onboarding(d, &session_snapshot),
        )
        .await?;

        if let Some(goal) = decision.goal_summary {
            if !goal.trim().is_empty() {
                session.goal = Some(goal);
            }
        }
        if let Some(start) = decision.starting_point {
            if !start.trim().is_empty() {
                session.starting_point = Some(start);
            }
        }
        for c in &decision.observed_concepts {
            session.upsert_concept(&c.id, Some(&c.label), c.status, None);
        }

        if decision.ready_to_plan {
            session.push_turn("assistant", &decision.reply);
            return self.build_initial_plan(session, files, sink).await;
        }

        session.push_turn("assistant", &decision.reply);
        Ok(TurnOutput {
            reply: decision.reply,
            plan_changed: false,
        })
    }

    async fn build_initial_plan(
        &self,
        session: &mut Session,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let folder_summary = summarize_folder(&session.cwd);
        let history = format_history(session);
        let concepts_summary = session.render_concepts();
        let goal = session
            .goal
            .as_deref()
            .unwrap_or("(infer from conversation)");
        let starting = session
            .starting_point
            .as_deref()
            .unwrap_or("(infer from conversation and folder)");

        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: PLANNER_SYSTEM.to_string(),
            prompt: format!(
                "Goal: {goal}\nStarting point: {starting}\n\nTracked concept state:\n{concepts_summary}\n\nWorkspace folder snapshot:\n{folder_summary}\n\nOnboarding conversation:\n{history}\n\nBuild a sound dependency graph as JSON only."
            ),
            files: files.to_vec(),
            stream_text: false,
        };

        let concepts_snapshot = session.concepts.clone();
        let proposal: PlanProposal = run_gated(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "plan",
            &req,
            sink,
            |p: &PlanProposal| check_plan_proposal(p, &[], &concepts_snapshot),
        )
        .await?;

        let (brief, graph) = proposal
            .into_graph(&session.concepts)
            .map_err(|e| anyhow!(e))?;

        for item in &graph.items {
            for cid in &item.introduces {
                if !session.concepts.contains_key(cid) {
                    session.upsert_concept(cid, None, ConceptStatus::Unknown, None);
                }
            }
        }

        session.plan = graph;
        let active_id = session.advance_active();

        let reply = match active_id.as_deref().and_then(|id| session.plan.get(id)) {
            Some(first) => format!("{brief}\n\n{}", render_active_prompt(first)),
            None => brief,
        };

        session.push_turn("assistant", &reply);
        Ok(TurnOutput {
            reply,
            plan_changed: true,
        })
    }

    async fn handle_working(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let Some(active) = session.active().cloned() else {
            let next = session.advance_active();
            return Ok(TurnOutput {
                reply: match next.and_then(|id| session.plan.get(&id).cloned()) {
                    Some(item) => render_active_prompt(&item),
                    None => "Everything on this path is resolved.".to_string(),
                },
                plan_changed: true,
            });
        };

        if active.status == ItemStatus::Understood {
            return Ok(TurnOutput {
                reply: format!(
                    "You already passed **{}**. Run `/handoff` to let me finish remaining labor on it, or `/next` to move on.",
                    active.title
                ),
                plan_changed: false,
            });
        }

        session.push_turn("user", text);
        session.turns_on_active += 1;

        let force_s2 = session.turns_on_active >= ESCALATE_AFTER_TURNS;
        let tier = if force_s2 {
            EngineTier::System2
        } else {
            EngineTier::System1
        };

        let decision = self
            .run_coach_turn(session, &active, tier, files, sink.clone())
            .await?;

        if !force_s2 && decision.action == CoachAction::Escalate {
            sink(StreamEvent::Thought(
                "System 1 escalated coaching to System 2...\n".to_string(),
            ));
            let escalated = self
                .run_coach_turn(session, &active, EngineTier::System2, files, sink)
                .await?;
            session.turns_on_active = 0;
            session.push_turn("assistant", &escalated.reply);
            return Ok(TurnOutput {
                reply: escalated.reply,
                plan_changed: false,
            });
        }

        if decision.action == CoachAction::NeedReplan {
            return self.handle_replan(session, text, files, sink).await;
        }

        if force_s2 {
            session.turns_on_active = 0;
        }

        let mut reply = decision.reply;
        if decision.action == CoachAction::SuggestCheck {
            reply.push_str("\n\nReady to verify? Run `/check` with your answer or edits.");
        }

        session.push_turn("assistant", &reply);
        Ok(TurnOutput {
            reply,
            plan_changed: false,
        })
    }

    async fn run_coach_turn(
        &self,
        session: &Session,
        active: &PathItem,
        tier: EngineTier,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<CoachDecision> {
        let history = format_history(session);
        let concepts = session.render_concepts();
        let req = EngineRequest {
            tier,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: COACH_SYSTEM.to_string(),
            prompt: format!(
                "Goal: {}\nCurrent focus: {} (`{}`)\nFraming given: {}\nUser task: {}\nCriteria for understanding: {}\n\nConcept state:\n{concepts}\n\nRecent conversation:\n{history}\n\nRespond with JSON only.",
                session.goal.as_deref().unwrap_or("(session goal)"),
                active.title,
                active.id,
                active.framing,
                active.user_task,
                active.check_criteria
            ),
            files: files.to_vec(),
            stream_text: false,
        };

        run_gated(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "coach",
            &req,
            sink,
            check_coach,
        )
        .await
    }

    async fn handle_check(
        &self,
        session: &mut Session,
        answer: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let Some(active) = session.active().cloned() else {
            return Ok(TurnOutput {
                reply: "No active item to check yet.".to_string(),
                plan_changed: false,
            });
        };

        if !answer.is_empty() {
            session.push_turn("user", format!("/check {answer}"));
        }

        let history = format_history(session);
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: CHECK_SYSTEM.to_string(),
            prompt: format!(
                "Checking item: {} (`{}`)\nIntroduces concepts: {}\nUser task: {}\nCriteria for understanding: {}\nUser submission: {}\n\nRecent conversation:\n{history}\n\nInspect workspace files if relevant, then return JSON only.",
                active.title,
                active.id,
                active.introduces.join(", "),
                active.user_task,
                active.check_criteria,
                if answer.is_empty() {
                    "(check recent conversation and workspace files)"
                } else {
                    answer
                }
            ),
            files: files.to_vec(),
            stream_text: false,
        };

        let check: CheckDecision = run_gated(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "check",
            &req,
            sink,
            check_verdict,
        )
        .await?;

        let mut plan_changed = false;
        let reply = match check.verdict {
            CheckVerdict::Pass => {
                if let Some(item) = session.plan.get_mut(&active.id) {
                    item.status = ItemStatus::Understood;
                    item.notes = Some(check.feedback.clone());
                }
                for cid in &active.introduces {
                    session.upsert_concept(
                        cid,
                        None,
                        ConceptStatus::Proven,
                        Some(check.feedback.clone()),
                    );
                }
                for cid in &check.proven_concepts {
                    session.upsert_concept(
                        cid,
                        None,
                        ConceptStatus::Proven,
                        Some(check.feedback.clone()),
                    );
                }
                self.store.record_outcome(
                    &session.id,
                    &active.id,
                    &active.title,
                    ItemStatus::Understood,
                    Some(&check.feedback),
                )?;
                plan_changed = true;

                let next_hint = session
                    .plan
                    .next_ready_id()
                    .and_then(|id| session.plan.get(&id).map(|i| i.title.clone()));

                match next_hint {
                    Some(next_title) => format!(
                        "{}\n\nMarked **{}** understood. Run `/handoff` if you want me to finish remaining labor on it, or `/next` to move on to **{}**.",
                        check.feedback, active.title, next_title
                    ),
                    None => format!(
                        "{}\n\nMarked **{}** understood. Run `/handoff` for any remaining labor, or `/next` to wrap up.",
                        check.feedback, active.title
                    ),
                }
            }
            CheckVerdict::Close | CheckVerdict::NotYet => {
                format!(
                    "{}\n\nAdjust and run `/check` again, or `/skip` to hand this part off.",
                    check.feedback
                )
            }
        };

        session.push_turn("assistant", &reply);
        Ok(TurnOutput {
            reply,
            plan_changed,
        })
    }

    fn handle_next(&self, session: &mut Session) -> Result<TurnOutput> {
        if let Some(active) = session.active() {
            if !active.status.is_resolved() {
                return Ok(TurnOutput {
                    reply: format!(
                        "**{}** isn't resolved yet. Run `/check` when you've got it, or `/skip` to hand it off.",
                        active.title
                    ),
                    plan_changed: false,
                });
            }
        }

        let next = session.advance_active();
        let reply = match next.as_deref().and_then(|id| session.plan.get(id)) {
            Some(item) => render_active_prompt(item),
            None => "All items on the path are done.".to_string(),
        };

        session.push_turn("assistant", &reply);
        Ok(TurnOutput {
            reply,
            plan_changed: true,
        })
    }

    async fn handle_handoff(
        &self,
        session: &mut Session,
        instructions: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let Some(active) = session.active().cloned() else {
            return Ok(TurnOutput {
                reply: "No active item to hand off.".to_string(),
                plan_changed: false,
            });
        };

        if active.status != ItemStatus::Understood {
            return Ok(TurnOutput {
                reply: format!(
                    "Handoff unlocks after passing `/check` on **{}**. To skip learning this part and let me handle it now, run `/skip`.",
                    active.title
                ),
                plan_changed: false,
            });
        }

        let report = self
            .run_labor(session, &active, instructions, files, sink)
            .await?;

        if let Some(item) = session.plan.get_mut(&active.id) {
            item.status = ItemStatus::HandedOff;
            item.notes = Some(report.clone());
        }
        self.store.record_outcome(
            &session.id,
            &active.id,
            &active.title,
            ItemStatus::HandedOff,
            Some(&report),
        )?;

        let next = session.advance_active();
        let tail = match next.as_deref().and_then(|id| session.plan.get(id)) {
            Some(item) => format!("\n\n{}", render_active_prompt(item)),
            None => "\n\nEverything on this path is resolved.".to_string(),
        };

        session.push_turn("assistant", format!("{report}{tail}"));
        Ok(TurnOutput {
            reply: tail,
            plan_changed: true,
        })
    }

    async fn handle_skip(
        &self,
        session: &mut Session,
        instructions: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let Some(active) = session.active().cloned() else {
            return Ok(TurnOutput {
                reply: "No active item to skip.".to_string(),
                plan_changed: false,
            });
        };

        let streamed = !instructions.eq_ignore_ascii_case("none");
        let report = if streamed {
            self.run_labor(session, &active, instructions, files, sink)
                .await?
        } else {
            format!("Skipped **{}** (no agent work requested).", active.title)
        };

        if let Some(item) = session.plan.get_mut(&active.id) {
            item.status = ItemStatus::Skipped;
            item.notes = Some(report.clone());
        }
        for cid in &active.introduces {
            session.upsert_concept(cid, None, ConceptStatus::Skipped, None);
        }
        self.store.record_outcome(
            &session.id,
            &active.id,
            &active.title,
            ItemStatus::Skipped,
            Some(&report),
        )?;

        let next = session.advance_active();
        let tail = match next.as_deref().and_then(|id| session.plan.get(id)) {
            Some(item) => format!("\n\n{}", render_active_prompt(item)),
            None => "\n\nEverything on this path is resolved.".to_string(),
        };

        let reply = if streamed {
            format!("\n\nMarked **{}** skipped (not yet understood).{tail}", active.title)
        } else {
            format!("{report}{tail}")
        };
        session.push_turn("assistant", format!("{report}{tail}"));
        Ok(TurnOutput {
            reply,
            plan_changed: true,
        })
    }

    async fn run_labor(
        &self,
        session: &Session,
        item: &PathItem,
        instructions: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<String> {
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::Full,
            cwd: session.cwd.clone(),
            system_prompt: LABOR_SYSTEM.to_string(),
            prompt: format!(
                "Goal: {}\nItem to execute: {} (`{}`)\nTask scope: {}\nExtra user instructions: {}\n\nComplete the needed labor cleanly in the workspace (or provide a concise summary if purely conceptual). Keep your final report brief and concrete.",
                session.goal.as_deref().unwrap_or("(session goal)"),
                item.title,
                item.id,
                item.user_task,
                if instructions.is_empty() {
                    "(none)"
                } else {
                    instructions
                }
            ),
            files: files.to_vec(),
            stream_text: true,
        };

        self.engine.run_text(&req, sink).await
    }

    async fn handle_replan(
        &self,
        session: &mut Session,
        reason: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let resolved: Vec<PathItem> = session
            .plan
            .items
            .iter()
            .filter(|i| i.status.is_resolved())
            .cloned()
            .collect();
        let current_map = session.plan.render_map(session.active_item.as_deref());
        let concepts_summary = session.render_concepts();
        let history = format_history(session);

        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: REPLAN_SYSTEM.to_string(),
            prompt: format!(
                "Goal: {}\nWhat changed: {}\n\nTracked concept state:\n{concepts_summary}\n\nCurrent path:\n{current_map}\n\nRecent conversation:\n{history}\n\nReturn ONLY JSON for the updated remaining items (do not reuse already-resolved item IDs; new items may depend on resolved IDs).",
                session.goal.as_deref().unwrap_or("(session goal)"),
                if reason.is_empty() {
                    "(infer from conversation)"
                } else {
                    reason
                }
            ),
            files: files.to_vec(),
            stream_text: false,
        };

        let resolved_snapshot = resolved.clone();
        let concepts_snapshot = session.concepts.clone();
        let proposal: PlanProposal = run_gated(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "replan",
            &req,
            sink,
            |p: &PlanProposal| check_plan_proposal(p, &resolved_snapshot, &concepts_snapshot),
        )
        .await?;

        let mut merged = resolved;
        for prop in proposal.items {
            for cid in &prop.introduces {
                if !session.concepts.contains_key(cid) {
                    session.upsert_concept(cid, None, ConceptStatus::Unknown, None);
                }
            }
            merged.push(PathItem {
                id: prop.id,
                title: prop.title,
                depends_on: prop.depends_on,
                requires: prop.requires,
                introduces: prop.introduces,
                framing: prop.framing,
                user_task: prop.user_task,
                check_criteria: prop.check_criteria,
                status: ItemStatus::Pending,
                notes: None,
            });
        }
        session.plan = PlanGraph { items: merged };
        let active_id = session.advance_active();

        let reply = match active_id.as_deref().and_then(|id| session.plan.get(id)) {
            Some(item) => format!("{}\n\n{}", proposal.brief, render_active_prompt(item)),
            None => proposal.brief,
        };

        session.push_turn("assistant", &reply);
        Ok(TurnOutput {
            reply,
            plan_changed: true,
        })
    }

    fn handle_focus(&self, session: &mut Session, id: &str) -> Result<TurnOutput> {
        if id.is_empty() {
            return Ok(TurnOutput {
                reply: "Usage: `/focus <id>`. Run `/map` to see item IDs.".to_string(),
                plan_changed: false,
            });
        }
        let Some(item) = session.plan.get(id).cloned() else {
            return Ok(TurnOutput {
                reply: format!("No item `{id}` on the current path."),
                plan_changed: false,
            });
        };
        if !session.plan.deps_satisfied(&item) {
            return Ok(TurnOutput {
                reply: format!("`{id}` depends on {} first.", item.depends_on.join(", ")),
                plan_changed: false,
            });
        }
        session.active_item = Some(item.id.clone());
        session.turns_on_active = 0;
        if let Some(m) = session.plan.get_mut(&item.id) {
            if m.status == ItemStatus::Pending {
                m.status = ItemStatus::Working;
            }
        }
        session.mode = SessionMode::Working;
        Ok(TurnOutput {
            reply: render_active_prompt(&item),
            plan_changed: true,
        })
    }
}

fn render_active_prompt(item: &PathItem) -> String {
    let mut blocks = vec![format!("**{}** (`{}`)", item.title, item.id)];
    if !item.framing.trim().is_empty() {
        blocks.push(item.framing.trim().to_string());
    }
    blocks.push(format!("Try this: {}", item.user_task.trim()));
    blocks.push("Run `/check` when you have it, or `/skip` to hand it off.".to_string());
    blocks.join("\n\n")
}

fn summarize_folder(cwd: &Path) -> String {
    let Ok(entries) = fs::read_dir(cwd) else {
        return "(unreadable folder)".to_string();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let suffix = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                "/"
            } else {
                ""
            };
            Some(format!("{name}{suffix}"))
        })
        .collect();
    names.sort();
    names.truncate(40);
    if names.is_empty() {
        "(empty folder)".to_string()
    } else {
        names.join(", ")
    }
}

fn format_history(session: &Session) -> String {
    if session.history.is_empty() {
        return "(none)".to_string();
    }
    session
        .history
        .iter()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|t| format!("{}: {}", t.role, t.content))
        .collect::<Vec<_>>()
        .join("\n")
}

const ONBOARDING_SYSTEM: &str = r#"You are a calm, direct human-in-the-loop pathfinder doing short onboarding.
Your job: establish TWO things before planning:
1. The concrete goal (and preferred medium/tools if relevant).
2. What the user already knows vs. does not know on the key prerequisite concepts or skills.

Strict rules (enforced mechanically by critics):
- Flat, calm tone. NEVER open with cheerleading or filler ("Great goal!", "Awesome!", "Sure!", "Let's dive in!").
- Keep `reply` to 1-2 short sentences (under 60 words). Ask ONE focused question at a time.
- Wrap math formulas in inline code backticks (e.g. `z = w1*x1 + w2*x2 + b`), never bare subscripts with underscores.
- Do NOT set `ready_to_plan` to true until you have actually probed the user's prior background AND populated `goal_summary`, `starting_point`, and at least one entry in `observed_concepts`.
- In `observed_concepts`, status must be `"unknown"` or `"claimed"` (never `"proven"` before a check).
- Never solve the task during onboarding.

Return ONLY a JSON object:
{
  "reply": "1-2 calm sentences with a single focused question",
  "ready_to_plan": false,
  "goal_summary": "concise goal or null",
  "starting_point": "concise summary of user background + folder state or null",
  "observed_concepts": [
    {"id": "lowercase_id", "label": "Readable name", "status": "unknown"}
  ]
}"#;

const PLANNER_SYSTEM: &str = r#"You are the deep planning engine of a neurosymbolic human-in-the-loop pathfinder.
Build a sound dependency graph (DAG) of 3-6 concrete items so the human builds real understanding by doing each part themselves before handing off repetitive labor.

Strict rules (enforced mechanically by critics):
- Calm, flat, plain language. No filler openers, no cheerleading, no exclamation spam.
- Wrap math expressions in inline code backticks (e.g. `z = w1*x1 + w2*x2 + b`), never bare underscore subscripts outside backticks.
- `brief`: 1-3 plain sentences summarizing the path (under 75 words).
- Every item has:
  - `id`: short lowercase id ("p1", "p2", ...)
  - `title`: crisp plain title
  - `depends_on`: valid earlier item IDs (no cycles)
  - `requires`: lowercase concept IDs that MUST already be `proven`/`skipped` in tracked concept state OR be listed in `introduces` of an item in `depends_on` (transitively). Do NOT put unintroduced concepts in `requires`.
  - `introduces`: lowercase concept IDs newly introduced by this item.
  - `framing`: MANDATORY whenever `introduces` is non-empty (1-3 crisp sentences, 12-80 words). Plainly explain the newly introduced idea BEFORE asking the user to act, so the user is never hit cold with unexplained terms or formulas.
  - `user_task`: 1-2 concrete sentences (under 70 words) telling the user what to try, compute, inspect, or write.
  - `check_criteria`: what proves real understanding.

Return ONLY a JSON object:
{
  "brief": "short plain summary",
  "items": [
    {
      "id": "p1",
      "title": "Short title",
      "depends_on": [],
      "requires": [],
      "introduces": ["weighted_sum", "bias"],
      "framing": "1-3 plain sentences introducing the concept clearly before the task.",
      "user_task": "Concrete action for the human.",
      "check_criteria": "What proves they got it."
    }
  ]
}"#;

const COACH_SYSTEM: &str = r#"You are a calm, direct partner guiding the human while they work on the current item.
Strict rules (enforced mechanically by critics):
- Flat, plain tone. Never use cheerleading or filler openers ("Great!", "Awesome!", "Sure!").
- Keep `reply` to 1-3 short sentences (under 80 words). Never dump walls of text or large code blocks (>6 lines).
- Wrap math formulas in inline code backticks, never bare underscore subscripts.
- Stay strictly on the current item and do not spoil the answer.
- Choose `action`:
  - "keep_going": normal short nudge or clarifying question
  - "suggest_check": user appears ready to verify with /check
  - "escalate": user is deeply stuck or has a subtle misconception needing deeper diagnosis
  - "need_replan": new evidence shows the path needs to change

Return ONLY a JSON object:
{
  "reply": "1-3 calm sentences",
  "action": "keep_going"
}"#;

const CHECK_SYSTEM: &str = r#"You are the careful judge of a human-in-the-loop pathfinder.
Evaluate whether the user genuinely understands the current item based on their explanation, work, and workspace files.
Strict rules (enforced mechanically by critics):
- Calm, direct tone. No filler openers or cheerleading.
- Do not rubber-stamp vague or hand-wavy answers.
- Keep `feedback` to 1-3 sentences (under 90 words), specific about what landed or what is missing.
- Wrap math formulas in inline code backticks.
- `verdict` must be `"pass"`, `"close"`, or `"not_yet"`.
- `proven_concepts`: list of lowercase concept IDs genuinely demonstrated in this check (empty if not passed).

Return ONLY a JSON object:
{
  "verdict": "pass",
  "feedback": "short specific feedback",
  "proven_concepts": ["weighted_sum"]
}"#;

const LABOR_SYSTEM: &str = r#"You are executing handed-off or skipped labor inside a human-in-the-loop pathfinder.
Complete the requested work cleanly using available tools.
Calm, direct tone. Keep your final summary brief (3-6 sentences or short bullets) reporting what you changed or found."#;

const REPLAN_SYSTEM: &str = r#"You are updating the remaining path of a neurosymbolic human-in-the-loop pathfinder because reality changed mid-session.
Strict rules (enforced mechanically by critics):
- Calm, flat tone. Wrap math formulas in inline code backticks.
- `brief`: 1-2 plain sentences explaining what changed.
- Return ONLY the new/updated remaining items; do not reuse already-resolved item IDs.
- Every `requires` concept must already be proven/skipped or introduced by a dependency ancestor.
- Every item introducing new concepts must include a clear 1-3 sentence `framing` before `user_task`.

Return ONLY a JSON object:
{
  "brief": "what changed",
  "items": [
    {
      "id": "p_new_1",
      "title": "Short title",
      "depends_on": [],
      "requires": [],
      "introduces": ["concept_id"],
      "framing": "1-3 plain sentences explaining the new idea before the task.",
      "user_task": "Concrete action for the human.",
      "check_criteria": "What proves they got it."
    }
  ]
}"#;

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::engine::{BoxFuture, noop_sink};

    struct FakeEngine {
        replies: Mutex<VecDeque<(ToolAccess, EngineTier, String)>>,
    }

    impl FakeEngine {
        fn new(items: Vec<(ToolAccess, EngineTier, &str)>) -> Self {
            Self {
                replies: Mutex::new(
                    items
                        .into_iter()
                        .map(|(ta, tier, s)| (ta, tier, s.to_string()))
                        .collect(),
                ),
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
                let (expected_tools, expected_tier, reply) = self
                    .replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected engine call");
                assert_eq!(req.tools, expected_tools, "tool access mismatch");
                assert_eq!(req.tier, expected_tier, "engine tier mismatch");
                Ok(reply)
            })
        }
    }

    #[tokio::test]
    async fn critic_rejects_fluff_on_s1_and_escalates_to_s2() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(FakeEngine::new(vec![
            // System 1 violates tone ("Great goal!") -> critic rejects & escalates to System 2
            (
                ToolAccess::ReadOnly,
                EngineTier::System1,
                r#"{"reply":"Great goal! Do you know algebra?","ready_to_plan":false,"observed_concepts":[]}"#,
            ),
            // System 2 fixes the violation -> accepted
            (
                ToolAccess::ReadOnly,
                EngineTier::System2,
                r#"{"reply":"Before we map this out, how comfortable are you with linear combinations and basic derivatives?","ready_to_plan":false,"goal_summary":"Learn neuron mechanics","observed_concepts":[]}"#,
            ),
        ]));

        let rt = Runtime::new(store, engine);
        rt.create_session("s1".into(), PathBuf::from(".")).unwrap();

        let out = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "I want to learn neuron mechanics".into(),
                    files: vec![],
                },
                noop_sink(),
            )
            .await
            .unwrap();
        assert!(!out.plan_changed);
        assert!(out.reply.contains("linear combinations"));
    }

    #[tokio::test]
    async fn enforces_framing_concepts_and_tool_gating_end_to_end() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(FakeEngine::new(vec![
            (
                ToolAccess::ReadOnly,
                EngineTier::System1,
                r#"{"reply":"Building a short path from scratch.","ready_to_plan":true,"goal_summary":"Fix parser","starting_point":"Knows Rust, new to lexers","observed_concepts":[{"id":"rust_basics","label":"Rust basics","status":"claimed"},{"id":"lexer_state","label":"Lexer state","status":"unknown"}]}"#,
            ),
            (
                ToolAccess::ReadOnly,
                EngineTier::System2,
                r#"{"brief":"Two-part path covering the tokenizer and the remaining cases.","items":[{"id":"p1","title":"Trace tokenizer","depends_on":[],"requires":[],"introduces":["lexer_state"],"framing":"A lexer scans input one character at a time and skips whitespace before reading a token.","user_task":"Check line 10 and trace what happens on a tab character.","check_criteria":"Explain why tabs slip past whitespace handling."},{"id":"p2","title":"Fix remaining rules","depends_on":["p1"],"requires":["lexer_state"],"introduces":[],"framing":"","user_task":"Apply the same whitespace check to the remaining branches.","check_criteria":"Tests pass."}]}"#,
            ),
            (
                ToolAccess::ReadOnly,
                EngineTier::System1,
                r#"{"reply":"Look closely at which characters the whitespace helper matches.","action":"keep_going"}"#,
            ),
            (
                ToolAccess::ReadOnly,
                EngineTier::System2,
                r#"{"verdict":"pass","feedback":"Exact diagnosis: tabs were missing from the whitespace branch.","proven_concepts":["lexer_state"]}"#,
            ),
            (
                ToolAccess::Full,
                EngineTier::System2,
                "Updated the remaining tokenizer cases and all tests pass.",
            ),
        ]));

        let rt = Runtime::new(store.clone(), engine);
        rt.create_session("s1".into(), PathBuf::from(".")).unwrap();

        let out = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "Fix parser bug; I know Rust but never wrote a lexer".into(),
                    files: vec![],
                },
                noop_sink(),
            )
            .await
            .unwrap();
        assert!(out.plan_changed);
        assert!(out.reply.contains("A lexer scans input one character at a time"));
        assert!(out.reply.contains("Try this:"));

        // Coaching stays read-only on S1
        let out = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "Why does it fail on tabs?".into(),
                    files: vec![],
                },
                noop_sink(),
            )
            .await
            .unwrap();
        assert!(!out.plan_changed);
        assert!(out.reply.contains("whitespace helper"));

        // Handoff blocked before /check passes
        let blocked = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "/handoff".into(),
                    files: vec![],
                },
                noop_sink(),
            )
            .await
            .unwrap();
        assert!(blocked.reply.contains("unlocks after passing `/check`"));

        // /check passes on S2 (read-only) and marks `lexer_state` proven
        let checked = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "/check Tabs were not in the whitespace match".into(),
                    files: vec![],
                },
                noop_sink(),
            )
            .await
            .unwrap();
        assert!(checked.plan_changed);
        let session = rt.load_session("s1").unwrap().unwrap();
        assert_eq!(
            session.concepts.get("lexer_state").unwrap().status,
            ConceptStatus::Proven
        );

        // /handoff unlocks Full tools on S2 and advances to p2
        let handoff = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "/handoff".into(),
                    files: vec![],
                },
                noop_sink(),
            )
            .await
            .unwrap();
        assert!(handoff.plan_changed);
        assert!(handoff.reply.contains("Fix remaining rules"));
    }
}
