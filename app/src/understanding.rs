//! The understanding model (theory §10, A4–A5): concepts, an append-only
//! event log, and the rules that compute every state from it. Nothing here
//! calls a model; states, part status, candidate targets and gaps are pure
//! functions of the events and a `now`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A `solid` concept with no pass or `review_ok` for this long cools to
/// `shown` (A5).
pub const AGE_SECS: i64 = 30 * 24 * 3600;

/// The workspace concepts belong to: the enclosing git root, else `cwd`.
pub fn workspace_key(cwd: &Path) -> String {
    let mut dir: Option<&Path> = Some(cwd);
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return d.to_string_lossy().into_owned();
        }
        dir = d.parent();
    }
    PathBuf::from(cwd).to_string_lossy().into_owned()
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Lowercase words joined by hyphens, at most 32 characters.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&word.to_lowercase());
    }
    out.chars()
        .take(32)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Concept {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// `map`, `extractor` or `user`.
    pub created_by: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Claim,
    Touched,
    MisconceptionShown,
    Pass,
    Fail,
    MisconceptionResolved,
    Delegated,
    ReviewOk,
    ReviewMissed,
    User,
    /// You were told about it (a question answered outside a check). It
    /// never counts as understanding.
    Told,
    /// You opened a source to read it (`/open`). Activity, never evidence.
    Read,
    /// You searched the sources (`/find`, `/web`). Activity, never evidence.
    Searched,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claim => "claim",
            Self::Touched => "touched",
            Self::MisconceptionShown => "misconception_shown",
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::MisconceptionResolved => "misconception_resolved",
            Self::Delegated => "delegated",
            Self::ReviewOk => "review_ok",
            Self::ReviewMissed => "review_missed",
            Self::User => "user",
            Self::Told => "told",
            Self::Read => "read",
            Self::Searched => "searched",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "claim" => Self::Claim,
            "touched" => Self::Touched,
            "misconception_shown" => Self::MisconceptionShown,
            "pass" => Self::Pass,
            "fail" => Self::Fail,
            "misconception_resolved" => Self::MisconceptionResolved,
            "delegated" => Self::Delegated,
            "review_ok" => Self::ReviewOk,
            "review_missed" => Self::ReviewMissed,
            "user" => Self::User,
            "told" => Self::Told,
            "read" => Self::Read,
            "searched" => Self::Searched,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: i64,
    pub concept: String,
    pub kind: EventKind,
    /// A candidate, misconception, ledger entry or message id.
    pub reference: Option<String>,
    /// `pass`: `{"flawed": bool}`.
    pub detail: Value,
    /// Unix seconds.
    pub at: i64,
}

/// A new event, before the store gives it an id.
#[derive(Debug, Clone)]
pub struct NewEvent {
    pub concept: String,
    pub kind: EventKind,
    pub reference: Option<String>,
    pub detail: Value,
}

impl NewEvent {
    pub fn new(concept: &str, kind: EventKind) -> Self {
        Self {
            concept: concept.to_string(),
            kind,
            reference: None,
            detail: Value::Null,
        }
    }

    pub fn reference(mut self, r: impl Into<String>) -> Self {
        self.reference = Some(r.into());
        self
    }

    pub fn detail(mut self, d: Value) -> Self {
        self.detail = d;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Misconception {
    pub id: String,
    pub concept: String,
    pub statement: String,
    /// `log` or `judge`.
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConceptState {
    Unknown,
    Claimed,
    Delegated,
    Shaky,
    Shown,
    Solid,
}

impl ConceptState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Claimed => "claimed",
            Self::Delegated => "delegated",
            Self::Shaky => "shaky",
            Self::Shown => "shown",
            Self::Solid => "solid",
        }
    }

    pub fn has_evidence(self) -> bool {
        matches!(self, Self::Shown | Self::Solid)
    }
}

/// One concept's state as of `now`, with what it rests on.
#[derive(Debug, Clone, PartialEq)]
pub struct ConceptView {
    pub state: ConceptState,
    pub passes: usize,
    pub fails: usize,
    /// The newest pass or `review_ok`.
    pub last_evidence: Option<i64>,
    /// Open misconception ids, oldest first.
    pub open_misconceptions: Vec<String>,
    /// The events the state rests on.
    pub event_ids: Vec<i64>,
    pub now: i64,
}

