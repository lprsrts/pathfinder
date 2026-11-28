//! §15 of docs/theory.md: a commitment read from plain words is shown back
//! in one line before it counts, and can be undone in one step.
//!
//! - A claim (`known`) is recorded as it is shown back, and `/undo` takes
//!   it back in the very next turn, with compensating events.
//! - A verdict, a handoff and a delegation wait for your OK (Enter runs,
//!   Esc cancels): nothing runs before you confirm it, and once judged or
//!   run it is final.
//! - Asking to be told is shown back while the answer is held; stopping
//!   the turn before the answer arrives records nothing (see `tell`).
//!
//! Undo never raises the record: it retracts only a claim. Every line here
//! is built from part titles and your own words, never from a candidate,
//! its key or anything the gate would hold back (A6).

use serde_json::json;

use super::refer;
use super::*;
use crate::domain::{AskChoice, CommitKind, PendingAsk, PendingCommitment, Undoable};
use crate::understanding::NewEvent;

/// Your words in a verdict's show-back line, at most this many characters.
const WORDS_MAX: usize = 80;

/// At most `max` characters of `text`, cut on a character boundary.
pub(super) fn cut(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

impl Runtime {
    /// A claim read from plain words: recorded at once and shown back, and
    /// undoable in the next turn.
    /// `from` is where you were before your words moved you to the part
    /// claimed (undo takes you back there); None when they didn't.
    pub(super) fn claim_shown_back(
        &self,
        session: &mut Session,
        then: Option<&str>,
        from: Option<(Option<String>, SessionMode)>,
    ) -> Result<TurnOutput> {
        let was_open = session
            .active_part()
            .is_some_and(|p| p.status == PartStatus::Open);
        let (prev_active, prev_mode) = from.unwrap_or((session.active.clone(), session.mode));
        let snapshot = Undoable {
            part: session.active.clone().unwrap_or_default(),
            events: Vec::new(),
            turn: session.turn,
            prev_active,
            prev_mode,
            prev_change: session.pending_change.clone(),
            prev_gaps: session.gaps_raised.clone(),
            also: Vec::new(),
        };
        let (out, events) = self.record_known(session, then, i18n::Line::ShowKnown)?;
        let claimed = session
            .map
            .get(&snapshot.part)
            .is_some_and(|p| p.status == PartStatus::Known);
        if was_open && claimed {
            session.last_commitment = Some(Undoable { events, ..snapshot });
        }
        Ok(out)
    }

    /// Claims on several parts your words named at once ("I already know
    /// parts 1 and 2"): recorded together, shown back in one line, and
    /// undone together in the next turn. Parts that aren't open are left
    /// as they are; you move on only when the part you're on was claimed.
    pub(super) fn claim_many(&self, session: &mut Session, ids: &[String]) -> Result<TurnOutput> {
        let open: Vec<String> = ids
            .iter()
            .filter(|id| {
                session
                    .map
                    .get(id)
                    .is_some_and(|p| p.status == PartStatus::Open)
            })
            .cloned()
            .collect();
        let Some((first, rest)) = open.split_first() else {
            let titles: Vec<String> = ids
                .iter()
                .filter_map(|id| session.map.get(id))
                .map(|p| format!("**{}** is already {}.", p.title, p.status.label()))
                .collect();
            return Ok(notice(session, titles.join(" ")));
        };
        let snapshot = Undoable {
            part: first.clone(),
            events: Vec::new(),
            turn: session.turn,
            prev_active: session.active.clone(),
            prev_mode: session.mode,
            prev_change: session.pending_change.clone(),
            prev_gaps: session.gaps_raised.clone(),
            also: rest.to_vec(),
        };
        let mut events = Vec::new();
        for id in &open {
            if session
                .pending_candidate
                .as_ref()
                .is_some_and(|c| c.part_id == *id)
            {
                session.pending_candidate = None;
            }
            self.set_status(session, id, PartStatus::Known, "claimed by the user")?;
            events.extend(self.record_skip(session, id, EventKind::Claim));
        }
        let lang = session.language.clone();
        let titles: Vec<String> = open
            .iter()
            .filter_map(|id| session.map.get(id).map(|p| p.title.clone()))
            .collect();
        let joined = titles.join(&format!(
            "**{}**",
            i18n::line(lang.as_deref(), i18n::Line::And)
        ));
        let mut concepts: Vec<String> = Vec::new();
        for id in &open {
            for c in session
                .map
                .get(id)
                .map(|p| p.concepts.clone())
                .unwrap_or_default()
            {
                if !concepts.contains(&c) {
                    concepts.push(c);
                }
            }
        }
        let lead = self.known_lead(session, &concepts, &joined);
        let moved = session.active.as_ref().is_some_and(|a| open.contains(a));
        let out = if moved {
            self.after_resolution(session, lead)
        } else {
            session.push_turn("assistant", &lead);
            TurnOutput::text(lead)
        };
        session.last_commitment = Some(Undoable { events, ..snapshot });
        Ok(out)
    }

    /// The line a claim is shown back with, worded by what the part
    /// already rests on: a claim adds no evidence, but a concept you had
    /// already shown keeps its evidence, so "a claim, not evidence" is
    /// said only of the concepts that rest on your word alone.
    pub(super) fn known_lead(&self, session: &Session, concepts: &[String], title: &str) -> String {
        let lang = session.language.as_deref();
        let views = if concepts.is_empty() {
            None
        } else {
            self.concept_views(session, concepts).ok()
        };
        let Some(views) = views else {
            return i18n::line(lang, i18n::Line::ShowKnown).replace("{title}", title);
        };
        let labels = self.labels(session);
        let unshown: Vec<String> = concepts
            .iter()
            .filter(|c| !crate::understanding::state_of(&views, c).has_evidence())
            .map(|c| {
                format!(
                    "**{}**",
                    labels.get(c).cloned().unwrap_or_else(|| c.clone())
                )
            })
            .collect();
        if unshown.len() == concepts.len() {
            i18n::line(lang, i18n::Line::ShowKnown).replace("{title}", title)
        } else if unshown.is_empty() {
            i18n::line(lang, i18n::Line::ShowKnownShown).replace("{title}", title)
        } else {
            i18n::line(lang, i18n::Line::ShowKnownSome)
                .replace("{title}", title)
                .replace(
                    "{concepts}",
                    &unshown.join(i18n::line(lang, i18n::Line::And)),
                )
        }
    }

    /// `/undo`: takes back a claim from your last step. A verdict, a
    /// review, a handoff, a delegation and an answer you were told are
    /// never undone: undo only ever lowers the record.
    pub(super) fn undo(&self, session: &mut Session) -> Result<TurnOutput> {
        let lang = session.language.clone();
        if !session.undoable() {
            return Ok(notice(
                session,
                i18n::line(lang.as_deref(), i18n::Line::NothingToUndo).to_string(),
            ));
        }
        let Some(claim) = session.last_commitment.take() else {
            return Ok(TurnOutput::notice(""));
        };
        // Compensating events: the log stays append-only (A4).
        let events = claim
            .events
            .iter()
            .map(|(id, concept)| {
                NewEvent::new(concept, EventKind::Retracted)
                    .reference(id.to_string())
                    .detail(json!({"of": "claim", "by": "undo"}))
            })
            .collect();
        self.append_events(session, events);
        let mut titles = Vec::new();
        for id in std::iter::once(&claim.part).chain(&claim.also) {
            let mut title = id.clone();
            if let Some(part) = session.map.get_mut(id)
                && part.status == PartStatus::Known
            {
                part.status = PartStatus::Open;
                title = part.title.clone();
                self.store.record_outcome(
                    &session.id,
                    id,
                    &title,
                    "claim_withdrawn",
                    PartStatus::Open.label(),
                    Some("claim withdrawn"),
                )?;
            }
            titles.push(title);
        }
        let title = titles.join(&format!(
            "**{}**",
            i18n::line(lang.as_deref(), i18n::Line::And)
        ));
        session.active = claim.prev_active;
        session.mode = claim.prev_mode;
        session.pending_change = claim.prev_change;
        session.gaps_raised = claim.prev_gaps;
        Ok(notice(
            session,
            i18n::line(lang.as_deref(), i18n::Line::Undone).replace("{title}", &title),
        ))
    }

    /// "Let the agent do it" on the part your words name ("let the agent
    /// write the letter"), not just the one you're on: asks which part
    /// when they could mean several. `routed`: the part the Router read,
    /// used when the words name none.
    pub(super) fn agent_step(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        routed: Option<&str>,
    ) -> TurnOutput {
        let Some(active) = session.active.clone() else {
            return notice(session, "No part is active.".into());
        };
        // "Do the rest", "X and Y": every part named, run one after
        // another on one Enter.
        let many = self.agent_parts(session, text);
        if many.len() > 1 {
            return self.propose_agents(session, &many, text, files);
        }
        let clause = refer::agent_clause(text);
        let target = match self.part_named(session, &clause) {
            refer::PartRef::Active => match routed.filter(|r| session.map.get(r).is_some()) {
                Some(id) => refer::PartRef::One(id.to_string()),
                None => refer::PartRef::Active,
            },
            other => other,
        };
        match target {
            // "Parts 1 and 2": both, in order.
            refer::PartRef::All(ids) => self.propose_agents(session, &ids, text, files),
            refer::PartRef::Many(ids) => {
                let ask = PendingAsk {
                    text: text.to_string(),
                    files: files.to_vec(),
                    part: active,
                    choices: ids.into_iter().map(AskChoice::Part).collect(),
                    then: vec![AskChoice::Agent],
                };
                let line = ask_line(session, &ask);
                session.pending_ask = Some(ask);
                notice(session, line)
            }
            refer::PartRef::One(id) => self.propose_agent(session, &id, text, files),
            refer::PartRef::Active => self.propose_agent(session, &active, text, files),
        }
    }

    /// The parts one request for the agent names, in map order, when it
    /// names more than one the agent can take: "let the agent do the rest",
    /// "all remaining parts in one go", "…: the quadrature check and the
    /// L2 distance". Empty otherwise. Code alone.
    pub(super) fn agent_parts(&self, session: &Session, text: &str) -> Vec<String> {
        let takes = |id: &str| {
            session
                .map
                .get(id)
                .is_some_and(|p| matches!(p.status, PartStatus::Open | PartStatus::Understood))
        };
        let at = refer::pointers(&session.map, &session.cwd);
        let labels = self.labels(session);
        let one = |item: &str| match refer::resolve_part(&session.map, None, &at, &labels, item) {
            refer::PartRef::One(id) => vec![id],
            refer::PartRef::All(ids) => ids,
            _ => Vec::new(),
        };
        let mut named: Vec<String> = Vec::new();
        for item in refer::agent_items(text) {
            let mut found = one(&item);
            // "the component count and the L2 distance": each on its own.
            if found.is_empty() && item.contains(" and ") {
                found = item.split(" and ").flat_map(|p| one(p.trim())).collect();
            }
            for id in found {
                if !named.contains(&id) {
                    named.push(id);
                }
            }
        }
        // "…, but not the letter", "the rest except the letter".
        let left_out: Vec<String> = refer::agent_exclusions(text)
            .iter()
            .flat_map(|item| one(item))
            .collect();
        named.retain(|id| takes(id) && !left_out.contains(id));
        if named.len() < 2 && refer::asks_rest(text) {
            named = session
                .map
                .parts
                .iter()
                .filter(|p| takes(&p.id) && !left_out.contains(&p.id))
                .map(|p| p.id.clone())
                .collect();
        }
        if named.len() < 2 {
            return Vec::new();
        }
        session
            .map
            .parts
            .iter()
            .filter(|p| named.contains(&p.id))
            .map(|p| p.id.clone())
            .collect()
    }

    /// Several parts for the agent in one request, shown back together:
    /// each with what it will be (a handoff you review, or a delegation
    /// and why), and one Enter runs them in order (§12, §15).
    pub(super) fn propose_agents(
        &self,
        session: &mut Session,
        ids: &[String],
        text: &str,
        files: &[PathBuf],
    ) -> TurnOutput {
        use i18n::Line;
        let lang = session.language.clone();
        let lang = lang.as_deref();
        let mut queue: Vec<crate::domain::QueuedAgent> = Vec::new();
        let mut shown: Vec<String> = Vec::new();
        for id in ids {
            let Some(part) = session.map.get(id).cloned() else {
                continue;
            };
            let (reviewable, missing) = self.reviewable(session, &part);
            let unchecked = refer::wants_no_review(text) && part.status == PartStatus::Open;
            let kind = if reviewable && !unchecked {
                CommitKind::Handoff
            } else {
                CommitKind::Delegate
            };
            if refusal(&part, kind, reviewable).is_some() {
                continue;
            }
            shown.push(match kind {
                CommitKind::Handoff => {
                    i18n::line(lang, Line::AgentPartHandoff).replace("{title}", &part.title)
                }
                _ if !unchecked && !missing.is_empty() => {
                    i18n::line(lang, Line::AgentPartDelegateWhy)
                        .replace("{title}", &part.title)
                        .replace("{concepts}", &missing.join(", "))
                }
                _ => i18n::line(lang, Line::AgentPartDelegate).replace("{title}", &part.title),
            });
            queue.push(crate::domain::QueuedAgent {
                part: part.id.clone(),
                kind,
            });
        }
        match queue.len() {
            0 => return self.propose_agent(session, &ids[0], text, files),
            1 => return self.propose_agent(session, &queue[0].part, text, files),
            _ => {}
        }
        let first = queue.remove(0);
        let line = i18n::line(lang, Line::ShowAgentParts).replace("{parts}", &shown.join("; "));
        session.pending_commitment = Some(PendingCommitment {
            kind: first.kind,
            part: first.part,
            text: text.to_string(),
            files: files.to_vec(),
            candidate: None,
            then: queue,
        });
        notice(session, line)
    }

    /// Forgets the parts still queued from an earlier request for the agent.
    pub(super) fn drop_queue(session: &mut Session) {
        session.agent_queue.clear();
        session.agent_text.clear();
        session.agent_files.clear();
    }

    /// Runs the parts still queued from one request for the agent, in
    /// order, after `out` (the run before). Stops while a result waits for
    /// your review: the next part starts after your verdict on it.
    pub(super) async fn run_queue(
        &self,
        session: &mut Session,
        mut out: TurnOutput,
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        while !session.agent_queue.is_empty()
            && !session
                .map
                .parts
                .iter()
                .any(|p| p.status == PartStatus::InReview)
        {
            let next = session.agent_queue.remove(0);
            let Some(part) = session.map.get(&next.part) else {
                continue;
            };
            let runs = match next.kind {
                CommitKind::Handoff => {
                    matches!(part.status, PartStatus::Open | PartStatus::Understood)
                }
                _ => part.status == PartStatus::Open,
            };
            if !runs {
                continue;
            }
            // What the run before said stays in view above the next one.
            if !out.reply.trim().is_empty() {
                sink(StreamEvent::TextDelta(format!(
                    "{}\n\n",
                    out.reply.trim_end()
                )));
            }
            let text = queued_instructions(&session.agent_text);
            let files = session.agent_files.clone();
            go_to(session, &next.part);
            out = match next.kind {
                CommitKind::Handoff => {
                    self.handle_handoff(session, &text, &files, sink.clone())
                        .await?
                }
                _ => {
                    self.handle_delegate(session, &text, &files, sink.clone())
                        .await?
                }
            };
        }
        if session.agent_queue.is_empty() {
            session.agent_text.clear();
            session.agent_files.clear();
        }
        Ok(out)
    }

    /// "Let the agent do it": a handoff with your review when the part is
    /// reviewable (§12: understood, or every concept it rests on shown or
    /// solid), else a delegation that says in one line why. "Without me
    /// checking" asks for a delegation outright. Decided by code, never a
    /// model.
    pub(super) fn propose_agent(
        &self,
        session: &mut Session,
        part: &str,
        text: &str,
        files: &[PathBuf],
    ) -> TurnOutput {
        let Some(found) = session.map.get(part).cloned() else {
            return notice(session, "No part is active.".into());
        };
        let (reviewable, missing) = self.reviewable(session, &found);
        let unchecked = refer::wants_no_review(text) && found.status == PartStatus::Open;
        let kind = if reviewable && !unchecked {
            CommitKind::Handoff
        } else {
            CommitKind::Delegate
        };
        let out = self.propose_on(session, kind, &found, text, files);
        if kind == CommitKind::Delegate
            && !unchecked
            && !missing.is_empty()
            && session.pending_commitment.is_some()
        {
            // Why it isn't a handoff, in one line: what you haven't shown.
            let why = i18n::line(session.language.as_deref(), i18n::Line::DelegateWhy)
                .replace("{concepts}", &missing.join(", "));
            let line = format!("{}\n{why}", out.reply);
            if let Some(last) = session.history.last_mut()
                && last.role == "assistant"
            {
                last.content = line.clone();
            }
            return TurnOutput::notice(line);
        }
        out
    }

    /// Whether your review of `part` would count (§12): it is understood,
    /// or it is open and every concept it rests on is shown or solid. Also
    /// the concepts not shown yet, by label.
    pub(super) fn reviewable(
        &self,
        session: &Session,
        part: &crate::domain::Part,
    ) -> (bool, Vec<String>) {
        match part.status {
            PartStatus::Understood => return (true, Vec::new()),
            PartStatus::Open if !part.concepts.is_empty() => {}
            _ => return (false, Vec::new()),
        }
        let Ok(views) = self.concept_views(session, &part.concepts) else {
            return (false, Vec::new());
        };
        let labels = self.labels(session);
        let missing: Vec<String> = part
            .concepts
            .iter()
            .filter(|c| !crate::understanding::state_of(&views, c).has_evidence())
            .map(|c| labels.get(c).cloned().unwrap_or_else(|| c.clone()))
            .collect();
        (missing.is_empty(), missing)
    }

    /// Holds a commitment with side effects and shows it back: Enter runs
    /// it, Esc cancels it. A part that can't take it is refused at once,
    /// as its command would refuse it.
    pub(super) fn propose(
        &self,
        session: &mut Session,
        kind: CommitKind,
        text: &str,
        files: &[PathBuf],
    ) -> TurnOutput {
        let Some(part) = session.active_part().cloned() else {
            return notice(session, "No part is active.".into());
        };
        self.propose_on(session, kind, &part, text, files)
    }

    /// `propose` on `part`, which need not be the one you're on: a handoff
    /// or delegation of another part moves you there only once you
    /// confirm it.
    fn propose_on(
        &self,
        session: &mut Session,
        kind: CommitKind,
        part: &crate::domain::Part,
        text: &str,
        files: &[PathBuf],
    ) -> TurnOutput {
        let lang = session.language.clone();
        let lang = lang.as_deref();
        let part = part.clone();
        let line = match kind {
            CommitKind::Verdict => {
                if session.pending_candidate.is_none() {
                    return notice(
                        session,
                        i18n::line(lang, i18n::Line::NothingWaiting).to_string(),
                    );
                }
                i18n::line(lang, i18n::Line::ShowVerdict).replace("{words}", &cut(text, WORDS_MAX))
            }
            CommitKind::Handoff => {
                let reviewable = self.reviewable(session, &part).0;
                if let Some(refused) = refusal(&part, kind, reviewable) {
                    return notice(session, refused);
                }
                let which = if crate::ledger::repo_root(&session.cwd).is_some() {
                    i18n::Line::ShowHandoff
                } else {
                    i18n::Line::ShowHandoffInPlace
                };
                i18n::line(lang, which).replace("{title}", &part.title)
            }
            CommitKind::Delegate => {
                if let Some(refused) = refusal(&part, kind, false) {
                    return notice(session, refused);
                }
                i18n::line(lang, i18n::Line::ShowDelegate).replace("{title}", &part.title)
            }
            // Your verdict on the agent's result, as `/approve` and
            // `/object` give it: only on a result waiting for review.
            CommitKind::Approve | CommitKind::Object => {
                if part.status != PartStatus::InReview {
                    return notice(
                        session,
                        "Nothing is waiting for your review on this part.".into(),
                    );
                }
                let which = if kind == CommitKind::Approve {
                    i18n::Line::ShowApprove
                } else {
                    i18n::Line::ShowObject
                };
                i18n::line(lang, which)
                    .replace("{title}", &part.title)
                    .replace("{words}", &cut(text, WORDS_MAX))
            }
        };
        let candidate = match kind {
            CommitKind::Verdict => session.pending_candidate.as_ref().map(|c| c.id.clone()),
            _ => None,
        };
        session.pending_commitment = Some(PendingCommitment {
            kind,
            part: part.id,
            text: text.to_string(),
            files: files.to_vec(),
            candidate,
            then: Vec::new(),
        });
        notice(session, line)
    }

    /// `/confirm` (Enter): runs what waits for your OK, if it still
    /// applies, exactly as its command would.
    pub(super) async fn confirm(
        &self,
        session: &mut Session,
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let lang = session.language.clone();
        let Some(p) = session.pending_commitment.take() else {
            return Ok(notice(
                session,
                i18n::line(lang.as_deref(), i18n::Line::NothingWaiting).to_string(),
            ));
        };
        let applies = match p.kind {
            // The very candidate your words were shown back for: a newer
            // one on the same part was never judged by them.
            CommitKind::Verdict => {
                session.active.as_deref() == Some(p.part.as_str())
                    && session
                        .pending_candidate
                        .as_ref()
                        .is_some_and(|c| c.part_id == p.part && Some(&c.id) == p.candidate.as_ref())
            }
            // The part your words named, which you go to now; their
            // commands check its status again.
            CommitKind::Handoff | CommitKind::Delegate => session.map.get(&p.part).is_some(),
            // Still the result you were shown back: in review on that part.
            CommitKind::Approve | CommitKind::Object => session
                .map
                .get(&p.part)
                .is_some_and(|part| part.status == PartStatus::InReview),
        };
        if !applies {
            return Ok(notice(
                session,
                i18n::line(lang.as_deref(), i18n::Line::NotRunStale).to_string(),
            ));
        }
        session.ran_commitment = Some(p.kind);
        // One request for several parts: the rest wait their turn.
        let text = if p.then.is_empty() {
            // A new request for one part replaces what an older one left
            // queued: that queue never runs on a later, unrelated OK.
            if matches!(p.kind, CommitKind::Handoff | CommitKind::Delegate) {
                Self::drop_queue(session);
            }
            p.text.clone()
        } else {
            session.agent_queue = p.then.clone();
            session.agent_text = p.text.clone();
            session.agent_files = p.files.clone();
            queued_instructions(&p.text)
        };
        let out = match p.kind {
            CommitKind::Verdict => {
                forget_show_back(session, &p.text);
                return self.handle_judgment(session, &p.text, &p.files, sink).await;
            }
            CommitKind::Handoff => {
                go_to(session, &p.part);
                self.handle_handoff(session, &text, &p.files, sink.clone())
                    .await?
            }
            CommitKind::Delegate => {
                go_to(session, &p.part);
                self.handle_delegate(session, &text, &p.files, sink.clone())
                    .await?
            }
            CommitKind::Approve => {
                go_to(session, &p.part);
                self.handle_approve(session).await?
            }
            // Catching a planted check finishes the part: the queue goes on.
            CommitKind::Object => {
                go_to(session, &p.part);
                self.handle_object(session, &p.text, &p.files, sink.clone())
                    .await?
            }
        };
        self.run_queue(session, out, sink).await
    }

    /// `/cancel` (Esc): drops what waits for your OK. A verdict you didn't
    /// confirm is kept as a note in your log, so a question still gets its
    /// sources.
    pub(super) async fn cancel(
        &self,
        session: &mut Session,
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let lang = session.language.clone();
        let Some(p) = session.pending_commitment.clone() else {
            return Ok(notice(
                session,
                i18n::line(lang.as_deref(), i18n::Line::NothingWaiting).to_string(),
            ));
        };
        session.pending_commitment = None;
        let line = not_run(session, &p);
        if p.kind != CommitKind::Verdict {
            return Ok(notice(session, line));
        }
        if session.active.as_deref() != Some(p.part.as_str()) {
            return Ok(notice(
                session,
                i18n::line(lang.as_deref(), i18n::Line::NotRunStale).to_string(),
            ));
        }
        forget_show_back(session, &p.text);
        sink(StreamEvent::TextDelta(format!("{line}\n\n")));
        let before = session.history.len();
        // A note in your log: the Tutor answers it as it would any message.
        let out = self
            .handle_log(session, &p.text, &p.files, sink, Some("The user"))
            .await?;
        if let Some(reply) = session
            .history
            .iter_mut()
            .skip(before)
            .find(|t| t.role == "assistant")
        {
            reply.content = format!("{line}\n\n{}", reply.content);
        }
        Ok(out)
    }

    /// Drops what waits for your OK because you went on with another
    /// message, and says what didn't run. A verdict you didn't confirm stays
    /// in your log as a note, as the line says.
    pub(super) fn drop_pending(&self, session: &mut Session) -> Option<String> {
        let p = session.pending_commitment.take()?;
        if p.kind == CommitKind::Verdict
            && session.active.as_deref() == Some(p.part.as_str())
            && let Some(part) = session.active_part_mut()
        {
            part.log.push(p.text.clone());
        }
        Some(not_run(session, &p))
    }

    /// Which part `text` means (§15): a source, a page, a name, or the
    /// active part. Code alone.
    pub(super) fn part_named(&self, session: &Session, text: &str) -> refer::PartRef {
        refer::resolve_part(
            &session.map,
            session.active.as_deref(),
            &refer::pointers(&session.map, &session.cwd),
            &self.labels(session),
            text,
        )
    }

    /// The Router leaned toward a commitment but wasn't sure, or your words
    /// read like one it missed ("I understand this part"): ask which you
    /// meant, in plain words (§15, A3). Nothing is recorded and no model is
    /// called. None when there is nothing to ask: the message is a note.
    pub(super) fn ask_which(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        top: &[&str],
    ) -> Option<TurnOutput> {
        let active = session.active.clone()?;
        let wanted = wanted_choices(top);
        // Asking only about the agent: the part is read from the words
        // that ask for it ("… let the agent write the letter").
        let named = if top == [AGENT] {
            refer::agent_clause(text)
        } else {
            text.to_string()
        };
        let (part, choices, then) = match self.part_named(session, &named) {
            refer::PartRef::Many(ids) | refer::PartRef::All(ids) => {
                let choices = ids.into_iter().map(AskChoice::Part).collect();
                (active, choices, wanted)
            }
            refer::PartRef::One(id) => {
                let choices = offered(session, &id, &wanted);
                (id, choices, Vec::new())
            }
            refer::PartRef::Active => {
                let choices = offered(session, &active, &wanted);
                (active, choices, Vec::new())
            }
        };
        if choices.iter().all(|c| *c == AskChoice::Note) {
            return None;
        }
        session.push_turn("user", text);
        let ask = PendingAsk {
            text: text.to_string(),
            files: files.to_vec(),
            part,
            choices,
            then,
        };
        let line = ask_line(session, &ask);
        session.pending_ask = Some(ask);
        Some(notice(session, line))
    }

    /// A claim on a part your words named, not the one you're on: asks
    /// which part when they could mean several, else claims that one (and
    /// undo brings you back). None when the claim goes on the active part.
    pub(super) fn claim_target(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        then: Option<&str>,
    ) -> Result<Option<TurnOutput>> {
        Ok(match self.part_named(session, text) {
            refer::PartRef::Many(ids) => {
                session.push_turn("user", text);
                let ask = PendingAsk {
                    text: text.to_string(),
                    files: files.to_vec(),
                    part: session.active.clone().unwrap_or_default(),
                    choices: ids.into_iter().map(AskChoice::Part).collect(),
                    then: vec![AskChoice::Known],
                };
                let line = ask_line(session, &ask);
                session.pending_ask = Some(ask);
                Some(notice(session, line))
            }
            refer::PartRef::All(ids) => {
                session.push_turn("user", text);
                Some(self.claim_many(session, &ids)?)
            }
            refer::PartRef::One(id) if session.active.as_deref() == Some(id.as_str()) => None,
            refer::PartRef::One(id) => {
                session.push_turn("user", text);
                let Some(part) = session.map.get(&id) else {
                    return Ok(None);
                };
                if part.status != PartStatus::Open {
                    // Nothing to claim there, and you stay where you are.
                    let line = format!("**{}** is already {}.", part.title, part.status.label());
                    return Ok(Some(notice(session, line)));
                }
                let from = (session.active.clone(), session.mode);
                go_to(session, &id);
                Some(self.claim_shown_back(session, then, Some(from))?)
            }
            refer::PartRef::Active => None,
        })
    }

    /// Your answer to a question back to you, if this message is one: the
    /// step you picked runs. None when it isn't an answer; the question is
    /// dropped and the message goes on as any other.
    pub(super) async fn answer_ask(
        &self,
        session: &mut Session,
        ask: PendingAsk,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<Option<TurnOutput>> {
        let Some(choice) = self.picked(session, &ask, text) else {
            return Ok(None);
        };
        let mut files = files.to_vec();
        files.extend(ask.files.iter().cloned());
        let part = match &choice {
            AskChoice::Part(id) => id.clone(),
            _ => ask.part.clone(),
        };
        if session.map.get(&part).is_none() {
            return Ok(None);
        }
        let choice = match choice {
            AskChoice::Part(_) => {
                let then = offered(session, &part, &ask.then);
                match then.as_slice() {
                    // One thing to do there: do it.
                    [one] => one.clone(),
                    _ => {
                        session.push_turn("user", text);
                        let next = PendingAsk {
                            part,
                            choices: then,
                            then: Vec::new(),
                            ..ask
                        };
                        let line = ask_line(session, &next);
                        session.pending_ask = Some(next);
                        return Ok(Some(notice(session, line)));
                    }
                }
            }
            other => other,
        };
        let from = (session.active.clone(), session.mode);
        go_to(session, &part);
        Ok(Some(match choice {
            AskChoice::Note => {
                // Your message goes on the log as if never asked about.
                forget_show_back(session, &ask.text);
                self.handle_log(session, &ask.text, &files, sink, Some("The user"))
                    .await?
            }
            AskChoice::Known => {
                session.push_turn("user", text);
                self.claim_shown_back(session, None, Some(from))?
            }
            AskChoice::Check => {
                session.push_turn("user", text);
                if let Some(p) = session.active_part_mut() {
                    p.log.push(ask.text.clone());
                }
                let visible = self.write_candidate(session, &part, &files, sink).await?;
                TurnOutput::text(visible)
            }
            AskChoice::Tell => {
                session.push_turn("user", text);
                self.tell(session, &ask.text, &files, sink, true).await?
            }
            AskChoice::Agent => {
                session.push_turn("user", text);
                self.propose_agent(session, &part, &ask.text, &files)
            }
            AskChoice::Part(_) => unreachable!("a part was resolved above"),
        }))
    }

    /// Which offered choice `text` picks: its number, or plain words that
    /// fit exactly one of them. A long message is something else.
    fn picked(&self, session: &Session, ask: &PendingAsk, text: &str) -> Option<AskChoice> {
        let t = text
            .trim()
            .trim_end_matches(['.', '!', ')'])
            .trim_start_matches('(')
            .trim();
        if let Ok(n) = t.parse::<usize>() {
            return ask.choices.get(n.checked_sub(1)?).cloned();
        }
        if t.split_whitespace().count() > 10 {
            return None;
        }
        let low = t.to_lowercase().replace(['’', '‘'], "'");
        // Whole words only, so "record" doesn't match inside "recorder".
        let padded = format!(
            " {} ",
            low.split(|c: char| !c.is_alphanumeric() && c != '\'')
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        );
        let has = |words: &[&str]| words.iter().any(|w| padded.contains(&format!(" {w} ")));
        // "I don't know", "don't record it", "weiß nicht": a doubt or a no
        // never picks a step that records or acts (§15: the record is what
        // you said). Only "just a note" survives it.
        let doubt = refer::hedged(&low);
        // "No check" picks the claim, not the check.
        let unchecked = has(&[
            "no check",
            "without check",
            "without a check",
            "ohne prüfung",
        ]);
        let fits: Vec<&AskChoice> = ask
            .choices
            .iter()
            .filter(|c| match c {
                AskChoice::Note => {
                    has(&[
                        "note", "just a note", "neither", "nothing", "none", "just go on",
                        "notiz", "keins", "nichts",
                    ]) || matches!(low.as_str(), "no" | "nope" | "nein")
                }
                _ if doubt => false,
                AskChoice::Known => has(&[
                    "record", "know", "known", "already", "no check", "claim", "weiß",
                    "kenne",
                ]),
                AskChoice::Check => {
                    !unchecked
                        && has(&[
                            "check", "test", "quiz", "ask me", "prüf", "prüfe", "prüfen",
                            "frag mich",
                        ])
                }
                AskChoice::Tell => has(&["tell", "answer", "sag", "sage"]),
                AskChoice::Agent => has(&["agent", "delegate", "hand it", "do it"]),
                AskChoice::Part(id) => {
                    let title = session.map.get(id).map(|p| p.title.to_lowercase());
                    title.is_some_and(|t| low.contains(&t))
                        || matches!(self.part_named(session, text), refer::PartRef::One(ref r) if r == id)
                }
            })
            .collect();
        match fits.as_slice() {
            [one] => Some((*one).clone()),
            _ => None,
        }
    }
}

/// What didn't run, in one line.
fn not_run(session: &Session, p: &PendingCommitment) -> String {
    let title = session
        .map
        .get(&p.part)
        .map(|p| p.title.clone())
        .unwrap_or_else(|| p.part.clone());
    let which = match p.kind {
        CommitKind::Verdict => i18n::Line::NotRunVerdict,
        CommitKind::Handoff => i18n::Line::NotRunHandoff,
        CommitKind::Delegate => i18n::Line::NotRunDelegate,
        CommitKind::Approve => i18n::Line::NotRunApprove,
        CommitKind::Object => i18n::Line::NotRunObject,
    };
    i18n::line(session.language.as_deref(), which).replace("{title}", &title)
}

/// A fixed line, kept in the thread and shown as a notice.
fn notice(session: &mut Session, line: String) -> TurnOutput {
    session.push_turn("assistant", &line);
    TurnOutput::notice(line)
}

/// The choices a question about a commitment offers, in plain words: a
/// claim comes with "check me on it" beside it, and "just a note" always
/// comes last (§15).
fn wanted_choices(top: &[&str]) -> Vec<AskChoice> {
    let mut out = Vec::new();
    for c in top {
        let add: &[AskChoice] = match *c {
            "known" => &[AskChoice::Known, AskChoice::Check],
            TELL => &[AskChoice::Tell],
            AGENT => &[AskChoice::Agent],
            _ => &[],
        };
        for a in add {
            if !out.contains(a) {
                out.push(a.clone());
            }
        }
    }
    out.push(AskChoice::Note);
    out
}

/// The wanted choices that `part` can take: a claim or a check only on an
/// open part.
fn offered(session: &Session, part: &str, wanted: &[AskChoice]) -> Vec<AskChoice> {
    let open = session
        .map
        .get(part)
        .is_some_and(|p| p.status == PartStatus::Open);
    wanted
        .iter()
        .filter(|c| open || !matches!(c, AskChoice::Known | AskChoice::Check))
        .cloned()
        .collect()
}

/// The question, in one line: numbered choices in plain words.
fn ask_line(session: &Session, ask: &PendingAsk) -> String {
    use i18n::Line;
    let lang = session.language.as_deref();
    let title = |id: &str| {
        session
            .map
            .get(id)
            .map(|p| p.title.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let here = title(&ask.part);
    let items: Vec<String> = ask
        .choices
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let words = match c {
                AskChoice::Known => i18n::line(lang, Line::AskKnown).replace("{title}", &here),
                AskChoice::Check => i18n::line(lang, Line::AskCheck).replace("{title}", &here),
                AskChoice::Note => i18n::line(lang, Line::AskNote).to_string(),
                AskChoice::Tell => i18n::line(lang, Line::AskTell).to_string(),
                // Another part than the one you're on is named, so you know
                // which one the agent would do.
                AskChoice::Agent if session.active.as_deref() != Some(ask.part.as_str()) => {
                    i18n::line(lang, Line::AskAgentPart).replace("{title}", &here)
                }
                AskChoice::Agent => i18n::line(lang, Line::AskAgent).to_string(),
                AskChoice::Part(id) => format!("**{}**", title(id)),
            };
            format!("({}) {words}", i + 1)
        })
        .collect();
    let joined = match items.split_last() {
        Some((last, rest)) if !rest.is_empty() => {
            format!("{}{}{last}", rest.join(", "), i18n::line(lang, Line::Or))
        }
        _ => items.join(""),
    };
    let which = if ask.choices.iter().all(|c| matches!(c, AskChoice::Part(_))) {
        Line::AskWhichPart
    } else {
        Line::AskWhich
    };
    i18n::line(lang, which).replace("{choices}", &joined)
}

/// Your request for the agent, as each of the parts it named gets it: the
/// agent does one part per run, and the others follow.
fn queued_instructions(text: &str) -> String {
    format!(
        "{text}\n\n(This request hands several parts to the agent, one run each, in order. Do only this part now; the others follow in their own runs.)"
    )
}

/// Makes `id` the part you're on, as `/focus` would, without a reply.
fn go_to(session: &mut Session, id: &str) {
    if session.active.as_deref() != Some(id) && session.map.get(id).is_some() {
        session.active = Some(id.to_string());
        session.mode = SessionMode::Working;
        session.pending_candidate = None;
    }
}

/// Why `/handoff` or `/delegate` would refuse this part, in its words.
/// `reviewable`: an open part whose every concept is shown or solid can be
/// handed off as if understood (§12).
fn refusal(part: &crate::domain::Part, kind: CommitKind, reviewable: bool) -> Option<String> {
    match (kind, part.status) {
        (CommitKind::Handoff, PartStatus::Understood) => None,
        (CommitKind::Handoff, PartStatus::Open) if reviewable => None,
        (CommitKind::Delegate, PartStatus::Open) => None,
        (CommitKind::Handoff, PartStatus::Open) => Some(
            "Handoff comes after understanding: log, `/summary`, then judge the candidate. To hand it off without understanding, `/delegate`.".into(),
        ),
        (CommitKind::Delegate, PartStatus::Understood) => {
            Some("You understood this part, so `/handoff` it: you'll review the result.".into())
        }
        (_, other) => Some(format!(
            "**{}** is already {}.",
            part.title,
            other.label()
        )),
    }
}

/// Takes a verdict's show-back (your message and the line after it) out of
/// the thread before the message is judged or logged, so it isn't there
/// twice.
fn forget_show_back(session: &mut Session, text: &str) {
    let n = session.history.len();
    if n >= 2
        && session.history[n - 1].role == "assistant"
        && session.history[n - 2].role == "user"
        && session.history[n - 2].content == text
    {
        session.history.truncate(n - 2);
    }
}

#[cfg(test)]
mod tests {
    use super::cut;

    #[test]
    fn words_are_cut_on_a_char_boundary() {
        assert_eq!(cut("  short  ", 80), "short");
        assert_eq!(cut("äöüß", 2), "äö…");
        assert_eq!(cut(&"x".repeat(81), 80), format!("{}…", "x".repeat(80)));
    }
}
