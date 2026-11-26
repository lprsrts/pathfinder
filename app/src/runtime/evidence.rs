//! §5 of docs/theory.md: evidence of understanding (the log, the summary, the candidate and its judgment).

use super::*;

impl Runtime {
    /// `decided_by`: the decision model that already classified the message
    /// as a log entry; the LLM then only replies.
    pub(super) async fn handle_log(
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
                render_change(change, &session.map, session.language.as_deref())
            ));
        }
        // The waiting candidate is hidden from this call: it may be flawed on
        // purpose, and a model that can see it tends to vouch for it.
        let mut history = format_history(session);
        if let Some(pending) = &session.pending_candidate {
            history = history.replace(&pending.candidate, CANDIDATE_HIDDEN);
            waiting.push_str(
                "\n\nA candidate for this part is waiting for the user's verdict; it's hidden from you, so you don't know whether it's right. Never say it is right or wrong, or that any detail of it is. Answer questions about the general concepts only. If they ask about the candidate itself (a claim, step or number in it, or whether it's right), say that's what they're judging. Don't mention that it's hidden from you. A message that asks about the candidate is `log`, even if they add that they want to move on; `move` or `known` only when they ask to leave it unjudged and ask nothing about it.",
            );
        }

        // While a secret is active the reply is held for the gate (A6),
        // thinking included: it may re-derive what the candidate tests.
        let gate = self.gate_mode(session);
        if matches!(gate, gate::GateMode::Checked(_)) {
            waiting.push_str(&format!("\n\n{}", gate::NARROW));
        }
        let req = EngineRequest {
            tier: EngineTier::System1,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: LOG_SYSTEM.to_string(),
            prompt: format!(
                "Goal: {}\n\n{}{}\n\nMap:\n{}{waiting}\n\nRecent conversation:\n{}\n\nLatest message: {text}\n\nRead any files you need first, then respond with JSON only.",
                session.goal.as_deref().unwrap_or("(session goal)"),
                describe_part(&part),
                self.concepts_prompt(session, &part),
                session.map.render(session.active.as_deref()),
                history,
            ),
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        let current = session.map.clone();
        let change_waiting = session.pending_change.is_some();
        // The model's own reply is never shown: a question gets sources, a
        // log entry gets none.
        if matches!(gate, gate::GateMode::Checked(_)) {
            sink(StreamEvent::Thought(
                "Checking the reply against what you're judging before showing it.\n".into(),
            ));
        }
        let reply_sink = gate::held(&sink);
        let (mut decision, shown): (LogReply, _) = run_gated_streamed(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "log",
            &req,
            reply_sink,
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
        let _ = shown;
        decision.reply = if crate::sources::is_question(text) {
            self.cite_reply(session, text, files, &sink).await?
        } else {
            i18n::line(session.language.as_deref(), i18n::Line::Logged).to_string()
        };
        if let gate::GateMode::Checked(secrets) = &gate {
            decision.reply = self.gate_reply(session, secrets, decision.reply).await;
        }
        sink(StreamEvent::TextDelta(decision.reply.clone()));

        if let Err(e) = self
            .extract_from_log(
                session,
                text,
                &decision.concepts,
                decision.misconception.as_ref(),
            )
            .await
        {
            tracing::warn!("extracting concepts from the log: {e:#}");
        }
        let mut visible = String::new();
        let mut full = decision.reply;
        if let Some(change) = decision.map_change {
            let rendered = format!(
                "\n\n{}",
                render_change(&change, &session.map, session.language.as_deref())
            );
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
    pub(super) async fn handle_chat(
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
            Some((_, _, _))
                if (session.quick_check || session.exam) && session.pending_candidate.is_some() =>
            {
                // Not a verdict on the quick check: the judge decides whether
                // it is one, and drops the check if not.
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
                    "replan" => Intent::Replan,
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
    pub(super) async fn decide_step(
        &self,
        session: &Session,
        text: &str,
        sink: &StreamSink,
    ) -> Option<(String, Option<String>, String)> {
        if session.decisions_off {
            return None;
        }
        let part = session.active_part()?;
        let mut steps: Vec<(String, String)> = vec![
            ("log".into(), "Their thinking, reasoning, an attempt, or a question (including a question about a waiting candidate).".into()),
            ("summary".into(), "They offer their own explanation of the whole current part, ready to be checked.".into()),
            ("known".into(), "They clearly say they already know this whole part and want to skip it.".into()),
            ("move".into(), "They want to go on to the next part, or to another named part.".into()),
            ("replan".into(), "They ask to change the plan, or say something that changes the whole task's scope or purpose (a new constraint, requirement or aim).".into()),
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
            for (step, text) in steps.iter_mut() {
                if step == "move" || step == "known" {
                    text.push_str(" Only when they ask to leave the waiting candidate unjudged and ask nothing about it; a message that asks about the candidate is `log` or a verdict, even if they add that they want to move on.");
                }
            }
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
                instructions: "Which part do they ask to go to, if any (also when they skip this one)?".into(),
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
                render_change(change, &session.map, session.language.as_deref())
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

        let prefer = session.decision_model.as_deref();
        let decision = match self.decider.decide(&input, &questions, prefer).await {
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
        tracing::debug!(step = %step.choice, p = step.probability, text, "decided step");
        // While a candidate waits, only a confident "log", move or skip
        // passes the judge: a move or skip drops the candidate, and the judge
        // sends anything that isn't a verdict back to chat anyway.
        let mut choice = step.choice.clone();
        if session.pending_candidate.is_some()
            && matches!(choice.as_str(), "log" | "move" | "known")
            && step.probability < CONFIDENT_STEP
        {
            choice = VERDICT.to_string();
        }
        let part = decision
            .answers
            .get("part")
            .filter(|a| a.probability >= DECISION_THRESHOLD)
            .map(|a| a.choice.clone());
        Some((choice, part, decision.by))
    }

    /// Carries out what a chat message meant, the way its command would.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn route_intent(
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
            Intent::Known => self.handle_known(session, part),
            Intent::Move => Ok(match part.filter(|p| !p.is_empty() && *p != "next") {
                Some(id) => self.handle_focus(session, id),
                None => self.handle_next(session),
            }),
            Intent::Accept => self.handle_accept(session).await,
            Intent::Reject => Ok(match session.pending_change.take() {
                Some(_) => TurnOutput::text(i18n::line(
                    session.language.as_deref(),
                    i18n::Line::ChangeRejected,
                )),
                None => TurnOutput::text(i18n::line(
                    session.language.as_deref(),
                    i18n::Line::NoChangeWaiting,
                )),
            }),
            Intent::Replan => self.handle_replan(session, text, files, sink).await,
            Intent::Log => unreachable!("log entries are answered in handle_log"),
        }
    }

    pub(super) async fn handle_summary(
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

        let visible = self.write_candidate(session, &part.id, files, sink).await?;
        Ok(TurnOutput::text(visible))
    }

    /// Writes a candidate for the part and leaves it waiting (§5.3). It
    /// checks the concepts A5 picks; an open misconception makes it flawed
    /// on purpose, embodying that belief. Returns what is left to show.
    pub(super) async fn write_candidate(
        &self,
        session: &mut Session,
        part_id: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<String> {
        let part = session
            .map
            .get(part_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown part `{part_id}`"))?;
        let picked = self.next_targets(session, part_id);
        let mut flawed = (self.flaw_coin)();
        let mut checks = String::new();
        let (mut targets, mut misconception) = (Vec::new(), None);
        if let Some((t, m, labels)) = picked {
            let names: Vec<String> = t
                .concepts
                .iter()
                .map(|c| format!("{} (`{c}`)", labels.get(c).map(String::as_str).unwrap_or(c)))
                .collect();
            checks = format!(
                "\n\nThis candidate checks: {}. Build it around these concepts.",
                names.join(", ")
            );
            if let Some(m) = &m {
                flawed = true;
                checks.push_str(&format!(
                    "\nThe user showed this wrong belief: \"{}\". Make the one flaw embody it, so only someone past it would catch it.",
                    m.statement
                ));
            }
            targets = t.concepts;
            misconception = m.map(|m| m.id);
        }
        let exam = session.exam;
        let what = if exam {
            let asked = exam_questions_asked(session);
            format!(
                "Ask one exam question.{}",
                if asked.is_empty() {
                    String::new()
                } else {
                    format!(
                        " Already asked (ask something else):\n- {}",
                        asked.join("\n- ")
                    )
                }
            )
        } else if flawed {
            "Make this candidate FLAWED: exactly one real flaw.".to_string()
        } else {
            "Make this candidate CORRECT.".to_string()
        };
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: if exam {
                EXAM_QUESTION_SYSTEM
            } else {
                CANDIDATE_SYSTEM
            }
            .to_string(),
            prompt: format!(
                "Goal: {}\n\nMap (`▶` is this part; parts after it in `needs` order may be new to the user):\n{}\n\n{}{checks}\n\n{what}\n\nRead any files you need first, then respond with JSON only.",
                session.goal.as_deref().unwrap_or("(session goal)"),
                session.map.render(Some(&part.id)),
                describe_part(&part),
            ),
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        let lang = session.language.as_deref();
        let header = format!(
            "{} **{}**:\n\n",
            i18n::line(
                lang,
                if exam {
                    i18n::Line::ExamQuestionFor
                } else {
                    i18n::Line::CandidateFor
                }
            ),
            part.title
        );
        // The candidate's reasoning would give away whether (and where) it's
        // flawed, so it's never shown. The header goes right before the
        // candidate's first words, after any files the model reads.
        sink(StreamEvent::Thought(
            "Writing a candidate for you to judge; its reasoning stays hidden.\n".into(),
        ));
        let header_sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let candidate_sink: StreamSink = {
            let (inner, header, sent) = (sink.clone(), header.clone(), header_sent.clone());
            Arc::new(move |ev| match ev {
                StreamEvent::Thought(_) => {}
                StreamEvent::TextDelta(t) => {
                    if !sent.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        inner(StreamEvent::TextDelta(header.clone()));
                    }
                    inner(StreamEvent::TextDelta(t));
                }
                other => inner(other),
            })
        };
        let (proposal, shown): (CandidateProposal, _) = run_gated_streamed(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "candidate",
            &req,
            candidate_sink,
            check_candidate,
            Some("candidate"),
        )
        .await?;
        if !header_sent.load(std::sync::atomic::Ordering::Relaxed) {
            sink(StreamEvent::TextDelta(header.clone()));
        }
        flush(&sink, &proposal.candidate, &shown);

        let ask = format!(
            "\n\n{}",
            i18n::line(
                session.language.as_deref(),
                if exam {
                    i18n::Line::ExamAnswer
                } else {
                    i18n::Line::IsItRight
                }
            )
        );
        let reply = format!("{header}{}{ask}", proposal.candidate);
        session.push_turn("assistant", &reply);
        // The key goes to the secrets table, never into the thread's state.
        let secret = if exam {
            flawed = false;
            Secret::exam_key(
                &proposal.candidate,
                &proposal.key_reason,
                session.history.len() - 1,
            )
        } else {
            Secret::candidate_key(
                &proposal.candidate,
                !flawed,
                &proposal.key_reason,
                session.history.len() - 1,
            )
        };
        self.store.add_secret(&session.id, &secret)?;
        session.pending_candidate = Some(PendingCandidate {
            part_id: part.id,
            candidate: proposal.candidate,
            key_correct: !flawed,
            key_reason: proposal.key_reason,
            secret: Some(secret.id),
            targets,
            misconception,
            id: format!("cand-{}", uuid::Uuid::new_v4().simple()),
        });
        Ok(ask)
    }

    pub(super) async fn handle_judgment(
        &self,
        session: &mut Session,
        judgment: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let mut pending = session.pending_candidate.take().unwrap();
        if let Some(id) = &pending.secret {
            match self.store.secret(id)? {
                Some(secret) => {
                    pending.key_correct = secret.correct();
                    pending.key_reason = secret.reason().to_string();
                }
                None => return Err(anyhow!("the candidate's key is missing")),
            }
        }
        let Some(part) = session.map.get(&pending.part_id).cloned() else {
            return Ok(TurnOutput::text(
                "That candidate's part is no longer on the map.",
            ));
        };
        session.push_turn("user", judgment);

        let prompt = if session.exam {
            format!(
                "Part: {}\n\nExam question:\n{}\n\nHidden key, the points a good answer contains: {}\n\nUser's answer: {judgment}\n\nRespond with JSON only.",
                part.title, pending.candidate, pending.key_reason,
            )
        } else {
            format!(
                "Part: {}\n\nCandidate:\n{}\n\nHidden key: the candidate is {}. {}\n\nUser's judgment: {judgment}\n\nRespond with JSON only.",
                part.title,
                pending.candidate,
                if pending.key_correct {
                    "CORRECT"
                } else {
                    "FLAWED"
                },
                pending.key_reason,
            )
        };
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: if session.exam {
                EXAM_JUDGE_SYSTEM
            } else {
                JUDGE_SYSTEM
            }
            .to_string(),
            prompt,
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        // The Judge holds the key: its thinking is never shown, and its
        // feedback only once it confirms the message is a verdict (A2, A6).
        sink(StreamEvent::Thought(
            "Judging your verdict against the hidden key; that reasoning stays hidden.\n".into(),
        ));
        let verdict: JudgmentVerdict = run_gated(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "judgment",
            &req,
            gate::quiet(),
            check_verdict,
        )
        .await?;
        let shown = String::new();
        if !verdict.is_judgment {
            session.history.pop();
            // A quick check left unanswered: drop it, nothing recorded, and
            // answer the message as chat.
            if session.quick_check || session.exam {
                crate::telemetry::emit(pathfinder_schema::Event::Check {
                    outcome: pathfinder_schema::CheckOutcome::Skipped,
                    concepts: pending.targets.len() as u32,
                    quick: true,
                });
                end_quick_check(session);
                return self
                    .handle_opening(session, judgment, files, sink, true)
                    .await;
            }
            // A question or another step: the candidate keeps waiting.
            session.pending_candidate = Some(pending);
            return self.handle_log(session, judgment, files, sink, None).await;
        }
        flush(&sink, &verdict.feedback, &shown);
        crate::telemetry::emit(pathfinder_schema::Event::Check {
            outcome: if verdict.pass {
                pathfinder_schema::CheckOutcome::Right
            } else {
                pathfinder_schema::CheckOutcome::Wrong
            },
            concepts: pending.targets.len() as u32,
            quick: session.quick_check || session.exam,
        });
        self.record_verdict(
            session,
            &pending,
            verdict.pass,
            verdict.misconception.as_deref(),
        );

        // §10: the part is understood once every core concept has evidence;
        // a part from before concepts were recorded needs one pass.
        let status = self.understanding_status(session, &part.id);
        let understood = match status {
            Some(s) => s == crate::understanding::UnderstandingStatus::Understood,
            None => verdict.pass,
        };
        // An exam goes on: the next question, on the next part.
        if session.exam {
            if understood {
                self.set_status(session, &part.id, PartStatus::Understood, &verdict.feedback)?;
            }
            session.push_turn("assistant", &verdict.feedback);
            let ids: Vec<String> = session.map.parts.iter().map(|p| p.id.clone()).collect();
            let at = ids.iter().position(|id| *id == part.id).unwrap_or(0);
            let next_id = ids[(at + 1) % ids.len().max(1)].clone();
            session.active = Some(next_id.clone());
            sink(StreamEvent::TextDelta("\n\n".into()));
            let ask = self.write_candidate(session, &next_id, files, sink).await?;
            return Ok(TurnOutput {
                reply: ask,
                plan_changed: true,
            });
        }
        // A quick check is one candidate: record it and go back to chat.
        if session.quick_check {
            if understood {
                self.set_status(session, &part.id, PartStatus::Understood, &verdict.feedback)?;
            }
            session.push_turn("assistant", &verdict.feedback);
            end_quick_check(session);
            return Ok(TurnOutput::changed(String::new()));
        }
        if verdict.pass && !understood {
            let next = format!(
                "\n\n{}\n\n",
                i18n::line(session.language.as_deref(), i18n::Line::PassedOneMore)
            );
            session.push_turn("assistant", format!("{}{next}", verdict.feedback));
            sink(StreamEvent::TextDelta(next.to_string()));
            let ask = self.write_candidate(session, &part.id, files, sink).await?;
            return Ok(TurnOutput::text(ask));
        }
        let next = if understood {
            self.set_status(session, &part.id, PartStatus::Understood, &verdict.feedback)?;
            format!(
                "\n\n{}",
                i18n::line(session.language.as_deref(), i18n::Line::Understood)
                    .replace("{title}", &part.title)
            )
        } else {
            format!(
                "\n\n{}",
                i18n::line(session.language.as_deref(), i18n::Line::KeepWorking)
            )
        };
        session.push_turn("assistant", format!("{}{next}", verdict.feedback));
        Ok(TurnOutput::changed(next))
    }
}
