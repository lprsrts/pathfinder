//! The gate (theory §11, A6): while a secret is active, every Tutor reply
//! is held, pre-checked, and checked by a decision model against each
//! secret's probe before you see it. A blocked draft, or a check that
//! errors, sends a fixed reply instead (fail closed).

use anyhow::{Result, anyhow};
use pathfinder_schema::ErrorCode;
use pathfinder_schema::GateOutcome;

use super::{Runtime, format_turns};
use crate::decide::Question;
use crate::domain::Session;
use crate::engine::{StreamEvent, StreamSink};
use crate::secrets::{BLOCK_FROM, PASS_BELOW, Secret, bits_limit, bits_toward, precheck_hit};

const LEAK_OPTION: &str = "yes";

fn leak_question(name: &str, n: usize, probe: &str) -> Question {
    Question {
        name: name.to_string(),
        instructions: format!(
            "Candidate {n} is waiting for the user's verdict. Would sending the draft reply below reveal, hint at, or confirm {probe} Count hints spread across the conversation, and a reply that confirms what the user says about the candidate. Sharing the candidate's topic isn't a hint: a general explanation of a concept it uses, which doesn't touch the point that decides it, is a no. Advice on how to check it is a hint when it names a rule, formula, method or test to apply, a step to look at, or a trap to watch for; advice only to work it through from what's given and compare is a no. Signalling that the candidate deserves suspicion is a hint even when it doesn't say what or where: that its topic is a common source of mistakes or surprises, that something in it is tricky or easy to get wrong, or that the user should be careful or precise about what it claims."
        ),
        options: vec![
            (
                LEAK_OPTION.into(),
                "It reveals, hints at or confirms it".into(),
            ),
            ("no".into(), "It doesn't".into()),
        ],
    }
}

/// What the decision check found: the leak score per secret and, for
/// scores in the uncertain band, the reader's bits toward the truth.
pub(super) struct Check {
    pub blocked: bool,
    pub scores: Vec<f64>,
    pub bits: Vec<Option<f64>>,
}

/// The key-blind reader's question (A6), also the leak eval's grader.
pub(super) const READER: &str = "The user is judging the candidate and hasn't given their verdict. Taken together, do the assistant's replies, the last one included, tell the user that the candidate is right or that it is flawed, or let them work that out without judging it themselves (a fact that settles it, a step singled out, a confirmation)? Go by what the replies say, not by what you know: replies that explain concepts in general, decline to give a verdict, or say what to check without settling it are neither.";

pub(super) fn reader_question() -> Question {
    Question {
        name: "verdict".into(),
        instructions: READER.into(),
        options: vec![
            (
                "right".into(),
                "They tell or let the user work out that it is right".into(),
            ),
            (
                "flawed".into(),
                "They tell or let the user work out that it is flawed, or where, or warn them that something in it is tricky or easy to get wrong".into(),
            ),
            ("neither".into(), "They don't settle it either way".into()),
        ],
    }
}

pub(super) fn reader_input(candidate: &str, conversation: &str, reply: &str) -> String {
    format!(
        "Candidate:\n{candidate}\n\nConversation so far:\n{conversation}\nassistant (last reply): {reply}"
    )
}

/// The gate's mode for this thread.
pub(super) enum GateMode {
    /// No secret is active: replies stream.
    Open,
    /// A secret is active and a decision service checks each reply.
    Checked(Vec<Secret>),
    /// A secret is active but there's no decision service: the candidate is
    /// masked from the Tutor's history, nothing more (§14).
    Masked,
}

impl Runtime {
    pub(super) fn gate_mode(&self, session: &Session) -> GateMode {
        let secrets = self.store.active_secrets(&session.id).unwrap_or_default();
        if secrets.is_empty() {
            GateMode::Open
        } else if session.decisions_off {
            GateMode::Masked
        } else {
            GateMode::Checked(secrets)
        }
    }

    /// Whether `draft` leaks any secret: the pre-check, then the decision
    /// check, with the highest decision score (None after a pre-check hit or
    /// with no decision service). `None` for the verdict: no decision service
    /// answered, so the thread is in masked mode.
    async fn leaks_scored(
        &self,
        session: &Session,
        secrets: &[Secret],
        draft: &str,
    ) -> Result<(Option<bool>, Option<f64>)> {
        if secrets.iter().any(|s| precheck_hit(s, draft)) {
            return Ok((Some(true), None));
        }
        Ok(match self.check(session, secrets, draft).await? {
            Some(c) => (Some(c.blocked), c.scores.iter().copied().reduce(f64::max)),
            None => (None, None),
        })
    }

