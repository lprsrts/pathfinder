use std::collections::BTreeMap;

use serde::Serialize;

use crate::domain::{
    CheckDecision, CoachDecision, ConceptRecord, ConceptStatus, OnboardingDecision, PathItem,
    PlanGraph, PlanProposal, Session,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    pub code: &'static str,
    pub detail: String,
}

impl Violation {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

pub fn format_violations(violations: &[Violation]) -> String {
    violations
        .iter()
        .map(|v| format!("- [{}] {}", v.code, v.detail))
        .collect::<Vec<_>>()
        .join("\n")
}

const FLUFF_OPENERS: &[&str] = &[
    "great goal",
    "great question",
    "great choice",
    "great start",
    "great!",
    "awesome",
    "excellent",
    "fantastic",
    "wonderful",
    "certainly",
    "absolutely",
    "sure thing",
    "sure!",
    "of course!",
    "happy to help",
    "i'd love to",
    "i would love to",
    "let's dive in",
    "let's get started",
    "sounds great",
    "love that",
    "nice!",
];

pub fn check_tone_and_format(text: &str, field: &str) -> Vec<Violation> {
    let mut out = Vec::new();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        out.push(Violation::new(
            "empty_text",
            format!("{field} must not be empty"),
        ));
        return out;
    }

    let lower = trimmed.to_lowercase();
    for opener in FLUFF_OPENERS {
        if lower.starts_with(opener) {
            out.push(Violation::new(
                "fluff_opener",
                format!("{field} starts with filler/cheerleading (`{opener}`). Start directly with substance."),
            ));
            break;
        }
    }

    let excl_count = trimmed.chars().filter(|&c| c == '!').count();
    if excl_count > 1 {
        out.push(Violation::new(
            "cheerleading",
            format!("{field} has {excl_count} exclamation marks; keep tone calm and flat."),
        ));
    }

    if has_unescaped_math_underscores(trimmed) {
        out.push(Violation::new(
            "markdown_underscore_math",
            format!("{field} has math subscripts with raw underscores outside backticks (e.g. `w_1 x_1`), which Markdown turns into italics. Wrap formulas in inline code backticks or write `w1*x1`."),
        ));
    }

    out
}

fn has_unescaped_math_underscores(text: &str) -> bool {
    let mut in_code = false;
    let mut subscript_underscores = 0usize;
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        if chars[i] == '`' {
            in_code = !in_code;
            continue;
        }
        if !in_code && chars[i] == '_' && i > 0 && i + 1 < chars.len() {
            let prev = chars[i - 1];
            let next = chars[i + 1];
            // Pattern like w_1, x_i, a_2 where a single letter precedes `_` and digit/single letter follows
            let prev_is_single_var =
                prev.is_ascii_alphabetic() && (i < 2 || !chars[i - 2].is_ascii_alphanumeric());
            let next_is_sub = next.is_ascii_digit()
                || (next.is_ascii_alphabetic()
                    && (i + 2 >= chars.len() || !chars[i + 2].is_ascii_alphanumeric()));
            if prev_is_single_var && next_is_sub {
                subscript_underscores += 1;
            }
        }
    }
    subscript_underscores >= 2
}

pub fn check_length(
    text: &str,
    field: &str,
    max_words: usize,
    max_sentences: usize,
) -> Vec<Violation> {
    let mut out = Vec::new();
    let words = text.split_whitespace().count();
    if words > max_words {
        out.push(Violation::new(
            "too_long",
            format!("{field} is {words} words (max {max_words}). Tighten it."),
        ));
    }
    let sentences = count_sentences(text);
    if sentences > max_sentences {
        out.push(Violation::new(
            "too_many_sentences",
            format!("{field} has {sentences} sentences (max {max_sentences}). Keep it brief."),
        ));
    }
    out
}

fn count_sentences(text: &str) -> usize {
    let mut count = 0usize;
    let mut in_code = false;
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        if chars[i] == '`' {
            in_code = !in_code;
            continue;
        }
        if !in_code && matches!(chars[i], '.' | '?' | '!') {
            let next_ok = i + 1 == chars.len() || chars[i + 1].is_whitespace();
            let prev_is_digit = i > 0 && chars[i - 1].is_ascii_digit();
            let next_is_digit = i + 1 < chars.len() && chars[i + 1].is_ascii_digit();
            if next_ok && !(chars[i] == '.' && prev_is_digit && next_is_digit) {
                count += 1;
            }
        }
    }
    count.max(usize::from(!text.trim().is_empty()))
}

