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
    /// Short topic/domain tag (e.g. `"electromagnetism"`, `"rust-async"`), never raw paths.
    #[serde(default)]
    pub domain: String,
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
    /// A claim you took back with `/undo` (§15). `ref` is the claim's event
    /// id. It withdraws only a claim: aimed at anything else it does
    /// nothing, so an undo never raises the record.
    Retracted,
    /// A next step Pathfinder showed you (§16): `ref` is the part,
    /// `detail` `{"kind": "wide-example", "steered": bool}`. Activity, never
    /// evidence.
    Step,
    /// Explicit opt-out in this workspace: don't assume prior knowledge from
    /// other workspaces for this concept. Append-only, never evidence.
    Revisit,
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
            Self::Retracted => "retracted",
            Self::Step => "step",
            Self::Revisit => "revisit",
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
            "retracted" => Self::Retracted,
            "step" => Self::Step,
            "revisit" => Self::Revisit,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: i64,
    pub workspace: String,
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

/// Cross-workspace prior understanding when the current workspace has no local
/// evidence (`!state.has_evidence()`). Never counts as local evidence or A7
/// `vouched` provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PriorState {
    /// Shown (passed at least once, or cooled from solid after `AGE_SECS` of
    /// global inactivity) in another workspace.
    ShownElsewhere { workspace: String },
    /// Solid in another workspace and fresh within `AGE_SECS` globally.
    SolidElsewhere { workspace: String },
}

/// One concept's state as of `now`, with what it rests on.
///
/// `state`, `passes`, `fails`, `last_evidence`, and `event_ids` strictly
/// reflect the current (local) workspace so A7 ledger/coverage invariants never
/// conflate cross-workspace `prior` knowledge with local evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct ConceptView {
    pub state: ConceptState,
    pub passes: usize,
    pub fails: usize,
    /// The newest local pass or `review_ok`.
    pub last_evidence: Option<i64>,
    /// Open misconception ids, oldest first (global when computed via
    /// [`concept_view_in_workspace`]).
    pub open_misconceptions: Vec<String>,
    /// The local events the state rests on.
    pub event_ids: Vec<i64>,
    pub now: i64,
    /// Cross-workspace prior understanding when `!state.has_evidence()`,
    /// suppressed if a local [`EventKind::Revisit`] occurred after the latest
    /// elsewhere evidence.
    pub prior: Option<PriorState>,
}

/// A5: folds one concept's single-workspace events (in id order) into its state
/// as of `now`.
pub fn concept_view(events: &[Event], now: i64) -> ConceptView {
    use ConceptState::*;
    let mut state = Unknown;
    let mut open: Vec<String> = Vec::new();
    let (mut passes, mut fails, mut flawed_passes) = (0, 0, 0);
    let mut last_evidence = None;
    let mut ids = Vec::new();
    let withdrawn = retracted_claims(events);
    for e in events {
        if e.kind == EventKind::Claim && withdrawn.contains(&e.id) {
            continue;
        }
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
        prior: None,
    }
}

#[derive(Default)]
struct WorkspaceFold {
    state: Option<ConceptState>,
    passes: usize,
    fails: usize,
    flawed_passes: usize,
    last_evidence: Option<i64>,
    last_evidence_id: Option<i64>,
    event_ids: Vec<i64>,
}

impl WorkspaceFold {
    fn state(&self) -> ConceptState {
        self.state.unwrap_or(ConceptState::Unknown)
    }
}

fn active_prior(
    workspaces: &HashMap<&str, WorkspaceFold>,
    current_workspace: &str,
    global_last_evidence: Option<i64>,
    latest_elsewhere_evidence_id: Option<i64>,
    local_last_revisit_id: Option<i64>,
    at: i64,
) -> Option<PriorState> {
    if let Some(rev_id) = local_last_revisit_id
        && latest_elsewhere_evidence_id.is_none_or(|ev_id| rev_id > ev_id)
    {
        return None;
    }
    let global_fresh = global_last_evidence.is_some_and(|t| at - t <= AGE_SECS);
    // Pick the strongest prior: SolidElsewhere > ShownElsewhere, then newest
    // evidence timestamp/id, then lexicographically smallest workspace for
    // determinism.
    let mut best: Option<(bool, i64, i64, &str)> = None;
    for (&ws, fold) in workspaces {
        if ws == current_workspace {
            continue;
        }
        let is_solid = match fold.state() {
            ConceptState::Solid => global_fresh,
            ConceptState::Shown => false,
            _ => continue,
        };
        let ev_at = fold.last_evidence.unwrap_or(i64::MIN);
        let ev_id = fold.last_evidence_id.unwrap_or(i64::MIN);
        let better = match best {
            None => true,
            Some((b_solid, b_at, b_id, b_ws)) => {
                (is_solid, ev_at, ev_id) > (b_solid, b_at, b_id)
                    || ((is_solid, ev_at, ev_id) == (b_solid, b_at, b_id) && ws < b_ws)
            }
        };
        if better {
            best = Some((is_solid, ev_at, ev_id, ws));
        }
    }
    best.map(|(is_solid, _, _, ws)| {
        if is_solid {
            PriorState::SolidElsewhere {
                workspace: ws.to_string(),
            }
        } else {
            PriorState::ShownElsewhere {
                workspace: ws.to_string(),
            }
        }
    })
}