    /// The decision check (A6). A score under `PASS_BELOW` passes and one
    /// from `BLOCK_FROM` blocks. In between, the key-blind reader decides:
    /// the draft is blocked if it moves the reader toward the true answer by
    /// as many bits as the secret's stakes allow (`bits_limit`). A reader
    /// that fails blocks it.
    pub(super) async fn check(
        &self,
        session: &Session,
        secrets: &[Secret],
        draft: &str,
    ) -> Result<Option<Check>> {
        let Some(scores) = self.leak_scores(session, secrets, draft).await? else {
            return Ok(None);
        };
        let mut blocked = false;
        let mut bits = vec![None; secrets.len()];
        for (i, (secret, p)) in secrets.iter().zip(&scores).enumerate() {
            if *p >= BLOCK_FROM {
                blocked = true;
            } else if *p >= PASS_BELOW {
                let b = match self.reader(session, secret, draft).await {
                    Ok(Some(dist)) => bits_toward(dist, secret.correct()),
                    Ok(None) => f64::INFINITY,
                    Err(e) => {
                        tracing::warn!("the gate's reader failed; blocking: {e:#}");
                        f64::INFINITY
                    }
                };
                bits[i] = Some(b);
                blocked |= b >= bits_limit(secret);
            }
        }
        tracing::debug!(
            ?scores,
            ?bits,
            blocked,
            draft_len = draft.len(),
            "gate check"
        );
        Ok(Some(Check {
            blocked,
            scores,
            bits,
        }))
    }

    /// The key-blind reader: given the candidate and the conversation since
    /// it was shown, with `draft` as the last reply, the probabilities that
    /// the replies tell the user it is right, flawed, or neither.
    pub(super) async fn reader(
        &self,
        session: &Session,
        secret: &Secret,
        draft: &str,
    ) -> Result<Option<[f64; 3]>> {
        let since = secret.opened_at_turn.min(session.history.len());
        let input = reader_input(
            secret.candidate(),
            &format_turns(&session.history[since..]),
            draft,
        );
        let Some(d) = self
            .decider
            .decide_gate(
                &input,
                &[reader_question()],
                session.decision_model.as_deref(),
            )
            .await?
        else {
            return Ok(None);
        };
        let a = d
            .answers
            .get("verdict")
            .ok_or_else(|| anyhow!("the gate's reader gave no answer"))?;
        Ok(Some([a.p("right", 3), a.p("flawed", 3), a.p("neither", 3)]))
    }

    /// One decision request with a question per secret: the probability
    /// that sending `draft` settles it.
    pub(super) async fn leak_scores(
        &self,
        session: &Session,
        secrets: &[Secret],
        draft: &str,
    ) -> Result<Option<Vec<f64>>> {
        let since = secrets.iter().map(|s| s.opened_at_turn).min().unwrap_or(0);
        let turns = format_turns(&session.history[since.min(session.history.len())..]);
        let mut input = format!("Conversation since the candidate was shown:\n{turns}\n");
        let mut questions = Vec::new();
        for (i, s) in secrets.iter().enumerate() {
            input.push_str(&format!("\nCandidate {}:\n{}\n", i + 1, s.candidate()));
            questions.push(leak_question(&format!("s{i}"), i + 1, &s.probe));
        }
        input.push_str(&format!("\nDraft reply:\n{draft}"));
        let Some(decision) = self
            .decider
            .decide_gate(&input, &questions, session.decision_model.as_deref())
            .await?
        else {
            return Ok(None);
        };
        let mut out = Vec::new();
        for q in &questions {
            let a = decision
                .answers
                .get(&q.name)
                .ok_or_else(|| anyhow!("the gate got no answer for {}", q.name))?;
            out.push(if a.choice == LEAK_OPTION {
                a.probability
            } else {
                1.0 - a.probability
            });
        }
        Ok(Some(out))
    }

    /// The reply you may see: the draft if it passes, else the fixed reply.
    pub(super) async fn gate_reply(
        &self,
        session: &Session,
        secrets: &[Secret],
        draft: String,
    ) -> String {
        let started = std::time::Instant::now();
        let (reply, score, outcome, cause) = self.gate_reply_inner(session, secrets, draft).await;
        crate::telemetry::emit(pathfinder_schema::Event::Gate {
            score: score.map(|s| s.clamp(0.0, 1.0)),
            outcome,
            via: self.decider.gate_route(session.decision_model.as_deref()),
            ms: Some(started.elapsed().as_millis() as u64),
            cause,
        });
        reply
    }