/// A5: folds one concept's events (in id order) into its state as of `now`.
pub fn concept_view(events: &[Event], now: i64) -> ConceptView {
    use ConceptState::*;
    let mut state = Unknown;
    let mut open: Vec<String> = Vec::new();
    let (mut passes, mut fails, mut flawed_passes) = (0, 0, 0);
    let mut last_evidence = None;
    let mut ids = Vec::new();
    for e in events {
        let before = state;
        match e.kind {
            EventKind::Claim if state == Unknown => state = Claimed,
            EventKind::Delegated if state == Unknown => state = Delegated,
            EventKind::MisconceptionShown => {
                if let Some(m) = &e.reference
                    && !open.contains(m)
                {
                    open.push(m.clone());
                }
            }
            EventKind::MisconceptionResolved => {
                if let Some(m) = &e.reference {
                    open.retain(|o| o != m);
                }
            }
            EventKind::Pass => {
                passes += 1;
                if e.detail["flawed"].as_bool() == Some(true) {
                    flawed_passes += 1;
                }
                last_evidence = Some(e.at);
                state = match state {
                    Unknown | Claimed | Delegated => Shown,
                    Shaky if open.is_empty() => Shown,
                    Shaky => Shaky,
                    Shown if passes >= 2 && flawed_passes >= 1 && open.is_empty() => Solid,
                    other => other,
                };
            }
            EventKind::Fail => {
                fails += 1;
                state = Shaky;
            }
            EventKind::ReviewMissed if matches!(state, Shown | Solid) => state = Shaky,
            EventKind::ReviewOk => last_evidence = Some(e.at),
            _ => {}
        }
        if state != before || matches!(e.kind, EventKind::Pass | EventKind::ReviewOk) {
            ids.push(e.id);
        }
    }
    if state == Solid && last_evidence.is_none_or(|t| now - t > AGE_SECS) {
        state = Shown;
    }
    ConceptView {
        state,
        passes,
        fails,
        last_evidence,
        open_misconceptions: open,
        event_ids: ids,
        now,
    }
}

/// Views for every concept in `events`, grouped by concept.
pub fn views(events: &[Event], now: i64) -> HashMap<String, ConceptView> {
    let mut by: HashMap<String, Vec<Event>> = HashMap::new();
    for e in events {
        by.entry(e.concept.clone()).or_default().push(e.clone());
    }
    by.into_iter()
        .map(|(c, mut es)| {
            es.sort_by_key(|e| e.id);
            (c, concept_view(&es, now))
        })
        .collect()
}

pub fn state_of(views: &HashMap<String, ConceptView>, concept: &str) -> ConceptState {
    views
        .get(concept)
        .map(|v| v.state)
        .unwrap_or(ConceptState::Unknown)
}

/// A part's status from its core concepts (A5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderstandingStatus {
    Understood,
    Known,
    Delegated,
    Open,
}

pub fn part_status(core: &[ConceptState], handed_off: bool) -> UnderstandingStatus {
    if !core.is_empty() && core.iter().all(|s| s.has_evidence()) {
        UnderstandingStatus::Understood
    } else if !core.is_empty()
        && core
            .iter()
            .all(|s| matches!(s, ConceptState::Claimed) || s.has_evidence())
    {
        UnderstandingStatus::Known
    } else if handed_off {
        UnderstandingStatus::Delegated
    } else {
        UnderstandingStatus::Open
    }
}

/// What the next candidate checks (A5, first match wins).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Targets {
    /// One or two core concepts.
    pub concepts: Vec<String>,
    /// T1: the candidate must be flawed and embody this misconception.
    pub misconception: Option<String>,
}

pub fn targets(core: &[String], views: &HashMap<String, ConceptView>) -> Option<Targets> {
    if core.is_empty() {
        return None;
    }
    // T1: an open misconception.
    for c in core {
        if let Some(m) = views.get(c).and_then(|v| v.open_misconceptions.first()) {
            return Some(Targets {
                concepts: vec![c.clone()],
                misconception: Some(m.clone()),
            });
        }
    }
    let pick = |pred: &dyn Fn(ConceptState) -> bool| -> Vec<String> {
        core.iter()
            .filter(|c| pred(state_of(views, c)))
            .take(2)
            .cloned()
            .collect()
    };
    // T2: no evidence; T3: shaky.
    for tier in [
        &(|s: ConceptState| {
            matches!(
                s,
                ConceptState::Unknown | ConceptState::Claimed | ConceptState::Delegated
            )
        }) as &dyn Fn(ConceptState) -> bool,
        &|s| s == ConceptState::Shaky,
    ] {
        let found = pick(tier);
        if !found.is_empty() {
            return Some(Targets {
                concepts: found,
                misconception: None,
            });
        }
    }
    // T4: fewest passes, then oldest evidence.
    let best = core.iter().min_by_key(|c| {
        let v = views.get(*c);
        (
            v.map(|v| v.passes).unwrap_or(0),
            v.and_then(|v| v.last_evidence).unwrap_or(0),
        )
    })?;
    Some(Targets {
        concepts: vec![best.clone()],
        misconception: None,
    })
}