const STACK_QUESTION_MARKERS: &[&str] = &[
    "which language",
    "what language",
    "programming language",
    "which framework",
    "what framework",
    "tech stack",
    "which tools",
    "what tools",
    "preferred tools",
    "preferred language",
];

/// Flags a question about the stack when the workspace already shows it.
fn asks_known_stack(reply: &str, known_stack: &[String]) -> Option<Violation> {
    if known_stack.is_empty() || !reply.contains('?') {
        return None;
    }
    let lower = reply.to_lowercase();
    let marker = STACK_QUESTION_MARKERS.iter().find(|m| lower.contains(*m))?;
    Some(Violation::new(
        "asks_known_stack",
        format!(
            "reply asks about `{marker}`, but the workspace already shows: {}. Read the files and ask about intent or understanding instead.",
            known_stack.join("; ")
        ),
    ))
}

const PROMISE_PREFIXES: &[&str] = &[
    "i'll ",
    "i will ",
    "let me ",
    "i'm going to ",
    "i am going to ",
    "first i'll ",
];

const PROMISE_VERBS: &[&str] = &[
    "check",
    "look",
    "inspect",
    "read",
    "review",
    "examine",
    "explore",
    "scan",
    "open",
    "search",
    "investigate",
    "dig",
    "take a look",
    "go through",
];

/// A single-shot reply is the model's last word for the turn, so announcing
/// a future action ("I'll check the code") is always an empty promise.
fn promises_action(reply: &str, field: &str) -> Option<Violation> {
    let lower = reply.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    for prefix in PROMISE_PREFIXES {
        for (idx, _) in lower.match_indices(prefix) {
            let at_word_start = idx == 0
                || !lower[..idx]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric());
            if !at_word_start {
                continue;
            }
            let rest = &lower[idx + prefix.len()..];
            if let Some(verb) = PROMISE_VERBS.iter().find(|v| rest.starts_with(*v)) {
                return Some(Violation::new(
                    "promised_action",
                    format!(
                        "{field} announces `{}{verb}`, but this reply ends the turn and nothing will follow. Use the read tool before answering and state what you found, or just ask.",
                        prefix.trim_end()
                    ),
                ));
            }
        }
    }
    None
}

pub fn check_onboarding(
    decision: &OnboardingDecision,
    session: &Session,
    known_stack: &[String],
) -> Vec<Violation> {
    let mut out = Vec::new();
    out.extend(check_tone_and_format(&decision.reply, "reply"));
    out.extend(check_length(&decision.reply, "reply", 70, 3));
    out.extend(asks_known_stack(&decision.reply, known_stack));
    out.extend(promises_action(&decision.reply, "reply"));

    for c in &decision.observed_concepts {
        if c.status == ConceptStatus::Proven {
            out.push(Violation::new(
                "unverified_proof",
                format!(
                    "concept `{}` cannot be marked `proven` during onboarding before a check; use `claimed` or `unknown`.",
                    c.id
                ),
            ));
        }
    }

    if decision.ready_to_plan {
        let has_goal = session
            .goal
            .as_deref()
            .map(|g| !g.trim().is_empty())
            .unwrap_or(false)
            || decision
                .goal_summary
                .as_deref()
                .map(|g| !g.trim().is_empty())
                .unwrap_or(false);
        if !has_goal {
            out.push(Violation::new(
                "missing_goal",
                "cannot set ready_to_plan=true without a clear goal_summary.",
            ));
        }

        let has_starting = session
            .starting_point
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
            || decision
                .starting_point
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
        let has_concepts = !session.concepts.is_empty() || !decision.observed_concepts.is_empty();

        if !has_starting || !has_concepts {
            out.push(Violation::new(
                "unprobed_background",
                "cannot set ready_to_plan=true until you have probed the user's prior knowledge and recorded at least one entry in observed_concepts plus starting_point.",
            ));
        }
    }

    out
}