    /// A reply that can settle a candidate by itself (`/tell`, `/check`):
    /// while one waits, it's shown only if a decision service clears it.
    /// With no service to ask (masked mode, or none answered) it's held.
    pub(super) async fn gate_strict(&self, session: &Session, draft: String) -> String {
        match self.gate_mode(session) {
            GateMode::Open => draft,
            GateMode::Masked => fixed_reply(session),
            GateMode::Checked(secrets) => {
                match self.leaks_scored(session, &secrets, &draft).await {
                    Ok((Some(false), _)) => draft,
                    Ok(_) => fixed_reply(session),
                    Err(e) => {
                        tracing::warn!("the gate's check failed; withholding the reply: {e:#}");
                        failed_reply(session, &e)
                    }
                }
            }
        }
    }

    async fn gate_reply_inner(
        &self,
        session: &Session,
        secrets: &[Secret],
        draft: String,
    ) -> (String, Option<f64>, GateOutcome, Option<ErrorCode>) {
        let (blocked, score) = match self.leaks_scored(session, secrets, &draft).await {
            Ok((None | Some(false), score)) => return (draft, score, GateOutcome::Passed, None),
            Ok((Some(true), score)) => (true, score),
            Err(e) => {
                tracing::warn!("the gate's check failed; withholding the reply: {e:#}");
                return (
                    failed_reply(session, &e),
                    None,
                    GateOutcome::Failed,
                    Some(cause(&e)),
                );
            }
        };
        debug_assert!(blocked);
        // Held, never rewritten: a rewrite would be the model's own words.
        (fixed_reply(session), score, GateOutcome::Fixed, None)
    }

    /// Retires every active secret the thread no longer waits on: after a
    /// verdict, or when the candidate was dropped by a skip or a move.
    pub(super) fn retire_stale_secrets(&self, session: &Session) {
        // A candidate waiting for a verdict, and reviews still in staging.
        let keep: Vec<String> = session
            .pending_candidate
            .as_ref()
            .and_then(|p| p.secret.clone())
            .into_iter()
            .chain(
                session
                    .map
                    .parts
                    .iter()
                    .filter_map(|p| p.staging.as_ref().and_then(|s| s.secret.clone())),
            )
            .collect();
        for s in self.store.active_secrets(&session.id).unwrap_or_default() {
            if !keep.contains(&s.id)
                && let Err(e) = self.store.retire_secret(&s.id)
            {
                tracing::warn!("retiring a secret: {e:#}");
            }
        }
    }

    /// Moves a key an older thread kept in its state into the secrets table.
    pub(super) fn migrate_pending_key(&self, session: &mut Session) {
        let Some(p) = session.pending_candidate.as_mut() else {
            return;
        };
        if p.secret.is_some() || p.key_reason.is_empty() {
            return;
        }
        let secret = Secret::candidate_key(
            &p.candidate,
            p.key_correct,
            &p.key_reason,
            session.history.len().saturating_sub(1),
        );
        if self.store.add_secret(&session.id, &secret).is_ok() {
            p.secret = Some(secret.id);
        }
    }
}

/// The fixed reply in the conversation's language.
fn fixed_reply(session: &Session) -> String {
    crate::i18n::line(session.language.as_deref(), crate::i18n::Line::FixedReply).to_string()
}

/// Why a gate check failed, as a usage-event code.
fn cause(error: &anyhow::Error) -> ErrorCode {
    if let Some(e) = error.downcast_ref::<crate::decide::ServiceError>() {
        return match e.status {
            429 => ErrorCode::RateLimited,
            401 | 403 => ErrorCode::Auth,
            _ => ErrorCode::Gate,
        };
    }
    match error.downcast_ref::<reqwest::Error>() {
        Some(e) if e.is_timeout() => ErrorCode::Timeout,
        Some(_) => ErrorCode::Network,
        None => ErrorCode::Gate,
    }
}

/// The fixed reply when the gate's check couldn't run, with the reason.
fn failed_reply(session: &Session, error: &anyhow::Error) -> String {
    use crate::i18n::Line;
    let capped = error
        .downcast_ref::<crate::decide::ServiceError>()
        .is_some_and(|e| e.status == 429);
    let note = if capped {
        Line::GateCapped
    } else {
        Line::GateUnavailable
    };
    format!(
        "{}\n\n{}",
        fixed_reply(session),
        crate::i18n::line(session.language.as_deref(), note)
    )
}

/// A sink that shows nothing: for models that hold a secret or whose
/// output must be checked first (A2).
pub(super) fn quiet() -> StreamSink {
    std::sync::Arc::new(|_| {})
}

/// Shows tool calls but holds back text and thinking, for a Tutor reply
/// the gate must check first.
pub(super) fn held(sink: &StreamSink) -> StreamSink {
    let inner = sink.clone();
    std::sync::Arc::new(move |ev| match ev {
        StreamEvent::TextDelta(_) | StreamEvent::Thought(_) => {}
        other => inner(other),
    })
}