/// Folds one concept's global events (across all workspaces) into its view for
/// `current_workspace` as of `now`:
/// - Local state (`state`, `passes`, `fails`, `last_evidence`, `event_ids`)
///   comes strictly from `current_workspace` events (preserving A7 `vouched`
///   safety).
/// - Open misconceptions travel and resolve globally across workspaces.
/// - Any `Pass` or `ReviewOk` in any workspace refreshes the 30-day `AGE_SECS`
///   recency clock for both local `Solid` and `PriorState::SolidElsewhere`.
/// - When `!state.has_evidence()`, `prior` surfaces `SolidElsewhere` or
///   `ShownElsewhere` from other workspaces unless a local `Revisit` occurred
///   after the latest elsewhere evidence.
/// - 1-pass fast promotion: if active `SolidElsewhere` holds immediately before
///   a local `Pass` (with no open misconceptions and local state not `Shaky`),
///   that single local `Pass` promotes local state directly to `Solid`.
pub fn concept_view_in_workspace(
    events: &[Event],
    current_workspace: &str,
    now: i64,
) -> ConceptView {
    use ConceptState::*;
    let mut sorted: Vec<&Event> = events.iter().collect();
    sorted.sort_by_key(|e| e.id);

    let withdrawn = retracted_claims(events);
    let mut workspaces: HashMap<&str, WorkspaceFold> = HashMap::new();
    let mut open: Vec<String> = Vec::new();
    let mut global_last_evidence: Option<i64> = None;
    let mut latest_elsewhere_evidence_id: Option<i64> = None;
    let mut local_last_revisit_id: Option<i64> = None;

    for e in sorted {
        if e.kind == EventKind::Claim && withdrawn.contains(&e.id) {
            continue;
        }
        match e.kind {
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
            EventKind::Revisit if e.workspace == current_workspace => {
                local_last_revisit_id = Some(e.id);
            }
            _ => {}
        }

        let solid_elsewhere_before_pass =
            if e.kind == EventKind::Pass && e.workspace == current_workspace {
                matches!(
                    active_prior(
                        &workspaces,
                        current_workspace,
                        global_last_evidence,
                        latest_elsewhere_evidence_id,
                        local_last_revisit_id,
                        e.at,
                    ),
                    Some(PriorState::SolidElsewhere { .. })
                )
            } else {
                false
            };

        let fold = workspaces.entry(e.workspace.as_str()).or_default();
        let before = fold.state();
        let mut state = before;
        match e.kind {
            EventKind::Claim if state == Unknown => state = Claimed,
            EventKind::Delegated if state == Unknown => state = Delegated,
            EventKind::Pass => {
                fold.passes += 1;
                if e.detail["flawed"].as_bool() == Some(true) {
                    fold.flawed_passes += 1;
                }
                fold.last_evidence = Some(fold.last_evidence.map_or(e.at, |p| p.max(e.at)));
                fold.last_evidence_id = Some(e.id);
                global_last_evidence = Some(global_last_evidence.map_or(e.at, |p| p.max(e.at)));
                if e.workspace != current_workspace {
                    latest_elsewhere_evidence_id = Some(e.id);
                }
                state = match state {
                    Unknown | Claimed | Delegated => {
                        if solid_elsewhere_before_pass && open.is_empty() {
                            Solid
                        } else {
                            Shown
                        }
                    }
                    Shaky if open.is_empty() => Shown,
                    Shaky => Shaky,
                    Shown if fold.passes >= 2 && fold.flawed_passes >= 1 && open.is_empty() => {
                        Solid
                    }
                    other => other,
                };
            }
            EventKind::Fail => {
                fold.fails += 1;
                state = Shaky;
            }
            EventKind::ReviewMissed if matches!(state, Shown | Solid) => state = Shaky,
            EventKind::ReviewOk => {
                fold.last_evidence = Some(fold.last_evidence.map_or(e.at, |p| p.max(e.at)));
                fold.last_evidence_id = Some(e.id);
                global_last_evidence = Some(global_last_evidence.map_or(e.at, |p| p.max(e.at)));
                if e.workspace != current_workspace {
                    latest_elsewhere_evidence_id = Some(e.id);
                }
            }
            _ => {}
        }
        fold.state = Some(state);
        if state != before || matches!(e.kind, EventKind::Pass | EventKind::ReviewOk) {
            fold.event_ids.push(e.id);
        }
    }

    let local = workspaces.get(current_workspace);
    let mut local_state = local.map(WorkspaceFold::state).unwrap_or(Unknown);
    if local_state == Solid && global_last_evidence.is_none_or(|t| now - t > AGE_SECS) {
        local_state = Shown;
    }
    let prior = if !local_state.has_evidence() {
        active_prior(
            &workspaces,
            current_workspace,
            global_last_evidence,
            latest_elsewhere_evidence_id,
            local_last_revisit_id,
            now,
        )
    } else {
        None
    };

    ConceptView {
        state: local_state,
        passes: local.map(|f| f.passes).unwrap_or(0),
        fails: local.map(|f| f.fails).unwrap_or(0),
        last_evidence: local.and_then(|f| f.last_evidence),
        open_misconceptions: open,
        event_ids: local.map(|f| f.event_ids.clone()).unwrap_or_default(),
        now,
        prior,
    }
}

