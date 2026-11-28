//! §5 of docs/theory.md: evidence of understanding (the log, the summary, the candidate and its judgment).

use super::*;
use crate::domain::CommitKind;
use crate::understanding::Angle;

/// What the Router read a chat message as.
pub(super) enum Routed {
    /// A step, the part it names, who decided, and how you steered the
    /// next step (§16).
    Step {
        step: String,
        part: Option<String>,
        by: String,
        steer: Option<Angle>,
    },
    /// Unsure, but leaning toward a commitment: the two likeliest (§15).
    Unsure { top: Vec<&'static str> },
}

/// The Tutor's acknowledgement of a log entry, at most this many
/// characters.
const ACK_MAX: usize = 200;

/// The Tutor's open question for the next step, at most this many
/// characters.
const QUESTION_MAX: usize = 240;

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
        self.handle_log_steered(session, text, files, sink, decided_by, None)
            .await
    }

    /// `handle_log`, with how you asked the next step to start ("give me an
    /// example first"): the message is logged and answered as any other,
    /// and the next step after the reply is re-chosen with your steer.
    pub(super) async fn handle_log_steered(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
        decided_by: Option<&str>,
        steer: Option<Angle>,
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
        // The waiting candidate is hidden from this call (`format_history`).
        let history = self.format_history(session);
        if session.pending_candidate.is_some() {
            waiting.push_str(
                "\n\nA candidate for this part is waiting for the user's verdict; it's hidden from you, so you don't know whether it's right. A message that asks about the candidate is `log`, even if they add that they want to move on; `move` or `known` only when they ask to leave it unjudged and ask nothing about it.",
            );
        }

        let gate = self.gate_mode(session);
        let req = EngineRequest {
            tier: EngineTier::System1,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: in_language(LOG_SYSTEM, session),
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
        // The model's free `reply` is never shown: a question gets sources,
        // a log entry the Tutor's `ack`, and only once code has checked
        // that it adds nothing to your words (`plain_ack`).
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
        // A question gets its sources; a log entry a short acknowledgement
        // of what you did, adding nothing (§11), and both end with the
        // next step (§16).
        decision.reply = if crate::sources::is_question(text) {
            self.cite_reply(session, text, files, &sink).await?
        } else if let Some(ack) = plain_ack(&decision.ack, text, &part.title) {
            ack
        } else {
            i18n::line(session.language.as_deref(), i18n::Line::Logged).to_string()
        };
        // A map change is the Tutor's words too (its brief and part titles):
        // while a secret is active it passes the Gate with the reply, or is
        // dropped with it (A6).
        let mut map_change = decision.map_change.take();
        if let gate::GateMode::Checked(secrets) = &gate {
            let draft = match &map_change {
                Some(change) => format!(
                    "{}\n\n{}",
                    decision.reply,
                    render_change(change, &session.map, session.language.as_deref())
                ),
                None => decision.reply.clone(),
            };
            let shown = self.gate_reply(session, secrets, draft.clone()).await;
            if shown != draft {
                decision.reply = shown;
                map_change = None;
            }
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
        // §16: one exact next step, with the Tutor's open question when it
        // wrote one.
        let question = one_question(&decision.question);
        if let Some(next) = self.next_step(session, steer, question.as_deref(), steer.is_some()) {
            let next = format!("\n\n{next}");
            sink(StreamEvent::TextDelta(next.clone()));
            full.push_str(&next);
        }
        if let Some(change) = map_change {
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
    /// A commitment read from plain words is shown back first (§15).
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
        let routed = self.decide_step(session, text, &sink).await;
        let routed = second_look(session, text, routed);
        // Something waits for your OK: only a clear yes runs it; any other
        // message drops it, and nothing ever runs on its own.
        let mut dropped = None;
        if session.pending_commitment.is_some() {
            if matches!(&routed, Some(Routed::Step { step, .. }) if step == CONFIRM) {
                if session
                    .pending_commitment
                    .as_ref()
                    .is_some_and(|p| p.kind != CommitKind::Verdict)
                {
                    session.push_turn("user", text);
                }
                return self.confirm(session, sink).await;
            }
            if let Some(line) = self.drop_pending(session) {
                sink(StreamEvent::TextDelta(format!("{line}\n\n")));
                dropped = Some(line);
            }
        }
        let before = session.history.len();
        let out = self.route_chat(session, routed, text, files, sink).await?;
        // A reply no step kept (a move's part card, a refusal) is kept
        // here, so the thread never holds a message left unanswered.
        if !out.reply.trim().is_empty()
            && !session.history[before.min(session.history.len())..]
                .iter()
                .any(|t| t.role == "assistant")
        {
            session.push_turn("assistant", out.reply.trim_start());
        }
        if let Some(line) = dropped
            && let Some(reply) = session
                .history
                .iter_mut()
                .skip(before)
                .find(|t| t.role == "assistant")
        {
            reply.content = format!("{line}\n\n{}", reply.content);
        }
        Ok(out)
    }

    /// Carries out the step the Router read.
    async fn route_chat(
        &self,
        session: &mut Session,
        routed: Option<Routed>,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let checking = (session.quick_check || session.exam) && session.pending_candidate.is_some();
        // "… I think I understand it now, check me": a check on what you
        // just explained, whatever step the Router read in the words before
        // it (a long explanation reads like a question or a note). Asking
        // to be checked records nothing: the verdict is shown back.
        let other_step = matches!(&routed, Some(Routed::Step { step, .. })
            if !matches!(step.as_str(), "log" | "summary" | "known"));
        if !other_step
            && session.pending_candidate.is_none()
            && let Some(explained) = super::refer::asks_check(text)
            && let Some(part) = session
                .active_part()
                .filter(|p| p.status == PartStatus::Open)
                .cloned()
        {
            session.push_turn("user", text);
            if let Some(p) = session.active_part_mut() {
                if explained || p.log.is_empty() {
                    p.log.push(text.to_string());
                }
                if explained && !part.log.is_empty() {
                    p.summary = Some(text.to_string());
                }
            }
            let visible = self.write_candidate(session, &part.id, files, sink).await?;
            return Ok(TurnOutput::text(visible));
        }
        match routed {
            Some(Routed::Unsure { top }) => match self.ask_which(session, text, files, &top) {
                Some(out) => Ok(out),
                None => self.handle_log(session, text, files, sink, None).await,
            },
            // An exam question you chose to be asked: your answer goes to
            // the judge, which decides whether it is one and drops the exam
            // if not. An answer isn't a verdict on a candidate.
            Some(_) | None if checking && session.exam => {
                self.handle_judgment(session, text, files, sink).await
            }
            // A quick check's candidate: a verdict read from plain words is
            // shown back for one Enter, as on any candidate (§15). Only a
            // sure question about it is answered as one, the check still
            // waiting.
            Some(Routed::Step { step, by, .. }) if checking && step == "log" => {
                self.handle_log(session, text, files, sink, Some(&by)).await
            }
            Some(_) | None if checking => {
                session.push_turn("user", text);
                Ok(self.propose(session, CommitKind::Verdict, text, files))
            }
            Some(Routed::Step { step, .. }) if step == VERDICT => {
                session.push_turn("user", text);
                Ok(self.propose(session, CommitKind::Verdict, text, files))
            }
            Some(Routed::Step { step, .. }) if step == APPROVE || step == OBJECT => {
                session.push_turn("user", text);
                let kind = if step == APPROVE {
                    CommitKind::Approve
                } else {
                    CommitKind::Object
                };
                Ok(self.propose(session, kind, text, files))
            }
            Some(Routed::Step {
                step,
                by,
                steer: Some(angle),
                ..
            }) if step == "log" => {
                // "Give me an example first": logged and answered as any
                // message (a question still gets its sources), and the next
                // step re-chosen with your steer.
                self.handle_log_steered(session, text, files, sink, Some(&by), Some(angle))
                    .await
            }
            Some(Routed::Step { step, by, .. }) if step == "log" || step == CONFIRM => {
                self.handle_log(session, text, files, sink, Some(&by)).await
            }
            Some(Routed::Step { step, .. }) if step == UNDO => {
                session.push_turn("user", text);
                self.undo(session)
            }
            Some(Routed::Step { step, part, .. }) => {
                // A claim on the part your words name ("I know the part on
                // page 19"), never silently on another one.
                if step == "known"
                    && let Some(out) = self.claim_target(session, text, files, part.as_deref())?
                {
                    return Ok(out);
                }
                let intent = match step.as_str() {
                    "summary" => Intent::Summary,
                    "known" => Intent::Known,
                    "move" => Intent::Move,
                    "accept" => Intent::Accept,
                    "replan" => Intent::Replan,
                    AGENT => Intent::Agent,
                    TELL => Intent::Tell,
                    _ => Intent::Reject,
                };
                let had_log = session.active_part().is_some_and(|p| !p.log.is_empty());
                session.push_turn("user", text);
                self.route_intent(session, intent, part.as_deref(), text, had_log, files, sink)
                    .await
            }
            // Unsure while a candidate waits: whether it's your verdict is
            // shown back for your OK, never guessed.
            None if session.pending_candidate.is_some() => {
                session.push_turn("user", text);
                Ok(self.propose(session, CommitKind::Verdict, text, files))
            }
            None => self.handle_log(session, text, files, sink, None).await,
        }
    }

    /// Asks the decision model which step `text` is. `None` when there's no
    /// decision model, it fails, or it isn't sure enough: the LLM decides.
    /// `Unsure` when it isn't sure but leans toward a commitment (§15).
    pub(super) async fn decide_step(
        &self,
        session: &Session,
        text: &str,
        sink: &StreamSink,
    ) -> Option<Routed> {
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
            (AGENT.into(), "They ask for the agent to do this part or another one they name, or for something to be written, saved or changed in their files (do it for them, hand it off, delegate it, write the letter).".into()),
            (TELL.into(), "They ask to be told the answer or an explanation outright, rather than pointed to sources.".into()),
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
        // The agent's result waits for your review: accepting it or saying
        // what's wrong is your verdict on it, shown back first (§7, §15).
        if part.status == PartStatus::InReview {
            steps.push((
                APPROVE.into(),
                "They accept the agent's result waiting for their review (it looks good, approve it).".into(),
            ));
            steps.push((
                OBJECT.into(),
                "They say what's wrong with the agent's result and why, to send it back.".into(),
            ));
        }
        if session.undoable() {
            steps.push((UNDO.into(), "They take back what they just claimed.".into()));
        }
        if let Some(p) = &session.pending_commitment {
            let title = session
                .map
                .get(&p.part)
                .map(|p| p.title.clone())
                .unwrap_or_default();
            let what = match p.kind {
                CommitKind::Verdict => {
                    "judge their last message as their verdict on the candidate for"
                }
                CommitKind::Handoff => "hand off to the agent",
                CommitKind::Delegate => "delegate to the agent without understanding it",
                CommitKind::Approve => "approve the agent's result for",
                CommitKind::Object => {
                    "send back to the agent, with their message as the objection,"
                }
            };
            steps.push((
                CONFIRM.into(),
                format!("They agree to the step waiting for their OK: {what} **{title}**."),
            ));
        }
        if session.pending_candidate.is_some() {
            for (step, text) in steps.iter_mut() {
                if step == "move" || step == "known" || step == AGENT {
                    text.push_str(" Only when they ask to leave the waiting candidate unjudged and ask nothing about it; a message that asks about the candidate is `log` or a verdict, even if they add that they want to move on.");
                }
            }
            steps.push((VERDICT.into(), "They give their verdict on the waiting candidate: whether it's right, ideally why (\"I don't know\" counts).".into()));
        }
        let n_steps = steps.len();
        let offered: Vec<&'static str> = COMMITMENTS
            .into_iter()
            .filter(|c| steps.iter().any(|(s, _)| s == c))
            .collect();
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
        questions.push(Question {
            name: "steer".into(),
            instructions: "Do they ask for a particular way into what to look at next?".into(),
            options: vec![
                ("none".into(), "No such request.".into()),
                (
                    "example".into(),
                    "They want a concrete example first.".into(),
                ),
                (
                    "purpose".into(),
                    "They want to start from what it's for.".into(),
                ),
                (
                    "mechanism".into(),
                    "They want to start from how it works.".into(),
                ),
            ],
        });

        let mut input = format!(
            "Goal: {}\nCurrent part: {} (`{}`): {}\nLog entries on this part so far: {}\n",
            session.goal.as_deref().unwrap_or("(none)"),
            part.title,
            part.id,
            part.note,
            part.log.len()
        );
        if part.status == PartStatus::InReview {
            input.push_str("The agent's result on this part is waiting for the user's review.\n");
        }
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
        // A review waiting for your verdict is hidden: the Router may not
        // see a planted hunk (A2).
        let reviews = self.waiting_reviews(session);
        for (i, t) in session.history[start..].iter().enumerate() {
            let content = if t.role == "assistant" && reviews.contains(&(start + i)) {
                REVIEW_HIDDEN
            } else {
                t.content.as_str()
            };
            input.push_str(&format!("{}: {content}\n", t.role));
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
            // Unsure, but it leans toward something that changes your
            // record: ask which you meant, never guess (§15). While a
            // candidate waits, the verdict's show-back is that question.
            let mass: f64 = offered.iter().map(|c| step.p(c, n_steps)).sum();
            // Any one commitment the model actually weighs (not an even
            // split of what's left) counts as meaningful.
            let weighed = offered
                .iter()
                .filter_map(|c| {
                    step.all
                        .get(*c)
                        .copied()
                        .or((step.choice == *c).then_some(step.probability))
                })
                .any(|p| p >= UNSURE_ONE);
            if session.pending_candidate.is_none() && (mass >= UNSURE_COMMITMENT || weighed) {
                let mut top = offered.clone();
                top.sort_by(|a, b| step.p(b, n_steps).total_cmp(&step.p(a, n_steps)));
                top.truncate(2);
                return Some(Routed::Unsure { top });
            }
            return None;
        }
        tracing::debug!(step = %step.choice, p = step.probability, text, "decided step");
        let mut choice = step.choice.clone();
        // Agreeing to what waits needs a clear yes.
        if choice == CONFIRM && step.probability < CONFIDENT_STEP {
            return None;
        }
        // While a candidate waits, only a confident "log", move, skip or
        // agent step passes the verdict: those drop the candidate, and the
        // judge sends anything that isn't a verdict back to chat anyway.
        if session.pending_candidate.is_some()
            && matches!(choice.as_str(), "log" | "move" | "known" | AGENT)
            && step.probability < CONFIDENT_STEP
        {
            choice = VERDICT.to_string();
        }
        let part = decision
            .answers
            .get("part")
            .filter(|a| a.probability >= DECISION_THRESHOLD)
            .map(|a| a.choice.clone());
        let steer = decision
            .answers
            .get("steer")
            .filter(|a| a.probability >= DECISION_THRESHOLD)
            .and_then(|a| Angle::parse(&a.choice));
        Some(Routed::Step {
            step: choice,
            part,
            by: decision.by,
            steer,
        })
    }

    /// Carries out what a chat message meant, the way its command would. A
    /// commitment is shown back first (§15).
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
            Intent::Known => self.claim_shown_back(session, part, None),
            Intent::Agent => Ok(self.agent_step(session, text, files, part)),
            // Something to write into your files is work for the agent: a
            // told answer can't save anything (§11).
            Intent::Tell if super::refer::wants_writing(text) => {
                Ok(self.agent_step(session, text, files, part))
            }
            Intent::Tell => self.tell(session, text, files, sink, true).await,
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
        self.write_candidate_led(session, part_id, files, sink, None)
            .await
    }

    /// [`Self::write_candidate`] with a `lead` line ("Quick check:") shown
    /// with the candidate's header, after any files the model reads, so it
    /// never stands alone as an empty heading.
    pub(super) async fn write_candidate_led(
        &self,
        session: &mut Session,
        part_id: &str,
        files: &[PathBuf],
        sink: StreamSink,
        lead: Option<&str>,
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
            // An exam question has no flaw to embody a belief, so it never
            // aims at (or closes) a misconception (A5 T1).
            let m = m.filter(|_| !session.exam);
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
            system_prompt: in_language(
                if exam {
                    EXAM_QUESTION_SYSTEM
                } else {
                    CANDIDATE_SYSTEM
                },
                session,
            ),
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
            "{}{}\n\n",
            lead.map(|l| format!("{l}\n\n")).unwrap_or_default(),
            i18n::line(
                lang,
                if exam {
                    i18n::Line::ExamQuestionFor
                } else {
                    i18n::Line::CandidateFor
                }
            )
            .replace("{title}", &part.title)
        );
        // The candidate's reasoning would give away whether (and where) it's
        // flawed, so it's never shown. The header goes right before the
        // candidate's first words, after any files the model reads.
        sink(StreamEvent::Thought("Writing a candidate…\n".into()));
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
        // The Judge holds the key, so it gets no tools (§11): only the
        // candidate, the key, your verdict and the files you attached.
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::None,
            cwd: session.cwd.clone(),
            system_prompt: in_language(
                if session.exam {
                    EXAM_JUDGE_SYSTEM
                } else {
                    JUDGE_SYSTEM
                },
                session,
            ),
            prompt,
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        // The Judge holds the key: its thinking is never shown, and its
        // feedback only once it confirms the message is a verdict (A2, A6).
        sink(StreamEvent::Thought("Judging your verdict…\n".into()));
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
        // A verdict never moves a part that has gone on (in review, done,
        // delegated, skipped): the evidence is recorded, the status stays.
        let movable = matches!(part.status, PartStatus::Open | PartStatus::Understood);
        // An exam goes on: the next question, on the next part.
        if session.exam {
            if understood && movable {
                self.set_status(session, &part.id, PartStatus::Understood, &verdict.feedback)?;
            }
            session.push_turn("assistant", &verdict.feedback);
            let ids: Vec<String> = session.map.parts.iter().map(|p| p.id.clone()).collect();
            let at = ids.iter().position(|id| *id == part.id).unwrap_or(0);
            let next_id = ids[(at + 1) % ids.len().max(1)].clone();
            session.active = Some(next_id.clone());
            // Judged and recorded: saved before the next question, so an
            // error writing it can't leave this one waiting to be judged
            // (and passed) again.
            self.store.save_session(session)?;
            sink(StreamEvent::TextDelta("\n\n".into()));
            let ask = self.write_candidate(session, &next_id, files, sink).await?;
            return Ok(TurnOutput::text(ask));
        }
        // A quick check is one candidate: record it and go back to chat.
        if session.quick_check {
            if understood && movable {
                self.set_status(session, &part.id, PartStatus::Understood, &verdict.feedback)?;
            }
            session.push_turn("assistant", &verdict.feedback);
            end_quick_check(session);
            return Ok(TurnOutput::text(String::new()));
        }
        if verdict.pass && !understood {
            let next = format!(
                "\n\n{}\n\n",
                i18n::line(session.language.as_deref(), i18n::Line::PassedOneMore)
            );
            session.push_turn("assistant", format!("{}{next}", verdict.feedback));
            // Saved before the next candidate, as for an exam.
            self.store.save_session(session)?;
            sink(StreamEvent::TextDelta(next.to_string()));
            let ask = self.write_candidate(session, &part.id, files, sink).await?;
            return Ok(TurnOutput::text(ask));
        }
        let next = if understood {
            if movable {
                self.set_status(session, &part.id, PartStatus::Understood, &verdict.feedback)?;
            }
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
        Ok(TurnOutput::text(next))
    }
}

/// A second look by code at what the Router read (§15, A3): words that
/// plausibly make a commitment ("I understand this part", "just tell me")
/// are asked about, never logged silently, even when the Router read
/// them as a log entry or wasn't sure. A clear commitment, anything while
/// a candidate waits (its show-back is that question) and a yes to what
/// waits for your OK are left as read.
fn second_look(session: &Session, text: &str, routed: Option<Routed>) -> Option<Routed> {
    if session.pending_candidate.is_some() || session.quick_check || session.exam {
        return routed;
    }
    // "Looks good, approve it" on a result waiting for your review: your
    // approval, shown back for Enter, whatever the Router read (or if it
    // wasn't asked). Code alone, as for a request for the agent.
    let in_review = session
        .active_part()
        .is_some_and(|p| p.status == PartStatus::InReview);
    let decided = matches!(&routed, Some(Routed::Step { step, .. })
        if step == APPROVE || step == OBJECT || step == CONFIRM);
    if in_review && !decided && super::refer::asks_approve(text) {
        return Some(Routed::Step {
            step: APPROVE.into(),
            part: None,
            by: "code".into(),
            steer: None,
        });
    }
    let read_as_log = matches!(&routed, Some(Routed::Step { step, .. }) if step == "log");
    let commitment = super::refer::plain_commitment(text);
    // "I get the bill now. Let the agent write the letter": a claim the
    // Router read in words that ask for the agent. The agent's step waits
    // for your OK; a claim would be recorded at once, on the wrong part.
    if let Some(Routed::Step {
        step,
        part,
        by,
        steer,
    }) = routed
    {
        let step = if step == "known" && commitment == Some(AGENT) {
            AGENT.to_string()
        } else {
            step
        };
        let routed = Some(Routed::Step {
            step,
            part,
            by,
            steer,
        });
        return match commitment {
            Some(c) if read_as_log => Some(Routed::Unsure { top: vec![c] }),
            _ => routed,
        };
    }
    match commitment {
        // A claim the Router read as a log entry or wasn't sure about: the
        // Tutor would log it silently. Asking to be told or for the agent,
        // when unsure, the Tutor still reads and shows back itself.
        Some(c) if read_as_log || (routed.is_none() && c == "known") => {
            Some(Routed::Unsure { top: vec![c] })
        }
        _ => routed,
    }
}

/// Words an acknowledgement may use beyond yours: who did it and what kind
/// of thing they did, never what it is about.
const ACK_WORDS: &[&str] = &[
    "you",
    "your",
    "yours",
    "yourself",
    "linked",
    "link",
    "connected",
    "connect",
    "traced",
    "trace",
    "tied",
    "related",
    "relate",
    "described",
    "describe",
    "explained",
    "explain",
    "wrote",
    "said",
    "noted",
    "put",
    "own",
    "words",
    "between",
    "together",
    "these",
    "those",
    "them",
    "its",
    "through",
    "way",
    "back",
    "step",
    "steps",
    "part",
    "think",
    "thinking",
    "thought",
    "idea",
    "ideas",
    "reasoning",
    "here",
    "just",
    "also",
    "all",
    "out",
    "walked",
    "summed",
    "summarized",
    "went",
    "how",
    "what",
    "why",
    "who",
    "both",
    "each",
    "one",
    "two",
    "first",
    "then",
    "now",
    "got",
    "gave",
    "set",
    "laid",
    "spelled",
    "stated",
    "named",
];

/// The Tutor's acknowledgement of a log entry, if code can tell it adds
/// nothing (§11): one sentence of at most `ACK_WORDS_MAX` words, not a
/// question, whose content words all come from your message or the
/// part's title. Else None, and the fixed "Logged." is shown.
fn plain_ack(ack: &str, text: &str, title: &str) -> Option<String> {
    let ack = ack.trim();
    if ack.is_empty()
        || ack.contains('\n')
        || ack.ends_with('?')
        || ack.split_whitespace().count() > ACK_WORDS_MAX
        || ack.chars().count() > ACK_MAX
        || more_than_one_sentence(ack)
    {
        return None;
    }
    let yours: Vec<String> = crate::sources::keywords(&format!("{text} {title}"));
    let adds = crate::sources::keywords(ack).into_iter().any(|w| {
        !ACK_WORDS.contains(&w.as_str()) && !yours.iter().any(|y| super::refer::alike(&w, y))
    });
    (!adds).then(|| ack.to_string())
}

/// The Tutor's open question, if it is one: a single sentence ending in
/// "?". Else None, and the fixed question for the step is asked.
fn one_question(q: &str) -> Option<String> {
    let q = q.trim();
    (q.ends_with('?')
        && !q.contains('\n')
        && q.chars().count() <= QUESTION_MAX
        && !more_than_one_sentence(q))
    .then(|| q.to_string())
}

/// Whether `s` has a sentence end before its last character.
fn more_than_one_sentence(s: &str) -> bool {
    let body = s.trim_end_matches(['.', '!', '?', ' ']);
    body.contains(". ") || body.contains("? ") || body.contains("! ") || body.contains("; ")
}

/// Words an acknowledgement may have, at most.
const ACK_WORDS_MAX: usize = 20;