pub fn check_plan_proposal(
    proposal: &PlanProposal,
    existing_resolved: &[PathItem],
    concepts: &BTreeMap<String, ConceptRecord>,
) -> Vec<Violation> {
    let mut out = Vec::new();
    out.extend(check_tone_and_format(&proposal.brief, "brief"));
    out.extend(check_length(&proposal.brief, "brief", 80, 3));

    let mut merged = existing_resolved.to_vec();
    for item in &proposal.items {
        out.extend(check_tone_and_format(
            &item.title,
            &format!("{}.title", item.id),
        ));
        out.extend(check_tone_and_format(
            &item.user_task,
            &format!("{}.user_task", item.id),
        ));
        out.extend(check_length(
            &item.user_task,
            &format!("{}.user_task", item.id),
            75,
            3,
        ));

        if !item.introduces.is_empty() {
            out.extend(check_tone_and_format(
                &item.framing,
                &format!("{}.framing", item.id),
            ));
            out.extend(check_length(
                &item.framing,
                &format!("{}.framing", item.id),
                85,
                3,
            ));
            if item.framing.split_whitespace().count() < 8 {
                out.push(Violation::new(
                    "thin_framing",
                    format!(
                        "item `{}` introduces {} but framing is too thin; explain the new idea plainly in 1-3 sentences before the user task.",
                        item.id,
                        item.introduces.join(", ")
                    ),
                ));
            }
        } else if !item.framing.trim().is_empty() {
            out.extend(check_tone_and_format(
                &item.framing,
                &format!("{}.framing", item.id),
            ));
            out.extend(check_length(
                &item.framing,
                &format!("{}.framing", item.id),
                85,
                3,
            ));
        }

        merged.push(PathItem {
            id: item.id.clone(),
            title: item.title.clone(),
            depends_on: item.depends_on.clone(),
            requires: item.requires.clone(),
            introduces: item.introduces.clone(),
            framing: item.framing.clone(),
            user_task: item.user_task.clone(),
            check_criteria: item.check_criteria.clone(),
            status: crate::domain::ItemStatus::Pending,
            notes: None,
        });
    }

    let graph = PlanGraph { items: merged };
    if let Err(err) = graph.validate_against(concepts) {
        out.push(Violation::new("invalid_graph", err.to_string()));
    }

    out
}

pub fn check_coach(decision: &CoachDecision) -> Vec<Violation> {
    let mut out = Vec::new();
    out.extend(check_tone_and_format(&decision.reply, "reply"));
    out.extend(check_length(&decision.reply, "reply", 85, 4));
    out.extend(promises_action(&decision.reply, "reply"));

    if has_large_code_block(&decision.reply, 6) {
        out.push(Violation::new(
            "spoiler_code_dump",
            "coaching reply contains a large code block; guide with a short nudge instead of writing the solution.",
        ));
    }

    out
}

fn has_large_code_block(text: &str, max_lines: usize) -> bool {
    let mut in_fence = false;
    let mut block_lines = 0usize;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if in_fence {
                if block_lines > max_lines {
                    return true;
                }
                block_lines = 0;
            }
            in_fence = !in_fence;
        } else if in_fence {
            block_lines += 1;
        }
    }
    in_fence && block_lines > max_lines
}