/// The ids `retracted` events point at that are claims (§15) in the same
/// workspace. A retraction aimed at any other kind is ignored: undo only
/// lowers the record.
pub fn retracted_claims(events: &[Event]) -> HashSet<i64> {
    let claims: HashMap<i64, &str> = events
        .iter()
        .filter(|e| e.kind == EventKind::Claim)
        .map(|e| (e.id, e.workspace.as_str()))
        .collect();
    events
        .iter()
        .filter(|e| e.kind == EventKind::Retracted)
        .filter_map(|e| {
            let id = e.reference.as_deref()?.parse::<i64>().ok()?;
            (claims.get(&id) == Some(&e.workspace.as_str())).then_some(id)
        })
        .collect()
}

/// Views for every concept in `events`, grouped by concept (single-workspace).
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

/// Views for every concept in `events` (across all workspaces) evaluated in
/// `current_workspace` as of `now`.
pub fn views_in_workspace(
    events: &[Event],
    current_workspace: &str,
    now: i64,
) -> HashMap<String, ConceptView> {
    let mut by: HashMap<String, Vec<Event>> = HashMap::new();
    for e in events {
        by.entry(e.concept.clone()).or_default().push(e.clone());
    }
    by.into_iter()
        .map(|(c, mut es)| {
            es.sort_by_key(|e| e.id);
            (c, concept_view_in_workspace(&es, current_workspace, now))
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

/// Words that carry no meaning in a concept's name.
const NAME_FILLER: &[&str] = &[
    "a", "an", "the", "of", "for", "between", "in", "on", "to", "and", "with", "by", "two", "its",
    "from", "via", "using", "der", "die", "das", "des", "von", "und", "zwischen", "zweier",
];

/// A concept name's meaningful words, in order: lowercased, filler left
/// out, a plural `s` dropped ("Gaussian mixtures" is `gaussian mixture`).
fn name_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .filter(|w| !NAME_FILLER.contains(&w.as_str()))
        .map(|w| {
            let n = w.chars().count();
            if n > 3 && w.ends_with('s') && !w.ends_with("ss") {
                w[..w.len() - 1].to_string()
            } else {
                w
            }
        })
        .collect()
}

/// `words` with each abbreviation in it spelled out as `other` spells it:
/// "gmm" becomes `gaussian mixture model` when `other` has those words in
/// a row, or `gaussian mixture` (a trailing "model" is often left out).
fn spelled_out(words: &[String], other: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for w in words {
        let short = (2..=5).contains(&w.chars().count())
            && w.chars().all(|c| c.is_ascii_alphabetic())
            && !other.contains(w);
        let run = short
            .then(|| {
                (1..=w.len()).rev().find_map(|n| {
                    other.windows(n).find(|run| {
                        let initials: String =
                            run.iter().filter_map(|r| r.chars().next()).collect();
                        initials == *w
                            || (n + 1 == w.len() && w.ends_with('m') && initials == w[..n])
                    })
                })
            })
            .flatten();
        match run {
            Some(run) => out.extend(run.iter().cloned()),
            None => out.push(w.clone()),
        }
    }
    out
}

/// Words that make a name's word order part of its meaning: "probability
/// of X given Y" is not "probability of Y given X", "KL divergence from P
/// to Q" is not the one from Q to P, "sum of squares" is not "square of
/// the sum".
const NAME_RELATIONS: &[&str] = &[
    "of",
    "given",
    "from",
    "to",
    "into",
    "over",
    "under",
    "vs",
    "versus",
    "per",
    "than",
    "by",
    "on",
    "against",
    "wrt",
    "conditioned",
    "relative",
    "minus",
    "divided",
    "times",
    "after",
    "before",
    "von",
    "nach",
    "gegen",
    "unter",
    "über",
    "pro",
];

/// Whether two names say the same concept in other words: the same
/// meaningful words, with plurals, filler and spelled-out abbreviations
/// evened out ("L2 distance between GMMs" and "L2 distance of Gaussian
/// mixtures"). The words must come in the same order, unless neither name
/// holds a word that makes order matter ([`NAME_RELATIONS`]): swapping a
/// relation's arguments is another idea. Never a related idea: one word
/// more or less is a different name.
pub fn same_name(a: &str, b: &str) -> bool {
    let (wa, wb) = (name_words(a), name_words(b));
    if wa.is_empty() || wb.is_empty() {
        return false;
    }
    let (oa, ob) = (spelled_out(&wa, &wb), spelled_out(&wb, &wa));
    let relational = |t: &str| {
        t.split(|c: char| !c.is_alphanumeric())
            .any(|w| NAME_RELATIONS.contains(&w.to_lowercase().as_str()))
    };
    let any_order = !relational(a) && !relational(b);
    let shape = |w: &[String]| -> Vec<String> {
        let mut w = w.to_vec();
        if any_order {
            w.sort();
            w.dedup();
        }
        w
    };
    let (sa, sb) = (shape(&oa), shape(&ob));
    let model = |s: &[String]| {
        s.iter()
            .filter(|w| *w != "model")
            .cloned()
            .collect::<Vec<_>>()
    };
    sa == sb
        || (model(&sa) == model(&sb) && !model(&sa).is_empty() && sa.len().abs_diff(sb.len()) <= 1)
}

/// The recorded concept `name` says in other words (see [`same_name`]):
/// by its label or an alias. None when no one or more than one does.
pub fn same_named<'a>(name: &str, concepts: &'a [Concept]) -> Option<&'a Concept> {
    let found: Vec<&Concept> = concepts
        .iter()
        .filter(|c| {
            std::iter::once(&c.label)
                .chain(&c.aliases)
                .any(|n| same_name(name, n))
        })
        .collect();
    match found.as_slice() {
        [one] => Some(one),
        _ => None,
    }
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

/// How wide a next step is (§16): an orienting look at a concept you have
/// no evidence on, or a narrow one at a concept that is shaky.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Wide,
    Narrow,
}

