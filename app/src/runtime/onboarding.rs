//! §3 of docs/theory.md: onboarding, and the map of what a task depends on.

use super::*;

impl Runtime {
    pub(super) async fn handle_onboarding(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        self.handle_opening(session, text, files, sink, true).await
    }

    /// A message with no map to work on: a question gets answered; a task
    /// starts onboarding, or with `allow_task` false (a finished thread)
    /// is pointed to a new thread.
    pub(super) async fn handle_opening(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
        allow_task: bool,
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
            ctx: crate::engine::turn_ctx(),
        };
        let (decision, _): (OnboardingDecision, _) = run_gated_streamed(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "onboarding",
            &req,
            // Held: a question's answer comes from its own call, and the
            // fast model sometimes answers here too.
            gate::held(&sink),
            check_onboarding,
            Some("reply"),
        )
        .await?;
        if let Some(lang) = decision.language.as_deref().and_then(crate::i18n::code) {
            session.language = Some(lang);
        }
        if decision.question && !decision.ready_to_map {
            return self
                .answer_question(session, text, files, sink, decision.offer_check)
                .await;
        }
        if !allow_task {
            let goal = decision.goal.unwrap_or_default();
            let reply = if goal.trim().is_empty() {
                decision.reply
            } else {
                format!("This thread's task is done. Start a new thread for: {goal}")
            };
            flush(&sink, &reply, "");
            session.push_turn("assistant", &reply);
            return Ok(TurnOutput::text(String::new()));
        }
        let quiet = decision.check || decision.exam;
        if !quiet {
            flush(&sink, &decision.reply, "");
        }

        if let Some(goal) = decision.goal.filter(|g| !g.trim().is_empty()) {
            session.goal = Some(goal);
            session.quick_check = decision.check && !decision.exam;
            session.exam = decision.exam;
            if decision.exam || decision.check {
                crate::telemetry::emit(pathfinder_schema::Event::Ui {
                    feature: if decision.exam {
                        pathfinder_schema::Feature::Exam
                    } else {
                        pathfinder_schema::Feature::QuickCheck
                    },
                });
            }
        }
        if !quiet {
            session.push_turn("assistant", &decision.reply);
        }
        if decision.ready_to_map {
            sink(StreamEvent::TextDelta("\n\n".to_string()));
            return self.build_map(session, files, sink).await;
        }
        Ok(TurnOutput::text(String::new()))
    }

    pub(super) async fn build_map(
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
                "Goal: {}\n\nWorkspace snapshot:\n{snapshot}{}\n\nOnboarding conversation:\n{}\n\nRead any files you need first, then respond with JSON only.",
                session
                    .goal
                    .as_deref()
                    .unwrap_or("(infer from the conversation)"),
                self.known_concepts_prompt(session),
                format_history(session)
            ),
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        // A quick check shows no map: its candidate follows at once.
        let quick = session.quick_check || session.exam;
        let (proposal, shown): (MapProposal, _) = run_gated_streamed(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "map",
            &req,
            if quick {
                gate::held(&sink)
            } else {
                sink.clone()
            },
            check_map,
            Some("brief"),
        )
        .await?;
        if !quick {
            flush(&sink, &proposal.brief, &shown);
        }

        session.map = proposal.to_map();
        session.active = None;
        session.pending_change = None;
        session.pending_candidate = None;
        session.gaps_raised.clear();
        if let Err(e) = self
            .register_map_concepts(session, &proposal.concepts, "map")
            .await
        {
            tracing::warn!("registering the map's concepts: {e:#}");
        }
        session.advance();

        if quick && let Some(part_id) = session.active.clone() {
            let lead = format!(
                "{}\n\n",
                i18n::line(
                    session.language.as_deref(),
                    if session.exam {
                        i18n::Line::ExamLead
                    } else {
                        i18n::Line::QuickCheckLead
                    }
                )
            );
            sink(StreamEvent::TextDelta(lead.clone()));
            session.push_turn("assistant", lead.trim());
            let ask = self.write_candidate(session, &part_id, files, sink).await?;
            return Ok(TurnOutput::changed(ask));
        }

        // The map itself shows in Zed's plan panel (and on `/map`).
        let rest = self.enter_part(session, true);
        session.push_turn("assistant", format!("{}\n\n{rest}", proposal.brief));
        Ok(TurnOutput::changed(format!("\n\n{rest}")))
    }

    /// §3: the model proposes map changes; you accept or reject them.
    pub(super) async fn handle_replan(
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
            ctx: crate::engine::turn_ctx(),
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
        let reply = render_change(&change, &session.map, session.language.as_deref());
        session.pending_change = Some(change);
        session.push_turn("assistant", &reply);
        Ok(TurnOutput::text(reply))
    }

    pub(super) async fn handle_accept(&self, session: &mut Session) -> Result<TurnOutput> {
        let Some(change) = session.pending_change.take() else {
            return Ok(TurnOutput::text(i18n::line(
                session.language.as_deref(),
                i18n::Line::NoChangeWaiting,
            )));
        };
        match session.map.with_change(&change) {
            Ok(map) => {
                session.map = map;
                if let Err(e) = self
                    .register_map_concepts(session, &change.concepts, "map")
                    .await
                {
                    tracing::warn!("registering the change's concepts: {e:#}");
                }
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
                Ok(TurnOutput::changed(i18n::line(
                    session.language.as_deref(),
                    i18n::Line::MapUpdated,
                )))
            }
            Err(e) => Ok(TurnOutput::text(format!(
                "That change no longer fits the map ({e}). Dropped it."
            ))),
        }
    }

    pub(super) fn handle_next(&self, session: &mut Session) -> TurnOutput {
        let current = session.active.clone();
        match session.map.next_open(current.as_deref()) {
            Some(id) => {
                session.active = Some(id);
                session.mode = SessionMode::Working;
                session.pending_candidate = None;
                TurnOutput::changed(self.enter_part(session, false))
            }
            None if session.map.all_resolved() => {
                session.advance();
                TurnOutput::changed(i18n::line(
                    session.language.as_deref(),
                    i18n::Line::AllResolved,
                ))
            }
            None => TurnOutput::text(i18n::line(
                session.language.as_deref(),
                i18n::Line::NoOtherPart,
            )),
        }
    }

    pub(super) fn handle_focus(&self, session: &mut Session, query: &str) -> TurnOutput {
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
        TurnOutput::changed(self.enter_part(session, false))
    }
}