pub fn check_verdict(decision: &CheckDecision) -> Vec<Violation> {
    let mut out = Vec::new();
    out.extend(check_tone_and_format(&decision.feedback, "feedback"));
    out.extend(check_length(&decision.feedback, "feedback", 95, 4));
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::{CoachAction, ObservedConcept, ProposedItem};

    #[test]
    fn rejects_fluff_opener_and_math_underscore_glitches() {
        let v = check_tone_and_format(
            "Great goal! Compute z = w_1 x_1 + w_2 x_2 + b for three pairs.",
            "reply",
        );
        let codes: Vec<&str> = v.iter().map(|x| x.code).collect();
        assert!(codes.contains(&"fluff_opener"));
        assert!(codes.contains(&"markdown_underscore_math"));
    }

    #[test]
    fn allows_backticked_math_and_calm_tone() {
        let v = check_tone_and_format(
            "A neuron combines inputs with weights and a bias: `z = w_1*x_1 + w_2*x_2 + b`.",
            "framing",
        );
        assert!(v.is_empty(), "unexpected violations: {v:?}");
    }

    #[test]
    fn blocks_onboarding_exit_without_probing_background() {
        let session = Session::new("s1".into(), PathBuf::from("."));
        let d = OnboardingDecision {
            reply: "Starting the path now.".into(),
            ready_to_plan: true,
            goal_summary: Some("Learn neuron mechanics".into()),
            starting_point: None,
            observed_concepts: vec![],
        };
        let v = check_onboarding(&d, &session, &[]);
        assert!(v.iter().any(|x| x.code == "unprobed_background"));
    }

    #[test]
    fn allows_onboarding_exit_when_goal_and_concepts_probed() {
        let session = Session::new("s1".into(), PathBuf::from("."));
        let d = OnboardingDecision {
            reply: "Building a path from basic algebra up to Python.".into(),
            ready_to_plan: true,
            goal_summary: Some("Learn neuron mechanics".into()),
            starting_point: Some("Knows basic algebra, new to neural nets".into()),
            observed_concepts: vec![ObservedConcept {
                id: "weighted_sum".into(),
                label: "Weighted sum".into(),
                status: ConceptStatus::Unknown,
            }],
        };
        let v = check_onboarding(&d, &session, &[]);
        assert!(v.is_empty(), "unexpected violations: {v:?}");
    }

    #[test]
    fn blocks_asking_for_stack_the_workspace_already_shows() {
        let session = Session::new("s1".into(), PathBuf::from("."));
        let d = OnboardingDecision {
            reply: "What should `/status` report, and which language do you want to use?".into(),
            ready_to_plan: false,
            goal_summary: None,
            starting_point: None,
            observed_concepts: vec![],
        };
        let stack = vec!["Rust (Cargo) in harness/".to_string()];
        let v = check_onboarding(&d, &session, &stack);
        assert!(v.iter().any(|x| x.code == "asks_known_stack"));

        // Same question is fine in an empty folder.
        assert!(check_onboarding(&d, &session, &[]).is_empty());
    }

    #[test]
    fn blocks_promised_actions_but_not_plain_questions() {
        let promised = [
            "I\u{2019}ll check how commands are currently handled. What should `/status` show?",
            "Let me look at runtime.rs first. What do you expect?",
            "First, I will inspect the store. Which fields matter?",
        ];
        for reply in promised {
            assert!(
                promises_action(reply, "reply").is_some(),
                "missed promise: {reply}"
            );
        }

        let fine = [
            "Commands live in `acp.rs` and `runtime.rs`. What should `/status` show?",
            "Would you check the output yourself, or should the harness?",
            "Should it read from the store? I'll leave that choice to you.",
        ];
        for reply in fine {
            assert!(
                promises_action(reply, "reply").is_none(),
                "false positive: {reply}"
            );
        }
    }

    #[test]
    fn blocks_plan_item_without_framing_for_new_concept() {
        let proposal = PlanProposal {
            brief: "Start on paper then move to Python.".into(),
            items: vec![ProposedItem {
                id: "p1".into(),
                title: "Weighted sum".into(),
                depends_on: vec![],
                requires: vec![],
                introduces: vec!["weighted_sum".into()],
                framing: "Short.".into(),
                user_task: "Compute `z = w1*x1 + w2*x2 + b` for three pairs.".into(),
                check_criteria: "Explains effect of weights and bias.".into(),
            }],
        };
        let v = check_plan_proposal(&proposal, &[], &BTreeMap::new());
        assert!(v.iter().any(|x| x.code == "thin_framing"));
    }

    #[test]
    fn blocks_large_code_dump_in_coaching() {
        let d = CoachDecision {
            reply: "Try this:\n```python\na=1\nb=2\nc=3\nd=4\ne=5\nf=6\ng=7\n```".into(),
            action: CoachAction::KeepGoing,
        };
        let v = check_coach(&d);
        assert!(v.iter().any(|x| x.code == "spoiler_code_dump"));
    }
}