/// Where a next step starts (§16): a concrete example, what the concept is
/// for, or how it works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Angle {
    Example,
    Purpose,
    Mechanism,
}

impl Angle {
    pub const ALL: [Angle; 3] = [Angle::Example, Angle::Purpose, Angle::Mechanism];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Example => "example",
            Self::Purpose => "purpose",
            Self::Mechanism => "mechanism",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == s)
    }
}

/// The kind of a next step: its scope and its angle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StepKind {
    pub scope: Scope,
    pub angle: Angle,
}

impl StepKind {
    /// `wide-example`, `narrow-mechanism`, …
    pub fn as_str(self) -> String {
        let scope = match self.scope {
            Scope::Wide => "wide",
            Scope::Narrow => "narrow",
        };
        format!("{scope}-{}", self.angle.as_str())
    }

    pub fn parse(s: &str) -> Option<Self> {
        let (scope, angle) = s.split_once('-')?;
        let scope = match scope {
            "wide" => Scope::Wide,
            "narrow" => Scope::Narrow,
            _ => return None,
        };
        Some(Self {
            scope,
            angle: Angle::parse(angle)?,
        })
    }
}

/// Per step kind (`wide-example`, …): (steps that led to a pass, steps
/// that led to any verdict).
pub type StepStats = std::collections::BTreeMap<String, (u32, u32)>;

/// Judged steps an angle needs before Pathfinder leans on it.
pub const LEAN_AFTER: u32 = 2;

/// §16: the next step's kind for a concept, from its state, your steer and
/// the kinds that led you to passes before. None when the concept already
/// has evidence and nothing open on it: no step is needed.
pub fn choose_step(
    view: &ConceptView,
    steer: Option<Angle>,
    stats: &StepStats,
) -> Option<StepKind> {
    let scope = if !view.open_misconceptions.is_empty() || view.state == ConceptState::Shaky {
        Scope::Narrow
    } else if matches!(
        view.state,
        ConceptState::Unknown | ConceptState::Claimed | ConceptState::Delegated
    ) {
        Scope::Wide
    } else {
        return None;
    };
    let default = match scope {
        Scope::Wide => Angle::Purpose,
        Scope::Narrow => Angle::Mechanism,
    };
    let angle = steer.unwrap_or_else(|| {
        let rate = |a: Angle| {
            let kind = StepKind { scope, angle: a }.as_str();
            stats
                .get(&kind)
                .filter(|(_, judged)| *judged >= LEAN_AFTER)
                .map(|(passes, judged)| *passes as f64 / *judged as f64)
        };
        let best = Angle::ALL
            .into_iter()
            .filter_map(|a| rate(a).map(|r| (a, r)))
            .fold(None::<(Angle, f64)>, |best, (a, r)| match best {
                Some((_, b)) if b >= r => best,
                _ => Some((a, r)),
            });
        match best {
            // Only something that worked moves it off the default, and a
            // tie goes to the default.
            Some((a, r)) if r > 0.0 && rate(default).is_none_or(|d| d < r) => a,
            _ => default,
        }
    });
    Some(StepKind { scope, angle })
}

