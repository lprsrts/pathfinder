//! The gate (theory §11, A6): while a secret is active, every Tutor reply
//! is held, pre-checked, and checked by a decision model against each
//! secret's probe before you see it. A blocked draft is rewritten once
//! without the probe or the secret; a second block, or a check that
//! errors, sends a fixed reply instead (fail closed).

use anyhow::{Result, anyhow};
use serde::Deserialize;

use super::{Runtime, format_turns};
use crate::decide::Question;
use crate::domain::Session;
use crate::engine::{EngineRequest, EngineTier, StreamEvent, StreamSink, ToolAccess, run_gated};
use crate::secrets::{
    BLOCK_FROM, FIXED_REPLY, PASS_BELOW, Secret, bits_limit, bits_toward, precheck_hit,
};
use crate::validate::Violation;

#[derive(Debug, Deserialize)]
struct Rewrite {
    #[serde(default)]
    reply: String,
}

const REWRITE_SYSTEM: &str = r#"The user is judging a candidate (a proposed approach or result, or an agent's change under review) and hasn't given their verdict yet. Your draft reply to their latest message was held back because it judges the candidate, hints at whether or where it is right or wrong, or settles the point it tests.

Rewrite it without any of that. Answer only what the user literally asked, as a general explanation or definition, and stop. Cut every sentence that links the answer to the task, the part, the candidate or a related idea ("which is where X comes in", "that's what matters when ..."), and every consequence or example the user didn't ask for. Don't say the candidate is right or wrong, don't point at a step, line or number in it, and don't give the fact that decides it. Advice on how to check it may only say to work it through from what's given and compare: no rule, formula, method or test to apply, no step to look at, no trap to watch for. Don't mention that anything was held back. Math as plain text, never LaTeX.

Return ONLY a JSON object:
{"reply": "..."}"#;

/// Added to the Tutor's prompt while the gate checks its replies: asides
/// that tie a general answer back to the topic are what settle a candidate.
pub(super) const NARROW: &str = "Something is being judged right now, so answer narrowly: reply to exactly what the user asked and stop. Don't add asides that tie your answer to the task, the part or a related idea (\"which is where X comes in\", \"that's what matters when ...\"), and don't add consequences, edge cases or examples they didn't ask for. If they ask how to check it, say only to work it through themselves from what's given and compare; don't name a rule, formula, method or test to apply, a step to look at, or a trap to watch for.";

const LEAK_OPTION: &str = "yes";

fn leak_question(name: &str, n: usize, probe: &str) -> Question {
    Question {
        name: name.to_string(),
        instructions: format!(
            "Candidate {n} is waiting for the user's verdict. Would sending the draft reply below reveal, hint at, or confirm {probe} Count hints spread across the conversation, and a reply that confirms what the user says about the candidate. Sharing the candidate's topic isn't a hint: a general explanation of a concept it uses, which doesn't touch the point that decides it, is a no. Advice on how to check it is a hint when it names a rule, formula, method or test to apply, a step to look at, or a trap to watch for; advice only to work it through from what's given and compare is a no."
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
                "They tell or let the user work out that it is flawed, or where".into(),
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
    /// check. `Ok(None)`: no decision service answered, so the thread is in
    /// masked mode.
    async fn leaks(
        &self,
        session: &Session,
        secrets: &[Secret],
        draft: &str,
    ) -> Result<Option<bool>> {
        if secrets.iter().any(|s| precheck_hit(s, draft)) {
            return Ok(Some(true));
        }
        Ok(self
            .check(session, secrets, draft)
            .await?
            .map(|c| c.blocked))
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
        tracing::debug!(?scores, ?bits, blocked, draft, "gate check");
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
            .decide(
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
            .decide(&input, &questions, session.decision_model.as_deref())
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

    /// The reply you may see: the draft if it passes, a rewrite if that
    /// passes, else the fixed reply.
    pub(super) async fn gate_reply(
        &self,
        session: &Session,
        secrets: &[Secret],
        message: &str,
        draft: String,
        sink: &StreamSink,
    ) -> String {
        let blocked = match self.leaks(session, secrets, &draft).await {
            Ok(None) | Ok(Some(false)) => return draft,
            Ok(Some(true)) => true,
            Err(e) => {
                tracing::warn!("the gate's check failed; withholding the reply: {e:#}");
                return FIXED_REPLY.to_string();
            }
        };
        debug_assert!(blocked);
        sink(StreamEvent::Thought(
            "\n\nThat draft touched the candidate you're judging; rewriting it.\n".into(),
        ));
        let rewritten = match self.rewrite(session, message, &draft).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("rewriting a held reply: {e:#}");
                return FIXED_REPLY.to_string();
            }
        };
        match self.leaks(session, secrets, &rewritten).await {
            Ok(None) | Ok(Some(false)) => rewritten,
            Ok(Some(true)) => FIXED_REPLY.to_string(),
            Err(e) => {
                tracing::warn!("the gate's check failed; withholding the reply: {e:#}");
                FIXED_REPLY.to_string()
            }
        }
    }

    async fn rewrite(&self, session: &Session, message: &str, draft: &str) -> Result<String> {
        let req = EngineRequest {
            tier: EngineTier::System1,
            tools: ToolAccess::None,
            cwd: session.cwd.clone(),
            system_prompt: REWRITE_SYSTEM.to_string(),
            prompt: format!(
                "The user's latest message: {message}\n\nYour held draft:\n{draft}\n\nRespond with JSON only."
            ),
            files: vec![],
            stream_text: false,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        let r: Rewrite = run_gated(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "rewrite",
            &req,
            quiet(),
            |r: &Rewrite| {
                if r.reply.trim().is_empty() {
                    vec![Violation::new("empty_field", "`reply` must not be empty")]
                } else {
                    vec![]
                }
            },
        )
        .await?;
        Ok(r.reply)
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
