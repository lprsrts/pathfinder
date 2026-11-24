//! Shape checks on model output. These only ensure the output is usable:
//! required fields present and the map structurally sound. They do not judge
//! tone, length or style (the theory has no section for that).

use serde::Serialize;

use crate::domain::{
    CandidateProposal, Intent, JudgmentVerdict, LogReply, Map, MapChange, MapProposal,
    OnboardingDecision,
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

fn require_text(out: &mut Vec<Violation>, value: &str, field: &str) {
    if value.trim().is_empty() {
        out.push(Violation::new(
            "empty_field",
            format!("`{field}` must not be empty"),
        ));
    }
}

pub fn check_onboarding(d: &OnboardingDecision) -> Vec<Violation> {
    let mut out = Vec::new();
    require_text(&mut out, &d.reply, "reply");
    if d.ready_to_map && d.goal.as_deref().is_none_or(|g| g.trim().is_empty()) {
        out.push(Violation::new(
            "missing_goal",
            "`ready_to_map` is true but `goal` is empty",
        ));
    }
    out
}

/// Ids are typed in commands, so they must be short slugs.
const MAX_ID_LEN: usize = 16;

fn check_id(out: &mut Vec<Violation>, id: &str) {
    let ok = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-');
    if !ok {
        out.push(Violation::new(
            "bad_id",
            format!(
                "part id `{id}` must be a short slug: lowercase letters, digits and hyphens, at most 12 characters (e.g. `fwd-pass`)"
            ),
        ));
    }
}

pub fn check_map(p: &MapProposal) -> Vec<Violation> {
    let mut out = Vec::new();
    require_text(&mut out, &p.brief, "brief");
    for part in &p.parts {
        require_text(&mut out, &part.title, "parts[].title");
        check_id(&mut out, &part.id);
    }
    if let Err(e) = p.to_map().validate() {
        out.push(Violation::new("invalid_map", e.to_string()));
    }
    out
}

pub fn check_change(change: &MapChange, current: &Map) -> Vec<Violation> {
    let mut out = Vec::new();
    require_text(&mut out, &change.brief, "brief");
    for part in change.parts.iter().filter(|p| current.get(&p.id).is_none()) {
        check_id(&mut out, &part.id);
    }
    let new_goal = change.goal.as_deref().is_some_and(|g| !g.trim().is_empty());
    if change.parts.is_empty() && change.remove.is_empty() && !new_goal {
        out.push(Violation::new(
            "empty_change",
            "a map change must change the goal or add, update or remove at least one part",
        ));
    }
    if let Err(e) = current.with_change(change) {
        out.push(Violation::new("invalid_change", e));
    }
    out
}

/// `change_waiting`: whether a map change is waiting for the user's OK, the
/// only time `accept` or `reject` makes sense.
/// `decided_log`: a decision model already classified the message as a log
/// entry, so a reply is required whatever `intent` says.
pub fn check_log_reply(
    r: &LogReply,
    current: &Map,
    change_waiting: bool,
    decided_log: bool,
) -> Vec<Violation> {
    let mut out = Vec::new();
    if decided_log {
        require_text(&mut out, &r.reply, "reply");
        if let Some(change) = &r.map_change {
            out.extend(check_change(change, current));
        }
        return out;
    }
    if matches!(r.intent, Intent::Accept | Intent::Reject) && !change_waiting {
        out.push(Violation::new(
            "no_change_waiting",
            "no map change is waiting, so the intent can't be accept or reject; it's probably log",
        ));
    }
    if r.intent == Intent::Log {
        require_text(&mut out, &r.reply, "reply");
    }
    if let Some(change) = &r.map_change {
        out.extend(check_change(change, current));
    }
    out
}

pub fn check_candidate(c: &CandidateProposal) -> Vec<Violation> {
    let mut out = Vec::new();
    require_text(&mut out, &c.candidate, "candidate");
    require_text(&mut out, &c.key_reason, "key_reason");
    out
}

pub fn check_verdict(v: &JudgmentVerdict) -> Vec<Violation> {
    let mut out = Vec::new();
    if v.is_judgment {
        require_text(&mut out, &v.feedback, "feedback");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{PartStatus, ProposedPart};

    fn proposed(id: &str, deps: &[&str]) -> ProposedPart {
        ProposedPart {
            id: id.into(),
            title: format!("Part {id}"),
            note: String::new(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn onboarding_needs_goal_only_when_ready() {
        let asking = OnboardingDecision {
            reply: "What should it report?".into(),
            ready_to_map: false,
            goal: None,
        };
        assert!(check_onboarding(&asking).is_empty());

        let ready_without_goal = OnboardingDecision {
            ready_to_map: true,
            ..asking
        };
        assert_eq!(
            check_onboarding(&ready_without_goal)[0].code,
            "missing_goal"
        );
    }

    #[test]
    fn map_is_checked_for_structure_not_size() {
        let ok = MapProposal {
            brief: "b".into(),
            parts: (0..12).map(|i| proposed(&format!("p{i}"), &[])).collect(),
        };
        assert!(check_map(&ok).is_empty());

        let long_id = MapProposal {
            brief: "b".into(),
            parts: vec![proposed("Forward_Pass_Of_The_Neuron", &[])],
        };
        assert_eq!(check_map(&long_id)[0].code, "bad_id");

        let cyclic = MapProposal {
            brief: "b".into(),
            parts: vec![proposed("a", &["b"]), proposed("b", &["a"])],
        };
        assert_eq!(check_map(&cyclic)[0].code, "invalid_map");
    }

    #[test]
    fn accept_needs_a_waiting_change() {
        let reply = LogReply {
            intent: Intent::Accept,
            part: None,
            reply: String::new(),
            map_change: None,
        };
        let map = Map { parts: vec![] };
        assert_eq!(
            check_log_reply(&reply, &map, false, false)[0].code,
            "no_change_waiting"
        );
        assert!(check_log_reply(&reply, &map, true, false).is_empty());
    }

    #[test]
    fn log_reply_change_must_apply_to_current_map() {
        let mut current = Map {
            parts: vec![proposed("a", &[]).into_part()],
        };
        current.parts[0].status = PartStatus::Understood;
        let bad = LogReply {
            intent: Intent::Log,
            part: None,
            reply: "Noted.".into(),
            map_change: Some(MapChange {
                goal: None,
                brief: "drop a".into(),
                parts: vec![],
                remove: vec!["a".into()],
            }),
        };
        assert_eq!(
            check_log_reply(&bad, &current, false, false)[0].code,
            "invalid_change"
        );

        let good = LogReply {
            intent: Intent::Log,
            part: None,
            reply: "Noted.".into(),
            map_change: Some(MapChange {
                goal: None,
                brief: "add b".into(),
                parts: vec![proposed("b", &["a"])],
                remove: vec![],
            }),
        };
        assert!(check_log_reply(&good, &current, false, false).is_empty());
    }
}