/// §16: which step kinds led to passes. A `step` on a concept counts only
/// once you took it up: you wrote about that concept in your log, or
/// opened or searched a source on it, before the next verdict. A verdict
/// on a candidate that merely followed a step you never took says nothing
/// about the step. The next `pass` or `fail` on that concept, before the
/// next `step` on it, is then the step's verdict; a later step supersedes
/// an earlier one. Activity only: nothing here is evidence.
pub fn step_stats(events: &[Event]) -> StepStats {
    let mut sorted: Vec<&Event> = events.iter().collect();
    sorted.sort_by_key(|e| e.id);
    // Per concept: the step shown last, and whether you took it up.
    let mut open: HashMap<&str, (String, bool)> = HashMap::new();
    let mut out = StepStats::new();
    for e in sorted {
        match e.kind {
            EventKind::Step => {
                let Some(kind) = e.detail["kind"]
                    .as_str()
                    .filter(|k| StepKind::parse(k).is_some())
                else {
                    continue;
                };
                out.entry(kind.to_string()).or_default();
                open.insert(e.concept.as_str(), (kind.to_string(), false));
            }
            EventKind::Touched | EventKind::Read | EventKind::Searched => {
                if let Some((_, taken)) = open.get_mut(e.concept.as_str()) {
                    *taken = true;
                }
            }
            EventKind::Pass | EventKind::Fail => {
                if let Some((kind, true)) = open.remove(e.concept.as_str()) {
                    let entry = out.entry(kind).or_default();
                    entry.1 += 1;
                    if e.kind == EventKind::Pass {
                        entry.0 += 1;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// How many steps you steered yourself ("give me an example first").
pub fn steered_steps(events: &[Event]) -> u32 {
    events
        .iter()
        .filter(|e| e.kind == EventKind::Step && e.detail["steered"].as_bool() == Some(true))
        .count() as u32
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn ev(id: i64, kind: EventKind) -> Event {
        Event {
            id,
            workspace: "/w".into(),
            concept: "c".into(),
            kind,
            reference: None,
            detail: Value::Null,
            at: 1000 + id,
        }
    }

    fn ev_in(ws: &str, id: i64, kind: EventKind, at: i64) -> Event {
        Event {
            id,
            workspace: ws.into(),
            concept: "c".into(),
            kind,
            reference: None,
            detail: Value::Null,
            at,
        }
    }

    fn pass_in(ws: &str, id: i64, flawed: bool, at: i64) -> Event {
        Event {
            detail: json!({"flawed": flawed}),
            ..ev_in(ws, id, EventKind::Pass, at)
        }
    }

    fn misc_in(ws: &str, id: i64, kind: EventKind, m: &str, at: i64) -> Event {
        Event {
            reference: Some(m.into()),
            ..ev_in(ws, id, kind, at)
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
                domain: String::new(),
                created_by: "map".into(),
            },
            Concept {
                id: "colliders".into(),
                label: "Colliders in a DAG".into(),
                aliases: vec![],
                domain: String::new(),
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
    fn retract(id: i64, of: i64) -> Event {
        Event {
            reference: Some(of.to_string()),
            ..ev(id, EventKind::Retracted)
        }
    }

    fn step(id: i64, concept: &str, kind: &str) -> Event {
        Event {
            concept: concept.into(),
            detail: json!({"kind": kind, "steered": false}),
            ..ev(id, EventKind::Step)
        }
    }

    fn on(concept: &str, e: Event) -> Event {
        Event {
            concept: concept.into(),
            ..e
        }
    }

    #[test]
    fn a_retracted_claim_is_ignored() {
        use ConceptState::*;
        assert_eq!(st(&[ev(1, EventKind::Claim), retract(2, 1)]), Unknown);
        // A later claim still counts.
        assert_eq!(
            st(&[
                ev(1, EventKind::Claim),
                retract(2, 1),
                ev(3, EventKind::Claim)
            ]),
            Claimed
        );
    }

    #[test]
    fn a_retraction_never_drops_evidence() {
        use ConceptState::*;
        assert_eq!(st(&[pass(1, false), retract(2, 1)]), Shown);
        assert_eq!(st(&[ev(1, EventKind::Fail), retract(2, 1)]), Shaky);
        let v = concept_view(
            &[misc(1, EventKind::MisconceptionShown, "m"), retract(2, 1)],
            2000,
        );
        assert_eq!(v.open_misconceptions, vec!["m".to_string()]);
        assert_eq!(st(&[ev(1, EventKind::Delegated), retract(2, 1)]), Delegated);
        assert_eq!(
            st(&[
                pass(1, false),
                ev(2, EventKind::ReviewMissed),
                retract(3, 2)
            ]),
            Shaky
        );
    }

    #[test]
    fn a_retraction_of_an_unknown_id_does_nothing() {
        use ConceptState::*;
        assert_eq!(st(&[ev(1, EventKind::Claim), retract(2, 99)]), Claimed);
        let junk = Event {
            reference: Some("not-an-id".into()),
            ..ev(2, EventKind::Retracted)
        };
        assert_eq!(st(&[ev(1, EventKind::Claim), junk]), Claimed);
    }

    #[test]
    fn step_events_are_not_evidence() {
        use ConceptState::*;
        assert_eq!(st(&[step(1, "c", "wide-purpose")]), Unknown);
        assert_eq!(
            st(&[step(1, "c", "wide-purpose"), ev(2, EventKind::Claim)]),
            Claimed
        );
        assert_eq!(EventKind::parse("retracted"), Some(EventKind::Retracted));
        assert_eq!(
            EventKind::parse(EventKind::Step.as_str()),
            Some(EventKind::Step)
        );
    }

    #[test]
    fn step_stats_count_the_next_verdict() {
        let took = |id| on("a", ev(id, EventKind::Touched));
        let one = step_stats(&[
            step(1, "a", "wide-purpose"),
            took(2),
            on("a", pass(3, false)),
        ]);
        assert_eq!(one.get("wide-purpose"), Some(&(1, 1)));

        let superseded = step_stats(&[
            step(1, "a", "wide-example"),
            took(2),
            step(3, "a", "narrow-mechanism"),
            on("a", ev(4, EventKind::Read)),
            on("a", ev(5, EventKind::Fail)),
        ]);
        assert_eq!(superseded.get("narrow-mechanism"), Some(&(0, 1)));
        assert_eq!(superseded.get("wide-example"), Some(&(0, 0)));

        let unjudged = step_stats(&[
            step(1, "a", "wide-purpose"),
            took(2),
            on("b", pass(3, false)),
        ]);
        assert_eq!(unjudged.get("wide-purpose"), Some(&(0, 0)));

        // Only the first verdict after a step counts.
        let twice = step_stats(&[
            step(1, "a", "wide-purpose"),
            took(2),
            on("a", pass(3, false)),
            on("a", pass(4, false)),
        ]);
        assert_eq!(twice.get("wide-purpose"), Some(&(1, 1)));
    }

    #[test]
    fn the_same_name_in_other_words() {
        assert!(same_name(
            "L2 distance between Gaussian mixtures",
            "L2 distance of two GMMs"
        ));
        assert!(same_name("Gaussian mixture models", "GMM"));
        assert!(same_name(
            "Bayesian fusion of GMMs",
            "Bayesian fusion of Gaussian mixtures"
        ));
        assert!(same_name("Command routing", "command routing"));
        // Order is free when no word in the name makes it matter.
        assert!(same_name(
            "Gaussian mixture L2 distance",
            "L2 distance between Gaussian mixtures"
        ));
        // A related idea is not the same one.
        assert!(!same_name("L2 distance", "L2 distance gradient"));
        assert!(!same_name("Bayesian fusion", "Gaussian product"));
        assert!(!same_name("VAT", "Tariff"));
        assert!(!same_name("the", "of"));
    }

    /// Adversarial: `same_name` compares word sets, so swapping the
    /// arguments of a relation merges two different ideas, and a pass on
    /// one is credited to the other without the decision model being asked.
    #[test]
    fn adv_swapped_words_are_not_the_same_idea() {
        let pairs = [
            ("probability of X given Y", "probability of Y given X"),
            ("sum of squares", "square of the sum"),
            ("KL divergence from P to Q", "KL divergence from Q to P"),
            ("variance of the mean", "mean of the variance"),
        ];
        let merged: Vec<_> = pairs.iter().filter(|(a, b)| same_name(a, b)).collect();
        assert!(merged.is_empty(), "different ideas merged: {merged:?}");
    }

    #[test]
    fn a_step_you_never_took_up_led_to_nothing() {
        // A candidate judged right after a step you didn't follow: the
        // pass came from the check, not from the step.
        let ignored = step_stats(&[step(1, "a", "wide-purpose"), on("a", pass(2, false))]);
        assert_eq!(ignored.get("wide-purpose"), Some(&(0, 0)));
        // Activity on another concept doesn't take it up either.
        let elsewhere = step_stats(&[
            step(1, "a", "wide-purpose"),
            on("b", ev(2, EventKind::Touched)),
            on("a", pass(3, false)),
        ]);
        assert_eq!(elsewhere.get("wide-purpose"), Some(&(0, 0)));
        // Searching a source on it does.
        let searched = step_stats(&[
            step(1, "a", "wide-purpose"),
            on("a", ev(2, EventKind::Searched)),
            on("a", pass(3, false)),
        ]);
        assert_eq!(searched.get("wide-purpose"), Some(&(1, 1)));
    }

    #[test]
    fn choose_step_follows_state_steer_and_history() {
        let none = StepStats::new();
        let view = |events: &[Event]| concept_view(events, 2000);
        let kind = |v: &ConceptView, steer, stats: &StepStats| {
            choose_step(v, steer, stats).map(|k| k.as_str())
        };
        assert_eq!(
            kind(&view(&[]), None, &none).as_deref(),
            Some("wide-purpose")
        );
        assert_eq!(
            kind(&view(&[ev(1, EventKind::Fail)]), None, &none).as_deref(),
            Some("narrow-mechanism")
        );
        let misconceived = view(&[pass(1, false), misc(2, EventKind::MisconceptionShown, "m")]);
        assert_eq!(
            choose_step(&misconceived, None, &none).map(|k| k.scope),
            Some(Scope::Narrow)
        );
        assert_eq!(kind(&view(&[pass(1, false)]), None, &none), None);
        assert_eq!(
            kind(&view(&[]), Some(Angle::Example), &none).as_deref(),
            Some("wide-example")
        );

        let mut stats = StepStats::new();
        stats.insert("wide-example".into(), (2, 2));
        stats.insert("wide-purpose".into(), (0, 2));
        assert_eq!(
            kind(&view(&[]), None, &stats).as_deref(),
            Some("wide-example")
        );
        // A steer still wins.
        assert_eq!(
            kind(&view(&[]), Some(Angle::Mechanism), &stats).as_deref(),
            Some("wide-mechanism")
        );
        // One judged step doesn't move it.
        let mut thin = StepStats::new();
        thin.insert("wide-example".into(), (1, 1));
        assert_eq!(
            kind(&view(&[]), None, &thin).as_deref(),
            Some("wide-purpose")
        );
        // A tie goes to the default.
        let mut tie = StepStats::new();
        tie.insert("wide-example".into(), (1, 2));
        tie.insert("wide-purpose".into(), (1, 2));
        assert_eq!(
            kind(&view(&[]), None, &tie).as_deref(),
            Some("wide-purpose")
        );
        assert_eq!(
            StepKind::parse("narrow-example")
                .map(|k| k.as_str())
                .as_deref(),
            Some("narrow-example")
        );
        assert_eq!(StepKind::parse("tall-example"), None);
    }

    #[test]
    fn prior_state_surfaces_elsewhere_evidence_without_affecting_local_has_evidence() {
        // /w1 is Solid; /w2 has no local events.
        let events = [
            pass_in("/w1", 1, false, 1000),
            pass_in("/w1", 2, true, 1001),
        ];
        let v_w2 = concept_view_in_workspace(&events, "/w2", 2000);
        // A7 safety: local state is Unknown and has_evidence() is false.
        assert_eq!(v_w2.state, ConceptState::Unknown);
        assert!(!v_w2.state.has_evidence());
        assert_eq!(v_w2.passes, 0);
        assert!(v_w2.event_ids.is_empty());
        assert_eq!(
            v_w2.prior,
            Some(PriorState::SolidElsewhere {
                workspace: "/w1".into()
            })
        );

        // /w1 only Shown -> ShownElsewhere in /w2.
        let shown_only = [pass_in("/w1", 1, false, 1000)];
        let v_shown = concept_view_in_workspace(&shown_only, "/w2", 2000);
        assert_eq!(v_shown.state, ConceptState::Unknown);
        assert_eq!(
            v_shown.prior,
            Some(PriorState::ShownElsewhere {
                workspace: "/w1".into()
            })
        );

        // SolidElsewhere beats ShownElsewhere when multiple workspaces have evidence.
        let mixed = [
            pass_in("/w1", 1, false, 1000),
            pass_in("/w3", 2, false, 1001),
            pass_in("/w3", 3, true, 1002),
        ];
        assert_eq!(
            concept_view_in_workspace(&mixed, "/w2", 2000).prior,
            Some(PriorState::SolidElsewhere {
                workspace: "/w3".into()
            })
        );

        // If /w1 becomes Shaky, it no longer supplies a prior.
        let failed = [
            pass_in("/w1", 1, false, 1000),
            pass_in("/w1", 2, true, 1001),
            ev_in("/w1", 3, EventKind::Fail, 1002),
        ];
        assert_eq!(concept_view_in_workspace(&failed, "/w2", 2000).prior, None);
    }

    #[test]
    fn one_pass_fast_promotion_requires_active_solid_elsewhere_and_clean_record() {
        // Active SolidElsewhere in /w1 + 1 non-flawed local Pass in /w2 -> Solid!
        let promoted = [
            pass_in("/w1", 1, false, 1000),
            pass_in("/w1", 2, true, 1001),
            pass_in("/w2", 3, false, 1500),
        ];
        let v = concept_view_in_workspace(&promoted, "/w2", 2000);
        assert_eq!(v.state, ConceptState::Solid);
        assert!(v.state.has_evidence());
        assert_eq!(v.passes, 1);
        assert_eq!(v.event_ids, vec![3]);
        assert_eq!(v.prior, None);

        // ShownElsewhere is NOT enough for 1-pass fast promotion.
        let from_shown = [
            pass_in("/w1", 1, false, 1000),
            pass_in("/w2", 2, false, 1500),
        ];
        assert_eq!(
            concept_view_in_workspace(&from_shown, "/w2", 2000).state,
            ConceptState::Shown
        );

        // Cooled SolidElsewhere (> AGE_SECS before local Pass) does NOT fast-promote.
        let cooled = [
            pass_in("/w1", 1, false, 1000),
            pass_in("/w1", 2, true, 1001),
            pass_in("/w2", 3, false, 1001 + AGE_SECS + 10),
        ];
        assert_eq!(
            concept_view_in_workspace(&cooled, "/w2", 1001 + AGE_SECS + 20).state,
            ConceptState::Shown
        );

        // Open global misconception blocks 1-pass fast promotion.
        let blocked = [
            pass_in("/w1", 1, false, 1000),
            pass_in("/w1", 2, true, 1001),
            misc_in("/w1", 3, EventKind::MisconceptionShown, "m1", 1100),
            pass_in("/w2", 4, false, 1500),
        ];
        let vb = concept_view_in_workspace(&blocked, "/w2", 2000);
        assert_eq!(vb.state, ConceptState::Shown);
        assert_eq!(vb.open_misconceptions, vec!["m1"]);

        // Local Fail (Shaky) followed by 1 Pass recovers to Shown, not Solid.
        let shaky_first = [
            pass_in("/w1", 1, false, 1000),
            pass_in("/w1", 2, true, 1001),
            ev_in("/w2", 3, EventKind::Fail, 1200),
            pass_in("/w2", 4, false, 1500),
        ];
        assert_eq!(
            concept_view_in_workspace(&shaky_first, "/w2", 2000).state,
            ConceptState::Shown
        );
    }

    #[test]
    fn global_misconceptions_travel_and_resolve_across_workspaces() {
        let events = [
            ev_in("/w2", 1, EventKind::Fail, 1000),
            misc_in("/w1", 2, EventKind::MisconceptionShown, "m-global", 1001),
            pass_in("/w2", 3, false, 1002),
        ];
        let v = concept_view_in_workspace(&events, "/w2", 2000);
        // Misconception from /w1 travels to /w2 and blocks Shaky -> Shown recovery.
        assert_eq!(v.state, ConceptState::Shaky);
        assert_eq!(v.open_misconceptions, vec!["m-global"]);

        // Resolving in /w3 clears it globally and unblocks /w2.
        let resolved = [
            events[0].clone(),
            events[1].clone(),
            events[2].clone(),
            misc_in("/w3", 4, EventKind::MisconceptionResolved, "m-global", 1003),
            pass_in("/w2", 5, false, 1004),
        ];
        let vr = concept_view_in_workspace(&resolved, "/w2", 2000);
        assert_eq!(vr.state, ConceptState::Shown);
        assert!(vr.open_misconceptions.is_empty());
    }

    #[test]
    fn global_recency_and_30_day_ageing_apply_across_all_workspaces() {
        // SolidElsewhere cools to ShownElsewhere after AGE_SECS of global inactivity.
        let elsewhere = [
            pass_in("/w1", 1, false, 1000),
            pass_in("/w1", 2, true, 1001),
        ];
        assert_eq!(
            concept_view_in_workspace(&elsewhere, "/w2", 1001 + AGE_SECS).prior,
            Some(PriorState::SolidElsewhere {
                workspace: "/w1".into()
            })
        );
        assert_eq!(
            concept_view_in_workspace(&elsewhere, "/w2", 1001 + AGE_SECS + 1).prior,
            Some(PriorState::ShownElsewhere {
                workspace: "/w1".into()
            })
        );

        // ReviewOk in /w3 refreshes the global 30-day clock for both local Solid and SolidElsewhere.
        let refreshed = [
            pass_in("/w1", 1, false, 1000),
            pass_in("/w1", 2, true, 1001),
            ev_in("/w3", 3, EventKind::ReviewOk, 1001 + AGE_SECS - 10),
        ];
        let after_30d = 1001 + AGE_SECS + 5;
        assert_eq!(
            concept_view_in_workspace(&refreshed, "/w1", after_30d).state,
            ConceptState::Solid
        );
        assert_eq!(
            concept_view_in_workspace(&refreshed, "/w2", after_30d).prior,
            Some(PriorState::SolidElsewhere {
                workspace: "/w1".into()
            })
        );
    }

    #[test]
    fn revisit_suppresses_prior_and_fast_promotion_until_newer_elsewhere_evidence() {
        assert_eq!(EventKind::parse("revisit"), Some(EventKind::Revisit));
        assert_eq!(EventKind::Revisit.as_str(), "revisit");

        // Revisit in /w2 after /w1's Solid suppresses prior.
        let revisited = [
            pass_in("/w1", 1, false, 1000),
            pass_in("/w1", 2, true, 1001),
            ev_in("/w2", 3, EventKind::Revisit, 1002),
        ];
        let v = concept_view_in_workspace(&revisited, "/w2", 2000);
        assert_eq!(v.state, ConceptState::Unknown);
        assert_eq!(v.prior, None);

        // And suppresses 1-pass fast promotion in /w2.
        let after_revisit_pass = [
            revisited[0].clone(),
            revisited[1].clone(),
            revisited[2].clone(),
            pass_in("/w2", 4, false, 1003),
        ];
        assert_eq!(
            concept_view_in_workspace(&after_revisit_pass, "/w2", 2000).state,
            ConceptState::Shown
        );

        // Newer elsewhere evidence after Revisit re-activates SolidElsewhere.
        let newer_elsewhere = [
            revisited[0].clone(),
            revisited[1].clone(),
            revisited[2].clone(),
            ev_in("/w1", 4, EventKind::ReviewOk, 1004),
        ];
        assert_eq!(
            concept_view_in_workspace(&newer_elsewhere, "/w2", 2000).prior,
            Some(PriorState::SolidElsewhere {
                workspace: "/w1".into()
            })
        );

        // Revisit never drops existing local evidence.
        let local_shown = [
            pass_in("/w2", 1, false, 1000),
            ev_in("/w2", 2, EventKind::Revisit, 1001),
        ];
        assert_eq!(
            concept_view_in_workspace(&local_shown, "/w2", 2000).state,
            ConceptState::Shown
        );
    }

    #[test]
    fn concept_domain_defaults_when_missing_in_json() {
        let c: Concept =
            serde_json::from_str(r#"{"id":"fft","label":"FFT","aliases":[],"created_by":"map"}"#)
                .unwrap();
        assert_eq!(c.domain, "");
    }
}