/// A gap (A5): a prerequisite of a core concept that the part rests on
/// without evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    pub concept: String,
    pub state: ConceptState,
}

/// Walks the prerequisites of `core` (transitively). `covered` are concepts
/// an open part on the map will still work through; they aren't gaps.
pub fn gaps(
    core: &[String],
    prerequisites: &HashMap<String, Vec<String>>,
    views: &HashMap<String, ConceptView>,
    covered: &HashSet<String>,
) -> Vec<Gap> {
    let own: HashSet<&String> = core.iter().collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = core.to_vec();
    let mut out = Vec::new();
    while let Some(c) = stack.pop() {
        for need in prerequisites.get(&c).into_iter().flatten() {
            if !seen.insert(need.clone()) || own.contains(need) {
                continue;
            }
            stack.push(need.clone());
            let state = state_of(views, need);
            if matches!(
                state,
                ConceptState::Claimed
                    | ConceptState::Delegated
                    | ConceptState::Unknown
                    | ConceptState::Shaky
            ) && !covered.contains(need)
            {
                out.push(Gap {
                    concept: need.clone(),
                    state,
                });
            }
        }
    }
    out
}

/// Words of three or more letters, for lexical matching.
fn tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect()
}

/// The concepts a question is plainly about: every word of the label (or
/// of an alias) appears in it. Stricter than `lexical_candidates`, since a
/// question shares stray words with many concepts.
pub fn named_in<'a>(text: &str, concepts: &'a [Concept]) -> Vec<&'a Concept> {
    let have = tokens(text);
    concepts
        .iter()
        .filter(|c| {
            std::iter::once(&c.label).chain(&c.aliases).any(|name| {
                let want = tokens(name);
                !want.is_empty() && want.is_subset(&have)
            })
        })
        .collect()
}

/// The concept `text` names exactly: its id, label or an alias.
pub fn exact_match<'a>(text: &str, concepts: &'a [Concept]) -> Option<&'a Concept> {
    let slug = slugify(text);
    let lower = text.trim().to_lowercase();
    concepts.iter().find(|c| {
        c.id == slug
            || c.label.to_lowercase() == lower
            || c.aliases
                .iter()
                .any(|a| a.to_lowercase() == lower || slugify(a) == slug)
    })
}

