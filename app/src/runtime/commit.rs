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

use super::*;
use crate::domain::{CommitKind, PendingCommitment, Undoable};
use crate::understanding::NewEvent;

/// Your words in a verdict's show-back line, at most this many characters.
const WORDS_MAX: usize = 80;

/// At most `max` characters of `text`, cut on a character boundary.
fn cut(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

impl Runtime {
    /// A claim read from plain words: recorded at once and shown back, and
    /// undoable in the next turn.
    pub(super) fn claim_shown_back(
        &self,
        session: &mut Session,
        then: Option<&str>,
    ) -> Result<TurnOutput> {
        let was_open = session
            .active_part()
            .is_some_and(|p| p.status == PartStatus::Open);
        let snapshot = Undoable {
            part: session.active.clone().unwrap_or_default(),
            events: Vec::new(),
            turn: session.turn,
            prev_active: session.active.clone(),
            prev_mode: session.mode,
            prev_change: session.pending_change.clone(),
            prev_gaps: session.gaps_raised.clone(),
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
        let mut title = claim.part.clone();
        if let Some(part) = session.map.get_mut(&claim.part)
            && part.status == PartStatus::Known
        {
            part.status = PartStatus::Open;
            title = part.title.clone();
            self.store.record_outcome(
                &session.id,
                &claim.part,
                &title,
                "claim_withdrawn",
                PartStatus::Open.label(),
                Some("claim withdrawn"),
            )?;
        }
        session.active = claim.prev_active;
        session.mode = claim.prev_mode;
        session.pending_change = claim.prev_change;
        session.gaps_raised = claim.prev_gaps;
        Ok(notice(
            session,
            i18n::line(lang.as_deref(), i18n::Line::Undone).replace("{title}", &title),
        ))
    }

    /// "Let the agent do it": a handoff for an understood part, a
    /// delegation for an open one, decided by code from its status.
    pub(super) fn propose_agent(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
    ) -> TurnOutput {
        let kind = match session.active_part().map(|p| p.status) {
            Some(PartStatus::Understood) => CommitKind::Handoff,
            _ => CommitKind::Delegate,
        };
        self.propose(session, kind, text, files)
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
        let lang = session.language.clone();
        let lang = lang.as_deref();
        let Some(part) = session.active_part().cloned() else {
            return notice(session, "No part is active.".into());
        };
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
                if let Some(refused) = refusal(&part, kind) {
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
                if let Some(refused) = refusal(&part, kind) {
                    return notice(session, refused);
                }
                i18n::line(lang, i18n::Line::ShowDelegate).replace("{title}", &part.title)
            }
        };
        let candidate = match kind {
            CommitKind::Verdict => session.pending_candidate.as_ref().map(|c| c.id.clone()),
            CommitKind::Handoff | CommitKind::Delegate => None,
        };
        session.pending_commitment = Some(PendingCommitment {
            kind,
            part: part.id,
            text: text.to_string(),
            files: files.to_vec(),
            candidate,
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
        let applies = session.active.as_deref() == Some(p.part.as_str())
            && match p.kind {
                // The very candidate your words were shown back for: a
                // newer one on the same part was never judged by them.
                CommitKind::Verdict => session
                    .pending_candidate
                    .as_ref()
                    .is_some_and(|c| c.part_id == p.part && Some(&c.id) == p.candidate.as_ref()),
                // Their commands check the part's status again.
                CommitKind::Handoff | CommitKind::Delegate => true,
            };
        if !applies {
            return Ok(notice(
                session,
                i18n::line(lang.as_deref(), i18n::Line::NotRunStale).to_string(),
            ));
        }
        session.ran_commitment = Some(p.kind);
        match p.kind {
            CommitKind::Verdict => {
                forget_show_back(session, &p.text);
                self.handle_judgment(session, &p.text, &p.files, sink).await
            }
            CommitKind::Handoff => self.handle_handoff(session, &p.text, &p.files, sink).await,
            CommitKind::Delegate => self.handle_delegate(session, &p.text, &p.files, sink).await,
        }
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

    /// The Router leaned toward a commitment but wasn't sure: ask which was
    /// meant, with the commands that say it outright. Nothing is recorded
    /// and no model is called.
    pub(super) fn ask_which(&self, session: &mut Session, top: &[&str]) -> TurnOutput {
        use i18n::Line;
        let lang = session.language.clone();
        let lang = lang.as_deref();
        let title = session
            .active_part()
            .map(|p| p.title.clone())
            .unwrap_or_default();
        let choices: Vec<String> = top
            .iter()
            .filter_map(|c| match *c {
                "known" => Some(i18n::line(lang, Line::AskKnown).replace("{title}", &title)),
                TELL => Some(i18n::line(lang, Line::AskTell).to_string()),
                AGENT => Some(i18n::line(lang, Line::AskAgent).to_string()),
                _ => None,
            })
            .collect();
        let line = i18n::line(lang, Line::AskWhich)
            .replace("{choices}", &choices.join(i18n::line(lang, Line::Or)));
        notice(session, line)
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
    };
    i18n::line(session.language.as_deref(), which).replace("{title}", &title)
}

/// A fixed line, kept in the thread and shown as a notice.
fn notice(session: &mut Session, line: String) -> TurnOutput {
    session.push_turn("assistant", &line);
    TurnOutput::notice(line)
}

/// Why `/handoff` or `/delegate` would refuse this part, in its words.
fn refusal(part: &crate::domain::Part, kind: CommitKind) -> Option<String> {
    match (kind, part.status) {
        (CommitKind::Handoff, PartStatus::Understood) => None,
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
