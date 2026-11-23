use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::domain::{
    CheckDecision, CheckVerdict, CoachAction, CoachDecision, ItemStatus, OnboardingDecision,
    PathItem, PlanGraph, PlanProposal, Session, SessionMode,
};
use crate::engine::{Engine, EngineRequest, EngineTier, ToolAccess, run_structured};
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

    pub async fn handle_turn(&self, session_id: &str, input: TurnInput) -> Result<TurnOutput> {
        let mut session = self
            .store
            .load_session(session_id)?
            .ok_or_else(|| anyhow!("unknown session `{session_id}`"))?;

        let trimmed = input.text.trim();
        let output = if trimmed.starts_with('/') {
            self.handle_command(&mut session, trimmed, &input.files).await?
        } else {
            match session.mode {
                SessionMode::Onboarding => {
                    self.handle_onboarding(&mut session, trimmed, &input.files).await?
                }
                SessionMode::Working => {
                    self.handle_working(&mut session, trimmed, &input.files).await?
                }
                SessionMode::Done => TurnOutput {
                    reply: "Everything on this path is resolved. Run `/replan <new goal>` to add more or start a new session."
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
    ) -> Result<TurnOutput> {
        let mut parts = raw.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();

        match cmd {
            "/map" => Ok(TurnOutput {
                reply: session.plan.render_map(session.active_item.as_deref()),
                plan_changed: false,
            }),
            "/plan" => {
                if !arg.is_empty() {
                    session.push_turn("user", arg);
                }
                self.build_initial_plan(session, files).await
            }
            "/check" => self.handle_check(session, arg, files).await,
            "/next" => self.handle_next(session),
            "/handoff" => self.handle_handoff(session, arg, files).await,
            "/skip" => self.handle_skip(session, arg, files).await,
            "/replan" => self.handle_replan(session, arg, files).await,
            "/focus" => self.handle_focus(session, arg),
            _ => Ok(TurnOutput {
                reply: "Commands: `/plan`, `/check [answer]`, `/next`, `/handoff [notes]`, `/skip [notes]`, `/replan [what changed]`, `/map`, `/focus <id>`."
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
    ) -> Result<TurnOutput> {
        session.push_turn("user", text);

        let folder_summary = summarize_folder(&session.cwd);
        let history = format_history(&session);

        let req = EngineRequest {
            tier: EngineTier::System1,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: ONBOARDING_SYSTEM.to_string(),
            prompt: format!(
                "Workspace folder snapshot:\n{folder_summary}\n\nConversation so far:\n{history}\n\nRespond with JSON only."
            ),
            files: files.to_vec(),
        };

        let decision: OnboardingDecision =
            run_structured(self.engine.as_ref(), &req, |d: &OnboardingDecision| {
            if d.reply.trim().is_empty() {
                anyhow::bail!("reply must not be empty");
            }
            Ok(())
        })
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

        if decision.ready_to_plan {
            session.push_turn("assistant", &decision.reply);
            return self.build_initial_plan(session, files).await;
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
    ) -> Result<TurnOutput> {
        let folder_summary = summarize_folder(&session.cwd);
        let history = format_history(session);
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
                "Goal: {goal}\nStarting point: {starting}\n\nWorkspace folder snapshot:\n{folder_summary}\n\nOnboarding conversation:\n{history}\n\nBuild a clean, practical dependency graph as JSON only."
            ),
            files: files.to_vec(),
        };

        let proposal: PlanProposal =
            run_structured(self.engine.as_ref(), &req, |p: &PlanProposal| {
                let graph = PlanGraph {
                items: p
                    .items
                    .iter()
                    .map(|i| PathItem {
                        id: i.id.clone(),
                        title: i.title.clone(),
                        depends_on: i.depends_on.clone(),
                        user_task: i.user_task.clone(),
                        check_criteria: i.check_criteria.clone(),
                        status: ItemStatus::Pending,
                        notes: None,
                    })
                    .collect(),
            };
            graph.validate().map_err(|e| anyhow!(e))
        })
        .await?;

        let (brief, graph) = proposal.into_graph().map_err(|e| anyhow!(e))?;
        session.plan = graph;
        let active_id = session.advance_active();

        let reply = match active_id.as_deref().and_then(|id| session.plan.get(id)) {
            Some(first) => format!(
                "{brief}\n\nStarting on **{}** (`{}`):\n{}\n\nWhen you have it, run `/check` (or `/skip` to let me handle it).",
                first.title, first.id, first.user_task
            ),
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
    ) -> Result<TurnOutput> {
        let Some(active) = session.active().cloned() else {
            let next = session.advance_active();
            return Ok(TurnOutput {
                reply: match next.and_then(|id| session.plan.get(&id).cloned()) {
                    Some(item) => format!(
                        "Now on **{}** (`{}`):\n{}",
                        item.title, item.id, item.user_task
                    ),
                    None => "Everything on this path is resolved.".to_string(),
                },
                plan_changed: true,
            });
        };

        if active.status == ItemStatus::Understood {
            return Ok(TurnOutput {
                reply: format!(
                    "You already passed **{}**. Run `/handoff` to let me finish any remaining labor on it, or `/next` to move on.",
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
            .run_coach_turn(session, &active, tier, files)
            .await?;

        if !force_s2 && decision.action == CoachAction::Escalate {
            let escalated = self
                .run_coach_turn(session, &active, EngineTier::System2, files)
                .await?;
            session.turns_on_active = 0;
            session.push_turn("assistant", &escalated.reply);
            return Ok(TurnOutput {
                reply: escalated.reply,
                plan_changed: false,
            });
        }

        if decision.action == CoachAction::NeedReplan {
            return self.handle_replan(session, text, files).await;
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
    ) -> Result<CoachDecision> {
        let history = format_history(session);
        let req = EngineRequest {
            tier,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: COACH_SYSTEM.to_string(),
            prompt: format!(
                "Goal: {}\nCurrent focus: {} (`{}`)\nWhat the user should do: {}\nWhat proves understanding: {}\n\nRecent conversation:\n{history}\n\nRespond with JSON only.",
                session.goal.as_deref().unwrap_or("(session goal)"),
                active.title,
                active.id,
                active.user_task,
                active.check_criteria
            ),
            files: files.to_vec(),
        };

        run_structured(self.engine.as_ref(), &req, |d: &CoachDecision| {
            if d.reply.trim().is_empty() {
                anyhow::bail!("reply must not be empty");
            }
            Ok(())
        })
        .await
    }

    async fn handle_check(
        &self,
        session: &mut Session,
        answer: &str,
        files: &[PathBuf],
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
                "Checking item: {} (`{}`)\nUser task: {}\nCriteria for understanding: {}\nUser submission: {}\n\nRecent conversation:\n{history}\n\nInspect relevant files in the workspace if needed, then return JSON only.",
                active.title,
                active.id,
                active.user_task,
                active.check_criteria,
                if answer.is_empty() {
                    "(check recent conversation and workspace files)"
                } else {
                    answer
                }
            ),
            files: files.to_vec(),
        };

        let check: CheckDecision =
            run_structured(self.engine.as_ref(), &req, |c: &CheckDecision| {
            if c.feedback.trim().is_empty() {
                anyhow::bail!("feedback must not be empty");
            }
            Ok(())
        })
        .await?;

        let mut plan_changed = false;
        let reply = match check.verdict {
            CheckVerdict::Pass => {
                if let Some(item) = session.plan.get_mut(&active.id) {
                    item.status = ItemStatus::Understood;
                    item.notes = Some(check.feedback.clone());
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
                        "{}\n\nMarked **{}** understood. Run `/handoff` if you want me to take over remaining labor on it, or `/next` to move on to **{}**.",
                        check.feedback, active.title, next_title
                    ),
                    None => format!(
                        "{}\n\nMarked **{}** understood. Run `/handoff` for any remaining labor, or `/next` to wrap up.",
                        check.feedback, active.title
                    ),
                }
            }
            CheckVerdict::Close | CheckVerdict::NotYet => {
                format!("{}\n\nGive it another shot, then run `/check` again (or `/skip` to hand it off).", check.feedback)
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
            Some(item) => format!(
                "Next up: **{}** (`{}`)\n{}\n\nRun `/check` when ready, or `/skip` to hand it off.",
                item.title, item.id, item.user_task
            ),
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
                    "Handoff unlocks after passing `/check` on **{}**. If you want to skip learning this part and let me do it now, run `/skip`.",
                    active.title
                ),
                plan_changed: false,
            });
        }

        let report = self
            .run_labor(session, &active, instructions, files)
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
            Some(item) => format!("\n\nNext up: **{}** (`{}`)\n{}", item.title, item.id, item.user_task),
            None => "\n\nEverything on this path is resolved.".to_string(),
        };

        let reply = format!("{report}{tail}");
        session.push_turn("assistant", &reply);
        Ok(TurnOutput {
            reply,
            plan_changed: true,
        })
    }

    async fn handle_skip(
        &self,
        session: &mut Session,
        instructions: &str,
        files: &[PathBuf],
    ) -> Result<TurnOutput> {
        let Some(active) = session.active().cloned() else {
            return Ok(TurnOutput {
                reply: "No active item to skip.".to_string(),
                plan_changed: false,
            });
        };

        let report = if instructions.eq_ignore_ascii_case("none") {
            format!("Skipped **{}** (no agent work requested).", active.title)
        } else {
            self.run_labor(session, &active, instructions, files).await?
        };

        if let Some(item) = session.plan.get_mut(&active.id) {
            item.status = ItemStatus::Skipped;
            item.notes = Some(report.clone());
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
            Some(item) => format!("\n\nNext up: **{}** (`{}`)\n{}", item.title, item.id, item.user_task),
            None => "\n\nEverything on this path is resolved.".to_string(),
        };

        let reply = format!("Skipped **{}** (recorded as not yet understood).\n\n{report}{tail}", active.title);
        session.push_turn("assistant", &reply);
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
        };

        self.engine.run_text(&req).await
    }

    async fn handle_replan(
        &self,
        session: &mut Session,
        reason: &str,
        files: &[PathBuf],
    ) -> Result<TurnOutput> {
        let resolved: Vec<PathItem> = session
            .plan
            .items
            .iter()
            .filter(|i| i.status.is_resolved())
            .cloned()
            .collect();
        let current_map = session.plan.render_map(session.active_item.as_deref());
        let history = format_history(session);

        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: REPLAN_SYSTEM.to_string(),
            prompt: format!(
                "Goal: {}\nWhat changed: {}\n\nCurrent path:\n{current_map}\n\nRecent conversation:\n{history}\n\nReturn ONLY JSON for the updated remaining items (do not repeat already-resolved item IDs; new items may depend on resolved IDs).",
                session.goal.as_deref().unwrap_or("(session goal)"),
                if reason.is_empty() {
                    "(infer from conversation)"
                } else {
                    reason
                }
            ),
            files: files.to_vec(),
        };

        let resolved_clone = resolved.clone();
        let proposal: PlanProposal =
            run_structured(self.engine.as_ref(), &req, |p: &PlanProposal| {
                let mut items = resolved_clone.clone();
            for prop in &p.items {
                items.push(PathItem {
                    id: prop.id.clone(),
                    title: prop.title.clone(),
                    depends_on: prop.depends_on.clone(),
                    user_task: prop.user_task.clone(),
                    check_criteria: prop.check_criteria.clone(),
                    status: ItemStatus::Pending,
                    notes: None,
                });
            }
            let graph = PlanGraph { items };
            graph.validate().map_err(|e| anyhow!(e))
        })
        .await?;

        let mut merged = resolved;
        for prop in proposal.items {
            merged.push(PathItem {
                id: prop.id,
                title: prop.title,
                depends_on: prop.depends_on,
                user_task: prop.user_task,
                check_criteria: prop.check_criteria,
                status: ItemStatus::Pending,
                notes: None,
            });
        }
        session.plan = PlanGraph { items: merged };
        let active_id = session.advance_active();

        let reply = match active_id.as_deref().and_then(|id| session.plan.get(id)) {
            Some(item) => format!(
                "{}\n\nNow focusing on **{}** (`{}`):\n{}",
                proposal.brief, item.title, item.id, item.user_task
            ),
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
                reply: format!(
                    "`{id}` depends on {} first.",
                    item.depends_on.join(", ")
                ),
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
            reply: format!(
                "Switched focus to **{}** (`{}`):\n{}",
                item.title, item.id, item.user_task
            ),
            plan_changed: true,
        })
    }
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

const ONBOARDING_SYSTEM: &str = r#"You are a calm, plain-language human-in-the-loop pathfinder doing short onboarding.
Your job: understand the user's goal and starting point (both what is in the folder and what they already know).
Rules:
- Keep `reply` to 1-3 short sentences. Never write walls of text.
- Ask at most ONE focused question at a time.
- As soon as you know the goal and roughly where they are starting (usually 1-2 exchanges), set `ready_to_plan` to true.
- Never solve the task yet.
Return ONLY a JSON object with this exact shape:
{
  "reply": "short response or single question",
  "ready_to_plan": false,
  "goal_summary": "optional concise goal",
  "starting_point": "optional concise starting point"
}"#;

const PLANNER_SYSTEM: &str = r#"You are the deep planning engine of a human-in-the-loop pathfinder.
Build a practical dependency graph (DAG) of 3-7 concrete items so the human builds real understanding by doing each part themselves before handing off repetitive labor.
Rules:
- Keep `brief` to 2-3 plain sentences explaining the path.
- Each item needs a short lowercase id (e.g. "p1", "p2"), a clear `title`, `depends_on` (valid earlier IDs only, no cycles), a concrete `user_task` (1-2 sentences telling the user what to try or examine), and `check_criteria` (what proves real understanding).
- Works for any domain: code, physics/study, hardware troubleshooting, data analysis, reading slides.
Return ONLY a JSON object with this exact shape:
{
  "brief": "short plain summary",
  "items": [
    {
      "id": "p1",
      "title": "Short title",
      "depends_on": [],
      "user_task": "Concrete action for the human",
      "check_criteria": "What proves they got it"
    }
  ]
}"#;

const COACH_SYSTEM: &str = r#"You are a calm, plain-language partner guiding the human while they do the current part themselves.
Rules:
- Keep `reply` to 1-3 short sentences. No lectures, no 100-line dumps, no spoiling the answer.
- Stay strictly on the current focus item; do not drift.
- Give one small nudge, observation, or clarifying question.
- Choose `action`:
  - "keep_going": normal coaching nudge
  - "suggest_check": the user seems to have it and should run /check
  - "escalate": the user is deeply stuck or there is a subtle misconception needing deeper diagnosis
  - "need_replan": new evidence shows the current path is wrong (e.g., different root cause or changed goal)
Return ONLY a JSON object with this exact shape:
{
  "reply": "1-3 short sentences",
  "action": "keep_going"
}"#;

const CHECK_SYSTEM: &str = r#"You are the careful judge of a human-in-the-loop pathfinder.
Evaluate whether the user genuinely understands the current item based on their explanation, work, and workspace files.
Rules:
- Do not rubber-stamp vague or hand-wavy answers.
- Keep `feedback` brief (1-3 sentences), plain language, and specific about what landed or what is still missing.
- `verdict` must be one of: "pass", "close", "not_yet".
Return ONLY a JSON object with this exact shape:
{
  "verdict": "pass",
  "feedback": "short specific feedback"
}"#;

const LABOR_SYSTEM: &str = r#"You are executing handed-off or skipped labor inside a human-in-the-loop pathfinder.
Do the requested work cleanly and accurately using available tools.
Keep your final report brief (3-6 sentences or short bullet list) summarizing what you changed or found."#;

const REPLAN_SYSTEM: &str = r#"You are updating the remaining path of a human-in-the-loop pathfinder because reality changed mid-session.
Rules:
- Keep `brief` to 1-2 plain sentences explaining how the path changed.
- Return ONLY the new/updated remaining items; do not reuse already-resolved item IDs.
- New items may depend on already-resolved IDs or earlier new items, with no cycles.
Return ONLY a JSON object with this exact shape:
{
  "brief": "what changed",
  "items": [
    {
      "id": "p_new_1",
      "title": "Short title",
      "depends_on": [],
      "user_task": "Concrete action for the human",
      "check_criteria": "What proves they got it"
    }
  ]
}"#;

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::engine::BoxFuture;

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
        fn run_text<'a>(&'a self, req: &'a EngineRequest) -> BoxFuture<'a, Result<String>> {
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
    async fn enforces_tool_gating_across_full_loop() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(FakeEngine::new(vec![
            (
                ToolAccess::ReadOnly,
                EngineTier::System1,
                r#"{"reply":"Got it, building a path.","ready_to_plan":true,"goal_summary":"Fix parser","starting_point":"Has failing test"}"#,
            ),
            (
                ToolAccess::ReadOnly,
                EngineTier::System2,
                r#"{"brief":"Two-part path.","items":[{"id":"p1","title":"Trace tokenizer","depends_on":[],"user_task":"Check line 10","check_criteria":"Explain token bug"},{"id":"p2","title":"Fix remaining rules","depends_on":["p1"],"user_task":"Apply pattern","check_criteria":"Tests pass"}]}"#,
            ),
            (
                ToolAccess::ReadOnly,
                EngineTier::System1,
                r#"{"reply":"Look at how whitespace is skipped.","action":"keep_going"}"#,
            ),
            (
                ToolAccess::ReadOnly,
                EngineTier::System2,
                r#"{"verdict":"pass","feedback":"Exactly right."}"#,
            ),
            (
                ToolAccess::Full,
                EngineTier::System2,
                "Updated remaining tokenizer cases and tests pass.",
            ),
        ]));

        let rt = Runtime::new(store.clone(), engine);
        rt.create_session("s1".into(), PathBuf::from(".")).unwrap();

        // Onboarding -> auto S2 plan
        let out = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "Fix parser bug".into(),
                    files: vec![],
                },
            )
            .await
            .unwrap();
        assert!(out.plan_changed);
        assert!(out.reply.contains("Trace tokenizer"));

        // Coaching stays read-only on S1
        let out = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "Why does it fail on tabs?".into(),
                    files: vec![],
                },
            )
            .await
            .unwrap();
        assert!(!out.plan_changed);
        assert!(out.reply.contains("whitespace"));

        // Handoff blocked before /check passes
        let blocked = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "/handoff".into(),
                    files: vec![],
                },
            )
            .await
            .unwrap();
        assert!(blocked.reply.contains("unlocks after passing `/check`"));

        // /check passes on S2 (read-only)
        let checked = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "/check Tabs were not in the whitespace match".into(),
                    files: vec![],
                },
            )
            .await
            .unwrap();
        assert!(checked.plan_changed);
        assert!(checked.reply.contains("Marked **Trace tokenizer** understood"));

        // /handoff now unlocks Full tools on S2 and advances to p2
        let handoff = rt
            .handle_turn(
                "s1",
                TurnInput {
                    text: "/handoff".into(),
                    files: vec![],
                },
            )
            .await
            .unwrap();
        assert!(handoff.plan_changed);
        assert!(handoff.reply.contains("Fix remaining rules"));
    }
}
