//! Secrets (theory §11, A6): what is kept from you until your verdict, and
//! the gate's deterministic pre-check. Only decision models, which return
//! no text, ever see a secret's payload or probe besides the model that
//! wrote it and the Judge.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The gate blocks a draft when any secret's probe reaches this (A6).
pub const LEAK_THRESHOLD: f64 = 0.3;

/// Shown when a draft fails the gate twice, or the check itself fails.
pub const FIXED_REPLY: &str = "That's what the candidate is testing, so I can't answer it before your verdict. Say whether you think it's right and why; the feedback afterwards covers it.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Secret {
    pub id: String,
    /// `candidate_key` or `planted_flaw`.
    pub kind: String,
    /// Never sent to you before your verdict.
    pub payload: Value,
    /// The gate's question.
    pub probe: String,
    /// Index into the thread's history where the secret opened.
    pub opened_at_turn: usize,
    pub active: bool,
}

impl Secret {
    pub fn candidate_key(
        candidate: &str,
        correct: bool,
        reason: &str,
        opened_at_turn: usize,
    ) -> Self {
        let probe = if correct {
            format!("whether the candidate is right or wrong. (It is right: {reason})")
        } else {
            format!(
                "whether the candidate is right or wrong, or where or what its flaw is. (It is flawed: {reason})"
            )
        };
        Self {
            id: format!("sec-{}", uuid::Uuid::new_v4().simple()),
            kind: "candidate_key".into(),
            payload: json!({"candidate": candidate, "correct": correct, "reason": reason}),
            probe,
            opened_at_turn,
            active: true,
        }
    }

    pub fn candidate(&self) -> &str {
        self.payload["candidate"].as_str().unwrap_or_default()
    }

    pub fn correct(&self) -> bool {
        self.payload["correct"].as_bool().unwrap_or(false)
    }

    pub fn reason(&self) -> &str {
        self.payload["reason"].as_str().unwrap_or_default()
    }
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn windows(words: &[String], n: usize) -> HashSet<String> {
    words.windows(n).map(|w| w.join(" ")).collect()
}

/// The deterministic pre-check (A6): the draft repeats the hidden key,
/// as six words in a row from it that the candidate itself doesn't
/// contain, or a long line of code from it.
pub fn precheck_hit(secret: &Secret, draft: &str) -> bool {
    const N: usize = 6;
    let reason = secret.reason();
    let shown = windows(&words(secret.candidate()), N);
    let key: HashSet<String> = windows(&words(reason), N)
        .difference(&shown)
        .cloned()
        .collect();
    let drafted = windows(&words(draft), N);
    if key.iter().any(|w| drafted.contains(w)) {
        return true;
    }
    let candidate = secret.candidate();
    reason
        .lines()
        .map(str::trim)
        .filter(|l| l.len() >= 16 && !candidate.contains(*l))
        .any(|l| draft.contains(l))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precheck_catches_echoes_of_the_key_only() {
        let s = Secret::candidate_key(
            "PC removes the edge A-B when A and B are dependent given C.",
            false,
            "It removes edges on dependence instead of independence, which keeps spurious edges out of the wrong set.\nif not indep(a, b, s): remove(a, b)",
            0,
        );
        assert!(precheck_hit(
            &s,
            "Hmm, it removes edges on dependence instead of independence, so..."
        ));
        assert!(precheck_hit(
            &s,
            "Look at `if not indep(a, b, s): remove(a, b)`."
        ));
        // Quoting the candidate itself, or talking about the topic, is fine.
        assert!(!precheck_hit(
            &s,
            "The candidate says PC removes the edge A-B when A and B are dependent given C."
        ));
        assert!(!precheck_hit(
            &s,
            "Independence tests decide which edges go."
        ));
    }
}
