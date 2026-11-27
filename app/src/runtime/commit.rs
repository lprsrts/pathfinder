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
        let (part, choices, then) = match self.part_named(session, text) {
            refer::PartRef::Many(ids) => {
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
                self.propose_agent(session, &ask.text, &files)
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

/// Makes `id` the part you're on, as `/focus` would, without a reply.
fn go_to(session: &mut Session, id: &str) {
    if session.active.as_deref() != Some(id) && session.map.get(id).is_some() {
        session.active = Some(id.to_string());
        session.mode = SessionMode::Working;
        session.pending_candidate = None;
    }
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
