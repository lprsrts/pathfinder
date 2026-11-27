//! Session state. Section numbers refer to `docs/theory.md`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Onboarding,
    Working,
    Done,
}

/// Where a part stands. The understanding behind it (§8) is `record()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartStatus {
    /// Nothing decided yet.
    Open,
    /// Understanding shown through log, summary and candidate (§5).
    Understood,
    /// Understood and handed off; the agent's result awaits your review (§7).
    InReview,
    /// Understood and finished: you approved the agent's result (§7).
    Done,
    /// Skipped as already known, on your word (§6).
    Known,
    /// Skipped and handed off without understanding (§6).
    Delegated,
}

impl PartStatus {
    pub fn is_resolved(self) -> bool {
        matches!(
            self,
            Self::Understood | Self::Done | Self::Known | Self::Delegated
        )
    }

    /// The §8 record this status falls under.
    pub fn record(self) -> Option<&'static str> {
        match self {
            Self::Open => None,
            Self::Understood | Self::InReview | Self::Done => Some("understood"),
            Self::Known => Some("claimed_known"),
            Self::Delegated => Some("handed_off_without_understanding"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Understood => "understood",
            Self::InReview => "in review",
            Self::Done => "understood, done",
            Self::Known => "claimed as known",
            Self::Delegated => "handed off without understanding",
        }
    }
}

/// Something the task depends on (§3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Part {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub status: PartStatus,
    /// Your thinking and reasons while working (§5.1).
    #[serde(default)]
    pub log: Vec<String>,
    /// Your summary of the part (§5.2).
    #[serde(default)]
    pub summary: Option<String>,
    /// The agent's latest result (§7).
    #[serde(default)]
    pub result: Option<String>,
    /// Objections you sent back to the agent (§7).
    #[serde(default)]
    pub objections: Vec<String>,
    /// The 1–4 core concepts the part rests on (§10, A4).
    #[serde(default)]
    pub concepts: Vec<String>,
    /// Ledger change entries from the agent's latest run, awaiting review.
    #[serde(default)]
    pub unreviewed: Vec<String>,
    /// A handoff's isolated copy, held until your verdict (§13, A8).
    #[serde(default)]
    pub staging: Option<Staging>,
}

/// Where a handoff runs until your verdict. Whether the review is checked
/// lives only in the secret, never here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Staging {
    pub dir: PathBuf,
    /// The workspace as a git tree when the handoff began.
    pub base: String,
    /// The agent's result as a git tree, once it finished.
    #[serde(default)]
    pub result: Option<String>,
    /// The review's secret: a planted flaw, or the plain review key.
    #[serde(default)]
    pub secret: Option<String>,
    /// The agent's report, shown after your verdict.
    #[serde(default)]
    pub report: String,
    /// Ignored top-level folders linked into the copy, left out of snapshots.
    #[serde(default)]
    pub linked: Vec<String>,
}

impl Part {
    pub fn new(id: String, title: String, note: String, depends_on: Vec<String>) -> Self {
        Self {
            id,
            title,
            note,
            depends_on,
            status: PartStatus::Open,
            log: Vec::new(),
            summary: None,
            result: None,
            objections: Vec::new(),
            concepts: Vec::new(),
            unreviewed: Vec::new(),
            staging: None,
        }
    }
}

/// The rough map of what the task depends on (§3). No size limit.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Map {
    pub parts: Vec<Part>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MapError {
    #[error("the map must contain at least one part")]
    Empty,
    #[error("part id must not be empty")]
    EmptyId,
    #[error("duplicate part id `{0}`")]
    DuplicateId(String),
    #[error("part `{part}` depends on unknown part `{missing}`")]
    MissingDependency { part: String, missing: String },
    #[error("part `{0}` cannot depend on itself")]
    SelfDependency(String),
    #[error("the map contains a dependency cycle")]
    Cycle,
}