/// Up to 20 concepts sharing a word with `text`, most shared first.
pub fn lexical_candidates<'a>(text: &str, concepts: &'a [Concept]) -> Vec<&'a Concept> {
    let want = tokens(text);
    let mut scored: Vec<(usize, &Concept)> = concepts
        .iter()
        .map(|c| {
            let mut have = tokens(&c.label);
            have.extend(tokens(&c.id));
            for a in &c.aliases {
                have.extend(tokens(a));
            }
            (want.intersection(&have).count(), c)
        })
        .filter(|(n, _)| *n > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    scored.into_iter().take(20).map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn ev(id: i64, kind: EventKind) -> Event {
        Event {
            id,
            concept: "c".into(),
            kind,
            reference: None,
            detail: Value::Null,
            at: 1000 + id,
        }
    }

    fn pass(id: i64, flawed: bool) -> Event {
        Event {
            detail: json!({"flawed": flawed}),
            ..ev(id, EventKind::Pass)
        }
    }

    fn misc(id: i64, kind: EventKind, m: &str) -> Event {
        Event {
            reference: Some(m.into()),
            ..ev(id, kind)
        }
    }

    fn st(events: &[Event]) -> ConceptState {
        concept_view(events, 2000).state
    }

    #[test]
    fn a_claim_never_becomes_evidence() {
        use ConceptState::*;
        assert_eq!(st(&[]), Unknown);
        assert_eq!(st(&[ev(1, EventKind::Claim)]), Claimed);
        assert_eq!(st(&[ev(1, EventKind::Delegated)]), Delegated);
        assert_eq!(
            st(&[ev(1, EventKind::Claim), ev(2, EventKind::Touched)]),
            Claimed
        );
        assert_eq!(st(&[ev(1, EventKind::Claim), pass(2, false)]), Shown);
        assert_eq!(
            st(&[ev(1, EventKind::Claim), ev(2, EventKind::Fail)]),
            Shaky
        );
        // A claim after evidence changes nothing.
        assert_eq!(st(&[pass(1, false), ev(2, EventKind::Claim)]), Shown);
    }

    #[test]
    fn solid_needs_two_passes_one_flawed_and_no_open_misconception() {
        use ConceptState::*;
        assert_eq!(st(&[pass(1, false), pass(2, false)]), Shown);
        assert_eq!(st(&[pass(1, false), pass(2, true)]), Solid);
        assert_eq!(
            st(&[
                pass(1, true),
                misc(2, EventKind::MisconceptionShown, "m1"),
                pass(3, false)
            ]),
            Shown
        );
        assert_eq!(
            st(&[
                pass(1, false),
                pass(2, true),
                ev(3, EventKind::ReviewMissed)
            ]),
            Shaky
        );
    }

    #[test]
    fn a_log_misconception_never_demotes_but_blocks_recovery() {
        use ConceptState::*;
        assert_eq!(
            st(&[pass(1, false), misc(2, EventKind::MisconceptionShown, "m")]),
            Shown
        );
        let shaky_with_m = [
            ev(1, EventKind::Fail),
            misc(2, EventKind::MisconceptionShown, "m"),
        ];
        assert_eq!(
            st(&[shaky_with_m.clone().to_vec(), vec![pass(3, false)]].concat()),
            Shaky
        );
        assert_eq!(
            st(&[
                shaky_with_m.to_vec(),
                vec![
                    misc(3, EventKind::MisconceptionResolved, "m"),
                    pass(4, true)
                ]
            ]
            .concat()),
            Shown
        );
    }

    #[test]
    fn solid_cools_without_fresh_evidence() {
        let events = [pass(1, false), pass(2, true)];
        assert_eq!(concept_view(&events, 2000).state, ConceptState::Solid);
        assert_eq!(
            concept_view(&events, 1002 + AGE_SECS + 1).state,
            ConceptState::Shown
        );
    }

    #[test]
    fn part_status_targets_and_gaps() {
        use ConceptState::*;
        assert_eq!(
            part_status(&[Shown, Solid], false),
            UnderstandingStatus::Understood
        );
        assert_eq!(
            part_status(&[Shown, Claimed], false),
            UnderstandingStatus::Known
        );
        assert_eq!(
            part_status(&[Shown, Unknown], true),
            UnderstandingStatus::Delegated
        );
        assert_eq!(
            part_status(&[Shown, Unknown], false),
            UnderstandingStatus::Open
        );

        let mut vs = HashMap::new();
        vs.insert("a".to_string(), concept_view(&[pass(1, false)], 2000));
        let core = vec!["a".to_string(), "b".to_string()];
        assert_eq!(targets(&core, &vs).unwrap().concepts, vec!["b".to_string()]);

        let mut with_m = vs.clone();
        with_m.insert(
            "a".into(),
            concept_view(&[misc(1, EventKind::MisconceptionShown, "m9")], 2000),
        );
        let t = targets(&core, &with_m).unwrap();
        assert_eq!(
            (t.concepts, t.misconception),
            (vec!["a".into()], Some("m9".into()))
        );

        let mut prereq = HashMap::new();
        prereq.insert("a".to_string(), vec!["x".to_string(), "b".to_string()]);
        prereq.insert("x".to_string(), vec!["y".to_string()]);
        let covered: HashSet<String> = ["y".to_string()].into();
        let g = gaps(&core, &prereq, &vs, &covered);
        assert_eq!(
            g,
            vec![Gap {
                concept: "x".into(),
                state: Unknown
            }]
        );
    }

    #[test]
    fn matching_by_name_and_words() {
        let cs = vec![
            Concept {
                id: "d-separation".into(),
                label: "d-separation".into(),
                aliases: vec!["d-sep".into()],
                created_by: "map".into(),
            },
            Concept {
                id: "colliders".into(),
                label: "Colliders in a DAG".into(),
                aliases: vec![],
                created_by: "map".into(),
            },
        ];
        assert_eq!(exact_match("D-Sep", &cs).unwrap().id, "d-separation");
        assert_eq!(
            exact_match("colliders in a dag", &cs).unwrap().id,
            "colliders"
        );
        assert!(exact_match("collider bias", &cs).is_none());
        let lex = lexical_candidates("DAG collider orientation", &cs);
        assert_eq!(lex[0].id, "colliders");
        assert_eq!(
            slugify("  Conditional Independence! "),
            "conditional-independence"
        );
    }
}
