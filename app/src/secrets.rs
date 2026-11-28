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
/// the draft, and the draft is blocked if it moves the reader as many bits
/// toward the true answer as the secret's stakes allow. b bits let a guesser
/// who follows the hint be right with probability 0.5 · 2^b.
///
/// A learning candidate: 0.2 bits, a guesser's best odds 57.4%.
pub const BITS_LIMIT: f64 = 0.2;
/// A review check, whose verdict vouches for the agent's work in the ledger
/// or marks it shaky: 0.14 bits, a guesser's best odds 55%. It blocks about
/// 2% of harmless replies on the eval's drafts, against none at 0.2.
pub const REVIEW_BITS_LIMIT: f64 = 0.14;

/// The bits a reply may move the key-blind reader toward a secret's true
/// answer, by what the verdict on it records.
pub fn bits_limit(secret: &Secret) -> f64 {
    match secret.kind.as_str() {
        "planted_flaw" | "review" => REVIEW_BITS_LIMIT,
        _ => BITS_LIMIT,
    }
}

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
pub const FIXED_REPLY: &str = "That's what you're judging. Verdict first.";

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

    /// An exam question's key: the points a good answer contains.
    pub fn exam_key(question: &str, key_points: &str, opened_at_turn: usize) -> Self {
        Self {
            id: format!("sec-{}", uuid::Uuid::new_v4().simple()),
            kind: "exam_key".into(),
            payload: json!({"candidate": question, "correct": true, "reason": key_points}),
            probe: format!(
                "the answer to the exam question: any of the points a good answer contains, or a judgment of the user's answer. (Key points: {key_points})"
            ),
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

/// A text as written and as the user is shown it: math in LaTeX is drawn
/// as Unicode (`crate::mathtext`), so the pre-check reads both.
fn forms(text: &str) -> Vec<String> {
    let shown = crate::mathtext::prose(text).into_owned();
    if shown == text {
        vec![shown]
    } else {
        vec![text.to_string(), shown]
    }
}

/// The deterministic pre-check (A6): the draft repeats the hidden key,
/// as six words in a row from it that the candidate itself doesn't
/// contain, or a long line of code from it. The key, the candidate and the
/// draft are each read as written and as displayed, so a key written in
/// Unicode is caught in a draft written in LaTeX, and the other way round.
pub fn precheck_hit(secret: &Secret, draft: &str) -> bool {
    const N: usize = 6;
    let reasons = forms(secret.reason());
    let candidates = forms(secret.candidate());
    let drafts = forms(draft);
    let shown: HashSet<String> = candidates
        .iter()
        .flat_map(|c| windows(&words(c), N))
        .collect();
    let drafted: HashSet<String> = drafts.iter().flat_map(|d| windows(&words(d), N)).collect();
    let echoed = reasons
        .iter()
        .flat_map(|r| windows(&words(r), N))
        .any(|w| !shown.contains(&w) && drafted.contains(&w));
    if echoed {
        return true;
    }
    reasons
        .iter()
        .flat_map(|r| r.lines())
        .map(str::trim)
        .filter(|l| l.len() >= 16 && !candidates.iter().any(|c| c.contains(*l)))
        .any(|l| drafts.iter().any(|d| d.contains(l)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviews_allow_fewer_bits_than_candidates() {
        let candidate = Secret::candidate_key("c", true, "r", 0);
        let mut review = candidate.clone();
        review.kind = "planted_flaw".into();
        assert!(bits_limit(&review) < bits_limit(&candidate));
        // A guesser's best odds at each limit.
        assert!((0.5 * 2f64.powf(bits_limit(&candidate)) - 0.574).abs() < 0.001);
        assert!((0.5 * 2f64.powf(bits_limit(&review)) - 0.551).abs() < 0.001);
    }

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

    #[test]
    fn precheck_reads_the_draft_as_it_is_shown() {
        // The key in Unicode, the draft in LaTeX: the user sees the key.
        let s = Secret::candidate_key(
            "fuse(a, b) multiplies the two densities.",
            false,
            "Uses μᵢ + σᵢ² instead of μᵢ + σᵢ",
            0,
        );
        let draft = r"Hint: it uses \(\mu_i + \sigma_i^2\) instead of \(\mu_i + \sigma_i\).";
        assert!(crate::mathtext::prose(draft).contains("uses μᵢ + σᵢ² instead of μᵢ + σᵢ"));
        assert!(precheck_hit(&s, draft));
        // The other way round: the key in LaTeX, the draft in Unicode.
        let s = Secret::candidate_key(
            "fuse(a, b) multiplies the two densities.",
            false,
            r"Uses \(\mu_i + \sigma_i^2\) instead of \(\mu_i + \sigma_i\)",
            0,
        );
        assert!(precheck_hit(
            &s,
            "Hint: it uses μᵢ + σᵢ² instead of μᵢ + σᵢ."
        ));
        // Math the candidate already shows is still fine to repeat.
        let s = Secret::candidate_key(
            r"It returns \(\mu_a + \mu_b\) over \(\sigma_a + \sigma_b\) here.",
            false,
            "It returns μₐ + μ_b over σₐ + σ_b here, which is wrong.",
            0,
        );
        assert!(!precheck_hit(
            &s,
            "The candidate returns μₐ + μ_b over σₐ + σ_b here, hmm."
        ));
    }
}