impl Map {
    pub fn validate(&self) -> Result<(), MapError> {
        if self.parts.is_empty() {
            return Err(MapError::Empty);
        }

        let mut ids = HashSet::with_capacity(self.parts.len());
        for part in &self.parts {
            if part.id.trim().is_empty() {
                return Err(MapError::EmptyId);
            }
            if !ids.insert(part.id.as_str()) {
                return Err(MapError::DuplicateId(part.id.clone()));
            }
        }

        let mut indegree: HashMap<&str, usize> =
            self.parts.iter().map(|p| (p.id.as_str(), 0)).collect();
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
        for part in &self.parts {
            for dep in &part.depends_on {
                if dep == &part.id {
                    return Err(MapError::SelfDependency(part.id.clone()));
                }
                if !ids.contains(dep.as_str()) {
                    return Err(MapError::MissingDependency {
                        part: part.id.clone(),
                        missing: dep.clone(),
                    });
                }
                *indegree.get_mut(part.id.as_str()).unwrap() += 1;
                dependents
                    .entry(dep.as_str())
                    .or_default()
                    .push(part.id.as_str());
            }
        }

        let mut queue: VecDeque<&str> = indegree
            .iter()
            .filter_map(|(&id, &deg)| (deg == 0).then_some(id))
            .collect();
        let mut seen = 0;
        while let Some(id) = queue.pop_front() {
            seen += 1;
            for &next in dependents.get(id).into_iter().flatten() {
                let deg = indegree.get_mut(next).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(next);
                }
            }
        }
        if seen != self.parts.len() {
            return Err(MapError::Cycle);
        }
        Ok(())
    }

    /// Finds a part by exact id, 1-based position, or unique id prefix.
    pub fn resolve(&self, query: &str) -> Result<String, String> {
        let query = query.trim().trim_start_matches('`').trim_end_matches('`');
        if let Some(part) = self.get(query) {
            return Ok(part.id.clone());
        }
        if let Ok(n) = query.parse::<usize>() {
            return match n.checked_sub(1).and_then(|i| self.parts.get(i)) {
                Some(part) => Ok(part.id.clone()),
                None => Err(format!(
                    "There is no part {n}; the map has {}.",
                    self.parts.len()
                )),
            };
        }
        let matches: Vec<&Part> = self
            .parts
            .iter()
            .filter(|p| p.id.starts_with(query))
            .collect();
        match matches.as_slice() {
            [one] => Ok(one.id.clone()),
            [] => Err(format!("No part matches `{query}`. `/map` shows the ids.")),
            many => Err(format!(
                "`{query}` matches {}. Type more of the id.",
                many.iter()
                    .map(|p| format!("`{}`", p.id))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Part> {
        self.parts.iter_mut().find(|p| p.id == id)
    }

    fn deps_resolved(&self, part: &Part) -> bool {
        part.depends_on
            .iter()
            .all(|d| self.get(d).is_some_and(|p| p.status.is_resolved()))
    }

    /// Next unresolved part other than `skip`, preferring one whose
    /// dependencies are resolved. Order is a suggestion; you can focus any part.
    pub fn next_open(&self, skip: Option<&str>) -> Option<String> {
        let candidates = || {
            self.parts
                .iter()
                .filter(|p| !p.status.is_resolved() && Some(p.id.as_str()) != skip)
        };
        candidates()
            .find(|p| self.deps_resolved(p))
            .or_else(|| candidates().next())
            .map(|p| p.id.clone())
    }

    pub fn all_resolved(&self) -> bool {
        !self.parts.is_empty() && self.parts.iter().all(|p| p.status.is_resolved())
    }

    pub fn render(&self, active: Option<&str>) -> String {
        if self.parts.is_empty() {
            return "No map yet.".to_string();
        }
        let mut lines = vec!["Map:".to_string()];
        for part in &self.parts {
            let marker = if Some(part.id.as_str()) == active {
                "- ▶"
            } else {
                "-"
            };
            let deps = if part.depends_on.is_empty() {
                String::new()
            } else {
                format!(" (needs {})", part.depends_on.join(", "))
            };
            lines.push(format!(
                "{marker} `{}` {} [{}]{deps}",
                part.id,
                part.title,
                part.status.label()
            ));
        }
        lines.join("\n")
    }

    /// The map as a Mermaid flowchart for the user: parts
    /// with their status, an arrow from each part to the parts that need it,
    /// the active part marked. `status` names a status in the user's
    /// language.
    pub fn mermaid(&self, active: Option<&str>, status: impl Fn(PartStatus) -> String) -> String {
        let mut out = String::from("```mermaid\nflowchart TD\n");
        for part in &self.parts {
            let mark = if Some(part.id.as_str()) == active {
                "▶ "
            } else {
                ""
            };
            out.push_str(&format!(
                "    {}[\"{mark}{}<br/><small>{} · {}</small>\"]\n",
                mermaid_id(&part.id),
                mermaid_text(&part.title),
                mermaid_text(&part.id),
                mermaid_text(&status(part.status)),
            ));
        }
        for part in &self.parts {
            for dep in &part.depends_on {
                out.push_str(&format!(
                    "    {} --> {}\n",
                    mermaid_id(dep),
                    mermaid_id(&part.id)
                ));
            }
        }
        out.push_str("```");
        out
    }

    /// Applies a proposed change (§3) and validates the result.
    pub fn with_change(&self, change: &MapChange) -> Result<Map, String> {
        let mut map = self.clone();
        for id in &change.remove {
            let Some(part) = map.get(id) else {
                return Err(format!("cannot remove unknown part `{id}`"));
            };
            if part.status != PartStatus::Open || !part.log.is_empty() {
                return Err(format!(
                    "cannot remove part `{id}`: it already has work or an outcome"
                ));
            }
            map.parts.retain(|p| &p.id != id);
        }
        for proposed in &change.parts {
            match map.get_mut(&proposed.id) {
                Some(existing) => {
                    existing.title = proposed.title.clone();
                    existing.note = proposed.note.clone();
                    existing.depends_on = proposed.depends_on.clone();
                    if !proposed.concepts.is_empty() {
                        existing.concepts = proposed.concepts.clone();
                    }
                }
                None => map.parts.push(proposed.clone().into_part()),
            }
        }
        map.validate().map_err(|e| e.to_string())?;
        Ok(map)
    }
}

/// A candidate you judge (§5.3). The key stays hidden until you've judged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingCandidate {
    pub part_id: String,
    pub candidate: String,
    /// The key lives in the secrets table (A6), not in the thread's state;
    /// these are filled from it when the Judge needs them. Older threads
    /// stored them here and are moved over on load.
    #[serde(default, skip_serializing)]
    pub key_correct: bool,
    #[serde(default, skip_serializing)]
    pub key_reason: String,
    /// The secret holding the key.
    #[serde(default)]
    pub secret: Option<String>,
    /// The concepts it checks (A5).
    #[serde(default)]
    pub targets: Vec<String>,
    /// The misconception it was built to embody (T1).
    #[serde(default)]
    pub misconception: Option<String>,
    #[serde(default)]
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub cwd: PathBuf,
    pub goal: Option<String>,
    /// The conversation's language (two-letter code), from onboarding: the
    /// fixed lines Pathfinder writes itself follow it.
    #[serde(default)]
    pub language: Option<String>,
    /// The task is a quick check of something just explained: no gap
    /// proposals, just the check.
    #[serde(default)]
    pub quick_check: bool,
    /// An exam: open questions on the map's parts, one at a time, each
    /// answer graded, until the user stops.
    #[serde(default)]
    pub exam: bool,
    pub mode: SessionMode,
    #[serde(default)]
    pub map: Map,
    pub active: Option<String>,
    #[serde(default)]
    pub pending_candidate: Option<PendingCandidate>,
    /// A map change the model proposed, waiting for your OK (§3).
    #[serde(default)]
    pub pending_change: Option<MapChange>,
    /// Model picks (config entry ids); None follows the config default.
    #[serde(default)]
    pub system1: Option<String>,
    #[serde(default)]
    pub system2: Option<String>,
    /// Effort for the main model (`low`, `medium`, `high`); None follows
    /// the model entry or the tier default.
    #[serde(default)]
    pub effort: Option<String>,
    /// The same for the fast model.
    #[serde(default)]
    pub fast_effort: Option<String>,
    /// The Decisions switch: off skips the decision model for this thread.
    #[serde(default)]
    pub decisions_off: bool,
    /// Decision service id (`typesafe`, `openai`, `gemini`); None picks the
    /// first one with a key.
    #[serde(default)]
    pub decision_model: Option<String>,
    pub history: Vec<TurnRecord>,
    /// Review checks (§13): `off`, `light` or `normal`; None is `light`.
    #[serde(default)]
    pub checks: Option<String>,
    /// Gap concepts already raised in this thread, so each is raised once.
    #[serde(default)]
    pub gaps_raised: Vec<String>,
}

impl Session {
    pub fn new(id: String, cwd: PathBuf) -> Self {
        Self {
            id,
            cwd,
            goal: None,
            language: None,
            quick_check: false,
            exam: false,
            mode: SessionMode::Onboarding,
            map: Map::default(),
            active: None,
            pending_candidate: None,
            pending_change: None,
            system1: None,
            system2: None,
            effort: None,
            fast_effort: None,
            decisions_off: false,
            decision_model: None,
            history: Vec::new(),
            gaps_raised: Vec::new(),
            checks: None,
        }
    }

    pub fn push_turn(&mut self, role: &str, content: impl Into<String>) {
        self.history.push(TurnRecord {
            role: role.to_string(),
            content: content.into(),
        });
    }

    pub fn active_part(&self) -> Option<&Part> {
        self.map.get(self.active.as_deref()?)
    }

    pub fn active_part_mut(&mut self) -> Option<&mut Part> {
        let id = self.active.clone()?;
        self.map.get_mut(&id)
    }

    /// Moves to the next unresolved part, or to Done when none is left.
    pub fn advance(&mut self) -> Option<String> {
        let next = self.map.next_open(self.active.as_deref());
        if next.is_some() {
            self.active = next.clone();
            self.mode = SessionMode::Working;
        } else if self.map.all_resolved() {
            self.active = None;
            self.mode = SessionMode::Done;
        }
        next
    }
}

// Model outputs.

#[derive(Debug, Clone, Deserialize)]
pub struct OnboardingDecision {
    #[serde(default)]
    pub reply: String,
    #[serde(default)]
    pub ready_to_map: bool,
    #[serde(default)]
    pub goal: Option<String>,
    /// A question to answer, not a task: Pathfinder answers it directly.
    #[serde(default)]
    pub question: bool,
    /// The question is about understanding something, not a quick lookup:
    /// offer a check after the answer.
    #[serde(default)]
    pub offer_check: bool,
    /// The language the user writes in (two-letter code).
    #[serde(default)]
    pub language: Option<String>,
    /// The goal is a quick check of a topic just explained.
    #[serde(default)]
    pub check: bool,
    /// The user asked to be examined: open questions, one at a time.
    #[serde(default)]
    pub exam: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProposedPart {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Core concept ids, 1–4.
    #[serde(default)]
    pub concepts: Vec<String>,
}

impl ProposedPart {
    pub fn into_part(self) -> Part {
        let mut part = Part::new(self.id, self.title, self.note, self.depends_on);
        part.concepts = self.concepts;
        part
    }
}

/// A concept the map rests on, with the concepts it needs first.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProposedConcept {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub needs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MapProposal {
    pub brief: String,
    pub parts: Vec<ProposedPart>,
    #[serde(default)]
    pub concepts: Vec<ProposedConcept>,
}

impl MapProposal {
    pub fn to_map(&self) -> Map {
        Map {
            parts: self
                .parts
                .iter()
                .cloned()
                .map(ProposedPart::into_part)
                .collect(),
        }
    }
}

/// Parts to add or update, and open parts to remove.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MapChange {
    pub brief: String,
    /// A new goal, when the change alters what the task is (§3).
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub parts: Vec<ProposedPart>,
    #[serde(default)]
    pub remove: Vec<String>,
    /// New concepts the added or updated parts rest on.
    #[serde(default)]
    pub concepts: Vec<ProposedConcept>,
}

/// What a chat message in a working session means, so plain chat drives the
/// loop and commands are shortcuts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Intent {
    /// A log entry: the user's thinking or a question (§5).
    #[default]
    Log,
    /// Their explanation of the whole part, as `/summary`.
    Summary,
    /// They already know this part, as `/known` (§6).
    Known,
    /// Go to the next part, or to `part`, as `/next` or `/focus`.
    Move,
    /// Accept or reject the waiting map change.
    Accept,
    Reject,
    /// They ask to change the plan, or say something that changes the
    /// task's scope or purpose: the main model proposes a map change.
    Replan,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogReply {
    #[serde(default)]
    pub intent: Intent,
    /// For `move`: the part to go to, if they named one.
    #[serde(default)]
    pub part: Option<String>,
    #[serde(default)]
    pub reply: String,
    #[serde(default)]
    pub map_change: Option<MapChange>,
    /// Concepts the message touched: ids from the part's list, or short
    /// names of new ones (the Extractor, A2).
    #[serde(default)]
    pub concepts: Vec<String>,
    /// A wrong belief the message states, if any.
    #[serde(default)]
    pub misconception: Option<ProposedMisconception>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProposedMisconception {
    pub concept: String,
    pub statement: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CandidateProposal {
    pub candidate: String,
    pub key_reason: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JudgmentVerdict {
    /// False when the message isn't a verdict on the candidate at all (a
    /// question, a log entry, a request to move on); it is then routed as chat.
    #[serde(default = "yes")]
    pub is_judgment: bool,
    #[serde(default)]
    pub pass: bool,
    #[serde(default)]
    pub feedback: String,
    /// On a fail: the wrong belief their judgment showed, if one did.
    #[serde(default)]
    pub misconception: Option<String>,
}

fn yes() -> bool {
    true
}

/// A Mermaid node id: letters, digits and underscores only.
pub fn mermaid_id(id: &str) -> String {
    let body: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("n_{body}")
}

/// Text inside a quoted Mermaid label.
pub fn mermaid_text(text: &str) -> String {
    text.replace('"', "#quot;").replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_draws_as_mermaid() {
        let mut a = Part::new(
            "fwd-pass".into(),
            "Forward \"pass\"".into(),
            String::new(),
            vec![],
        );
        a.status = PartStatus::Understood;
        let b = Part::new(
            "loss".into(),
            "Loss".into(),
            String::new(),
            vec!["fwd-pass".into()],
        );
        let map = Map { parts: vec![a, b] };
        let m = map.mermaid(Some("loss"), |s| s.label().to_string());
        assert!(m.starts_with("```mermaid\nflowchart TD\n") && m.ends_with("```"));
        assert!(
            m.contains(
                "n_fwd_pass[\"Forward #quot;pass#quot;<br/><small>fwd-pass · understood</small>\"]"
            ),
            "{m}"
        );
        assert!(
            m.contains("n_loss[\"▶ Loss<br/><small>loss · open</small>\"]"),
            "{m}"
        );
        assert!(m.contains("n_fwd_pass --> n_loss"), "{m}");
    }

    fn part(id: &str, deps: &[&str], status: PartStatus) -> Part {
        let mut p = Part::new(
            id.into(),
            format!("Part {id}"),
            String::new(),
            deps.iter().map(|s| s.to_string()).collect(),
        );
        p.status = status;
        p
    }

    fn map(parts: Vec<Part>) -> Map {
        Map { parts }
    }

    #[test]
    fn validates_structure_only() {
        assert_eq!(map(vec![]).validate(), Err(MapError::Empty));
        assert_eq!(
            map(vec![
                part("a", &[], PartStatus::Open),
                part("a", &[], PartStatus::Open)
            ])
            .validate(),
            Err(MapError::DuplicateId("a".into()))
        );
        assert!(matches!(
            map(vec![part("a", &["zz"], PartStatus::Open)]).validate(),
            Err(MapError::MissingDependency { .. })
        ));
        assert_eq!(
            map(vec![part("a", &["a"], PartStatus::Open)]).validate(),
            Err(MapError::SelfDependency("a".into()))
        );
        assert_eq!(
            map(vec![
                part("a", &["b"], PartStatus::Open),
                part("b", &["a"], PartStatus::Open)
            ])
            .validate(),
            Err(MapError::Cycle)
        );
        // No size limit.
        let many: Vec<Part> = (0..40)
            .map(|i| part(&format!("p{i}"), &[], PartStatus::Open))
            .collect();
        assert!(map(many).validate().is_ok());
    }

    #[test]
    fn resolve_takes_id_number_or_unique_prefix() {
        let m = map(vec![
            part("weights", &[], PartStatus::Open),
            part("fwd-pass", &["weights"], PartStatus::Open),
            part("fwd-check", &[], PartStatus::Open),
        ]);
        assert_eq!(m.resolve("weights").unwrap(), "weights");
        assert_eq!(m.resolve("2").unwrap(), "fwd-pass");
        assert_eq!(m.resolve("we").unwrap(), "weights");
        assert_eq!(m.resolve("fwd-c").unwrap(), "fwd-check");
        assert!(
            m.resolve("fwd")
                .unwrap_err()
                .contains("`fwd-pass`, `fwd-check`")
        );
        assert!(m.resolve("9").is_err());
        assert!(m.resolve("zz").is_err());
    }

    #[test]
    fn records_keep_the_three_outcomes_apart() {
        assert_eq!(PartStatus::Open.record(), None);
        assert_eq!(PartStatus::Understood.record(), Some("understood"));
        assert_eq!(PartStatus::InReview.record(), Some("understood"));
        assert_eq!(PartStatus::Done.record(), Some("understood"));
        assert_eq!(PartStatus::Known.record(), Some("claimed_known"));
        assert_eq!(
            PartStatus::Delegated.record(),
            Some("handed_off_without_understanding")
        );
        assert!(!PartStatus::InReview.is_resolved());
    }

    #[test]
    fn next_open_prefers_ready_parts_but_never_blocks() {
        let m = map(vec![
            part("a", &[], PartStatus::Known),
            part("b", &["c"], PartStatus::Open),
            part("c", &[], PartStatus::Open),
        ]);
        assert_eq!(m.next_open(None).as_deref(), Some("c"));
        assert_eq!(m.next_open(Some("c")).as_deref(), Some("b"));
    }

    #[test]
    fn map_change_adds_updates_and_guards_removal() {
        let mut worked = part("b", &[], PartStatus::Open);
        worked.log.push("tried something".into());
        let m = map(vec![part("a", &[], PartStatus::Open), worked]);

        let change = MapChange {
            goal: None,
            brief: "x".into(),
            parts: vec![
                ProposedPart {
                    id: "c".into(),
                    title: "New".into(),
                    note: String::new(),
                    depends_on: vec!["a".into()],
                    concepts: vec![],
                },
                ProposedPart {
                    id: "a".into(),
                    title: "Renamed".into(),
                    note: String::new(),
                    depends_on: vec![],
                    concepts: vec![],
                },
            ],
            remove: vec![],
            concepts: vec![],
        };
        let next = m.with_change(&change).unwrap();
        assert_eq!(next.parts.len(), 3);
        assert_eq!(next.get("a").unwrap().title, "Renamed");

        let remove_worked = MapChange {
            goal: None,
            brief: "x".into(),
            parts: vec![],
            remove: vec!["b".into()],
            concepts: vec![],
        };
        assert!(m.with_change(&remove_worked).is_err());

        let cycle = MapChange {
            goal: None,
            brief: "x".into(),
            parts: vec![ProposedPart {
                id: "a".into(),
                title: "A".into(),
                note: String::new(),
                depends_on: vec!["a".into()],
                concepts: vec![],
            }],
            remove: vec![],
            concepts: vec![],
        };
        assert!(m.with_change(&cycle).is_err());
    }
}
