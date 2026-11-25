//! Secrets (theory §11, A6): what is kept from you until your verdict, and
//! the gate's deterministic pre-check. Only decision models, which return
//! no text, ever see a secret's payload or probe besides the model that
//! wrote it and the Judge.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The gate passes a draft whose leak score for every secret is under
/// this (A6)...
pub const PASS_BELOW: f64 = 0.15;
/// ...and blocks one whose score for any secret reaches this.
pub const BLOCK_FROM: f64 = 0.5;
/// In between, a reader that never sees the key reads the conversation with
/// the draft, and the draft is blocked if it moves the reader this many bits
/// toward the true answer (`harness eval gate` set these three).
pub const BITS_LIMIT: f64 = 0.2;

/// Pointwise information, in bits, that a reader's distribution over
/// (right, flawed, neither) carries toward the true answer. "Neither" keeps
/// the even prior; one bit means the reader is certain of the truth, and a
/// negative value means it leans the wrong way.
pub fn bits_toward(dist: [f64; 3], correct: bool) -> f64 {
    let [right, flawed, neither] = dist;
    let total = right + flawed + neither;
    if total <= 0.0 {
        return 0.0;
    }
    let q = ((if correct { right } else { flawed }) + 0.5 * neither) / total;
    (q.clamp(1e-6, 1.0 - 1e-6) / 0.5).log2()
}

/// Shown when a draft fails the gate twice, or the check itself fails.
pub const FIXED_REPLY: &str = "That's what you're judging right now, so I can't settle it before your verdict. Give your verdict with your reasons; what was right or wrong is shown afterwards.";

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
    fn bits_measure_the_move_toward_the_truth() {
        assert!(bits_toward([0.0, 0.0, 1.0], true).abs() < 1e-9);
        assert!((bits_toward([1.0, 0.0, 0.0], true) - 1.0).abs() < 1e-3);
        assert!(bits_toward([1.0, 0.0, 0.0], false) < -10.0);
        // 0.3 on the true answer, the rest undecided: q = 0.65.
        let b = bits_toward([0.0, 0.3, 0.7], false);
        assert!((b - (0.65f64 / 0.5).log2()).abs() < 1e-9);
    }

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
