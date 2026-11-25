//! The runtime's side of the understanding model (theory §10, A5): concept
//! matching, events from the loop, candidate targets, gaps and `/why`.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use serde_json::json;

use super::{Runtime, render_change, render_part};
use crate::decide::Question;
use crate::domain::{MapChange, PartStatus, ProposedConcept, ProposedPart, Session};
use crate::understanding::{
    self, Concept, ConceptState, ConceptView, EventKind, Misconception, NewEvent, Targets,
    exact_match, lexical_candidates, slugify, workspace_key,
};

/// A proposed concept is the same as an existing one at this probability (A5).
const SAME_CONCEPT: f64 = 0.7;
/// A belief from your log opens a misconception at this probability (A5).
const STATES_BELIEF: f64 = 0.8;
/// At most this many gaps are proposed as parts at once.
const MAX_GAP_PARTS: usize = 2;

impl Runtime {
    pub(super) fn workspace(session: &Session) -> String {
        workspace_key(&session.cwd)
    }

    pub(super) fn concept_views(
        &self,
        session: &Session,
        concepts: &[String],
    ) -> Result<HashMap<String, ConceptView>> {
        let events = self.store.events(&Self::workspace(session), concepts)?;
        Ok(understanding::views(&events, understanding::now()))
    }

    /// Records `told` on the concepts a question plainly names and returns
    /// their labels.
    pub(super) fn record_told(&self, session: &Session, text: &str) -> Vec<String> {
        let concepts = self
            .store
            .concepts(&Self::workspace(session))
            .unwrap_or_default();
        let named = crate::understanding::named_in(text, &concepts);
        self.append_events(
            session,
            named
                .iter()
                .map(|c| NewEvent::new(&c.id, EventKind::Told))
                .collect(),
        );
        named.iter().map(|c| c.label.clone()).collect()
    }

    pub(super) fn append_events(&self, session: &Session, events: Vec<NewEvent>) {
        if events.is_empty() {
            return;
        }
        if let Err(e) = self.store.append_events(
            &Self::workspace(session),
            &session.id,
            &events,
            understanding::now(),
        ) {
            tracing::warn!("recording understanding events: {e:#}");
        }
    }

    fn labels(&self, session: &Session) -> HashMap<String, String> {
        self.store
            .concepts(&Self::workspace(session))
            .unwrap_or_default()
            .into_iter()
            .map(|c| (c.id, c.label))
            .collect()
    }

    /// The part's concepts and their states, for the Tutor's view (A2).
    pub(super) fn concepts_prompt(&self, session: &Session, part: &crate::domain::Part) -> String {
        if part.concepts.is_empty() {
            return String::new();
        }
        let labels = self.labels(session);
        let mut all: Vec<String> = part.concepts.clone();
        for p in &session.map.parts {
            for c in &p.concepts {
                if !all.contains(c) {
                    all.push(c.clone());
                }
            }
        }
        let views = self.concept_views(session, &all).unwrap_or_default();
        let line = |c: &String| {
            format!(
                "- `{c}`: {} ({})",
                labels.get(c).map(String::as_str).unwrap_or(c),
                understanding::state_of(&views, c).label()
            )
        };
        let own: Vec<String> = part.concepts.iter().map(line).collect();
        let other: Vec<String> = all[part.concepts.len()..].iter().map(line).collect();
        let mut out = format!(
            "\nConcept ids this part rests on (the user's state):\n{}",
            own.join("\n")
        );
        if !other.is_empty() {
            out.push_str(&format!(
                "\nOther concept ids on the map:\n{}",
                other.join("\n")
            ));
        }
        out
    }

    /// Existing concepts for the Mapper, so it reuses their ids.
    pub(super) fn known_concepts_prompt(&self, session: &Session) -> String {
        let concepts = self
            .store
            .concepts(&Self::workspace(session))
            .unwrap_or_default();
        if concepts.is_empty() {
            return String::new();
        }
        let list: Vec<String> = concepts
            .iter()
            .take(80)
            .map(|c| format!("- `{}`: {}", c.id, c.label))
            .collect();
        format!(
            "\n\nConcepts already recorded in this workspace (reuse these ids when a part rests on one):\n{}",
            list.join("\n")
        )
    }

    /// Maps each proposed name to a concept id: an exact match on id, label or
    /// alias; else a decision model picks among lexical candidates (same at
    /// p ≥ 0.7); else a new concept. One decision request covers them all.
    pub(super) async fn resolve_concepts(
        &self,
        session: &Session,
        proposed: &[ProposedConcept],
        created_by: &str,
    ) -> Result<HashMap<String, String>> {
        let ws = Self::workspace(session);
        let existing = self.store.concepts(&ws)?;
        let mut out = HashMap::new();
        let mut ask: Vec<(usize, Vec<String>)> = Vec::new();
        let mut questions = Vec::new();
        for (i, p) in proposed.iter().enumerate() {
            let label = if p.label.trim().is_empty() {
                p.id.clone()
            } else {
                p.label.clone()
            };
            if let Some(c) =
                exact_match(&p.id, &existing).or_else(|| exact_match(&label, &existing))
            {
                out.insert(p.id.clone(), c.id.clone());
                continue;
            }
            let lex = lexical_candidates(&label, &existing);
            if lex.is_empty() {
                continue;
            }
            let mut options: Vec<(String, String)> = lex
                .iter()
                .map(|c| (c.id.clone(), c.label.clone()))
                .collect();
            options.push(("new".into(), "None of these: a different concept".into()));
            questions.push(Question {
                name: format!("c{i}"),
                instructions: format!(
                    "Is the concept \"{label}\" the same idea as one of these recorded concepts? Only the same idea counts, not a related one."
                ),
                options,
            });
            ask.push((i, lex.iter().map(|c| c.id.clone()).collect()));
        }
        if !questions.is_empty() && !session.decisions_off {
            let input = format!(
                "Matching concepts for the task: {}",
                session.goal.as_deref().unwrap_or("(no goal)")
            );
            match self
                .decider
                .decide(&input, &questions, session.decision_model.as_deref())
                .await
            {
                Ok(Some(decision)) => {
                    for (i, ids) in &ask {
                        if let Some(a) = decision.answers.get(&format!("c{i}"))
                            && a.choice != "new"
                            && a.probability >= SAME_CONCEPT
                            && ids.contains(&a.choice)
                        {
                            out.insert(proposed[*i].id.clone(), a.choice.clone());
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("concept matching: {e:#}"),
            }
        }
        // Register: matched names become aliases, the rest new concepts.
        let mut taken: HashSet<String> = existing.iter().map(|c| c.id.clone()).collect();
        for p in proposed {
            let label = if p.label.trim().is_empty() {
                p.id.clone()
            } else {
                p.label.clone()
            };
            match out.get(&p.id) {
                Some(id) => {
                    let aliases = [p.id.clone(), label]
                        .into_iter()
                        .filter(|a| a != id)
                        .collect();
                    self.store.upsert_concept(
                        &ws,
                        &Concept {
                            id: id.clone(),
                            label: String::new(),
                            aliases,
                            created_by: created_by.into(),
                        },
                    )?;
                }
                None => {
                    let mut id = slugify(&p.id);
                    if id.is_empty() {
                        id = slugify(&label);
                    }
                    if id.is_empty() {
                        continue;
                    }
                    if taken.contains(&id) {
                        let base = id.clone();
                        id = (2..)
                            .map(|n| format!("{base}-{n}"))
                            .find(|c| !taken.contains(c))
                            .unwrap();
                    }
                    taken.insert(id.clone());
                    self.store.upsert_concept(
                        &ws,
                        &Concept {
                            id: id.clone(),
                            label,
                            aliases: vec![],
                            created_by: created_by.into(),
                        },
                    )?;
                    out.insert(p.id.clone(), id);
                }
            }
        }
        // Prerequisites, in resolved ids.
        for p in proposed {
            let Some(id) = out.get(&p.id) else { continue };
            for need in &p.needs {
                let need = out.get(need).cloned().unwrap_or_else(|| slugify(need));
                if taken.contains(&need) || existing.iter().any(|c| c.id == need) {
                    self.store.add_prerequisite(&ws, id, &need, created_by)?;
                }
            }
        }
        Ok(out)
    }

    /// Registers the concepts the map's parts rest on and rewrites each
    /// part's concept list to resolved ids (1–4 each).
    pub(super) async fn register_map_concepts(
        &self,
        session: &mut Session,
        declared: &[ProposedConcept],
        created_by: &str,
    ) -> Result<()> {
        let mut all: Vec<ProposedConcept> = declared.to_vec();
        for part in &session.map.parts {
            for c in &part.concepts {
                if !all.iter().any(|p| &p.id == c) {
                    all.push(ProposedConcept {
                        id: c.clone(),
                        label: c.replace('-', " "),
                        needs: vec![],
                    });
                }
            }
        }
        if all.is_empty() {
            return Ok(());
        }
        let resolved = self.resolve_concepts(session, &all, created_by).await?;
        for part in &mut session.map.parts {
            let mut ids: Vec<String> = Vec::new();
            for c in &part.concepts {
                let id = resolved.get(c).cloned().unwrap_or_else(|| c.clone());
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            ids.truncate(4);
            part.concepts = ids;
        }
        Ok(())
    }

    /// The Extractor (A2): the concepts a log entry touched, and a wrong
    /// belief it states if a decision model is sure (p ≥ 0.8).
    pub(super) async fn extract_from_log(
        &self,
        session: &Session,
        text: &str,
        touched: &[String],
        misconception: Option<&crate::domain::ProposedMisconception>,
    ) -> Result<()> {
        let mut names: Vec<ProposedConcept> = touched
            .iter()
            .filter(|t| !t.trim().is_empty())
            .take(6)
            .map(|t| ProposedConcept {
                id: t.clone(),
                label: t.clone(),
                needs: vec![],
            })
            .collect();
        if let Some(m) = misconception
            && !names.iter().any(|n| n.id == m.concept)
        {
            names.push(ProposedConcept {
                id: m.concept.clone(),
                label: m.concept.clone(),
                needs: vec![],
            });
        }
        if names.is_empty() {
            return Ok(());
        }
        let resolved = self.resolve_concepts(session, &names, "extractor").await?;
        let mut events: Vec<NewEvent> = Vec::new();
        let mut seen = HashSet::new();
        for t in touched {
            if let Some(id) = resolved.get(t)
                && seen.insert(id.clone())
            {
                events.push(NewEvent::new(id, EventKind::Touched));
            }
        }
        if let Some(m) = misconception
            && let Some(concept) = resolved.get(&m.concept)
            && !m.statement.trim().is_empty()
            && self.states_belief(session, text, &m.statement).await
        {
            let id = format!("m-{}", uuid::Uuid::new_v4().simple());
            self.store.add_misconception(
                &Self::workspace(session),
                &Misconception {
                    id: id.clone(),
                    concept: concept.clone(),
                    statement: m.statement.clone(),
                    source: "log".into(),
                },
            )?;
            events.push(NewEvent::new(concept, EventKind::MisconceptionShown).reference(id));
        }
        self.append_events(session, events);
        Ok(())
    }

    /// The decision predicate "does this message state that belief?". With
    /// no decision model it stays closed: a misconception never opens on a
    /// language model's impression alone.
    async fn states_belief(&self, session: &Session, text: &str, belief: &str) -> bool {
        if session.decisions_off {
            return false;
        }
        let q = [Question {
            name: "states".into(),
            instructions: format!("Does this message state this belief: \"{belief}\"?"),
            options: vec![
                (
                    "yes".into(),
                    "The message states or clearly assumes the belief".into(),
                ),
                ("no".into(), "It doesn't, or only asks about it".into()),
            ],
        }];
        match self
            .decider
            .decide(text, &q, session.decision_model.as_deref())
            .await
        {
            Ok(Some(d)) => d
                .answers
                .get("states")
                .is_some_and(|a| a.choice == "yes" && a.probability >= STATES_BELIEF),
            Ok(None) => false,
            Err(e) => {
                tracing::warn!("misconception check: {e:#}");
                false
            }
        }
    }

    /// What the next candidate for `part_id` checks (A5).
    pub(super) fn next_targets(
        &self,
        session: &Session,
        part_id: &str,
    ) -> Option<(Targets, Option<Misconception>, HashMap<String, String>)> {
        let part = session.map.get(part_id)?;
        let views = self.concept_views(session, &part.concepts).ok()?;
        let targets = understanding::targets(&part.concepts, &views)?;
        let misconception = targets
            .misconception
            .as_deref()
            .and_then(|m| self.store.misconception(m).ok().flatten());
        Some((targets, misconception, self.labels(session)))
    }

    /// The part's status from its core concepts, when it has any.
    pub(super) fn understanding_status(
        &self,
        session: &Session,
        part_id: &str,
    ) -> Option<understanding::UnderstandingStatus> {
        let part = session.map.get(part_id)?;
        if part.concepts.is_empty() {
            return None;
        }
        let views = self.concept_views(session, &part.concepts).ok()?;
        let states: Vec<ConceptState> = part
            .concepts
            .iter()
            .map(|c| understanding::state_of(&views, c))
            .collect();
        Some(understanding::part_status(
            &states,
            part.status == PartStatus::Delegated,
        ))
    }

    /// Events for a verdict on a candidate (A5).
    pub(super) fn record_verdict(
        &self,
        session: &Session,
        pending: &crate::domain::PendingCandidate,
        pass: bool,
        misconception: Option<&str>,
    ) {
        let mut events = Vec::new();
        let flawed = !pending.key_correct;
        if pass {
            if let Some(m) = &pending.misconception
                && let Ok(Some(found)) = self.store.misconception(m)
            {
                events.push(
                    NewEvent::new(&found.concept, EventKind::MisconceptionResolved)
                        .reference(m.clone()),
                );
            }
            for t in &pending.targets {
                events.push(
                    NewEvent::new(t, EventKind::Pass)
                        .reference(pending.id.clone())
                        .detail(json!({"flawed": flawed})),
                );
            }
        } else {
            for t in &pending.targets {
                events.push(
                    NewEvent::new(t, EventKind::Fail)
                        .reference(pending.id.clone())
                        .detail(json!({"flawed": flawed})),
                );
            }
            if let (Some(statement), Some(concept)) = (
                misconception.filter(|m| !m.trim().is_empty()),
                pending.targets.first(),
            ) {
                let id = format!("m-{}", uuid::Uuid::new_v4().simple());
                match self.store.add_misconception(
                    &Self::workspace(session),
                    &Misconception {
                        id: id.clone(),
                        concept: concept.clone(),
                        statement: statement.to_string(),
                        source: "judge".into(),
                    },
                ) {
                    Ok(()) => events
                        .push(NewEvent::new(concept, EventKind::MisconceptionShown).reference(id)),
                    Err(e) => tracing::warn!("recording a misconception: {e:#}"),
                }
            }
        }
        self.append_events(session, events);
    }

    /// `claim` or `delegated` on every core concept of a part (§6).
    pub(super) fn record_skip(&self, session: &Session, part_id: &str, kind: EventKind) {
        let Some(part) = session.map.get(part_id) else {
            return;
        };
        let events = part
            .concepts
            .iter()
            .map(|c| NewEvent::new(c, kind).reference(part_id.to_string()))
            .collect();
        self.append_events(session, events);
    }

    /// The active part as you enter it: its card, what it rests on, and any
    /// gaps underneath it (A5), each raised once, proposed as small parts.
    pub(super) fn enter_part(&self, session: &mut Session, hint: bool) -> String {
        let Some(part) = session.active_part().cloned() else {
            return String::new();
        };
        let lang = session.language.clone();
        let lang = lang.as_deref();
        let mut out = render_part(&part, hint, lang);
        if part.concepts.is_empty() {
            return out;
        }
        let labels = self.labels(session);
        let label = |id: &str| labels.get(id).cloned().unwrap_or_else(|| id.to_string());
        let ws = Self::workspace(session);
        let prerequisites = self.store.prerequisites(&ws).unwrap_or_default();
        // Everything reachable, so gaps can see the states underneath.
        let mut wanted: Vec<String> = part.concepts.clone();
        for needs in prerequisites.values() {
            wanted.extend(needs.iter().cloned());
        }
        wanted.extend(prerequisites.keys().cloned());
        wanted.sort();
        wanted.dedup();
        let Ok(views) = self.concept_views(session, &wanted) else {
            return out;
        };
        let rests: Vec<String> = part
            .concepts
            .iter()
            .map(|c| {
                format!(
                    "{} ({})",
                    label(c),
                    crate::i18n::state(lang, understanding::state_of(&views, c))
                )
            })
            .collect();
        out.push_str(&format!(
            "\n{}: {}.",
            crate::i18n::line(lang, crate::i18n::Line::RestsOn),
            rests.join(", ")
        ));

        let covered: HashSet<String> = session
            .map
            .parts
            .iter()
            .filter(|p| p.id != part.id && p.status == PartStatus::Open)
            .flat_map(|p| p.concepts.iter().cloned())
            .collect();
        // A quick check is only the check: no prerequisite parts.
        if session.quick_check {
            return out;
        }
        let gaps: Vec<understanding::Gap> =
            understanding::gaps(&part.concepts, &prerequisites, &views, &covered)
                .into_iter()
                .filter(|g| !session.gaps_raised.contains(&g.concept))
                .collect();
        if gaps.is_empty() {
            return out;
        }
        for g in &gaps {
            use crate::i18n::{Line, line};
            let why = line(
                lang,
                match g.state {
                    ConceptState::Claimed => Line::GapClaimed,
                    ConceptState::Delegated => Line::GapDelegated,
                    ConceptState::Shaky => Line::GapShaky,
                    _ => Line::GapUncovered,
                },
            );
            out.push_str(&format!(
                "\n{}",
                line(lang, Line::GapNeeds)
                    .replace("{concept}", &label(&g.concept))
                    .replace("{why}", why)
            ));
            session.gaps_raised.push(g.concept.clone());
        }
        if session.pending_change.is_none() {
            let mut taken: HashSet<String> =
                session.map.parts.iter().map(|p| p.id.clone()).collect();
            let parts: Vec<ProposedPart> = gaps
                .iter()
                .take(MAX_GAP_PARTS)
                .map(|g| {
                    let id = part_id_for(&g.concept, &taken);
                    taken.insert(id.clone());
                    ProposedPart {
                        id,
                        title: label(&g.concept),
                        note: crate::i18n::line(lang, crate::i18n::Line::NeededBy)
                            .replace("{part}", &part.title),
                        depends_on: vec![],
                        concepts: vec![g.concept.clone()],
                    }
                })
                .collect();
            let names: Vec<String> = gaps
                .iter()
                .take(MAX_GAP_PARTS)
                .map(|g| label(&g.concept))
                .collect();
            let change = MapChange {
                brief: crate::i18n::line(lang, crate::i18n::Line::AddGapParts).replace(
                    "{names}",
                    &names.join(crate::i18n::line(lang, crate::i18n::Line::And)),
                ),
                goal: None,
                parts,
                remove: vec![],
                concepts: vec![],
            };
            if session.map.with_change(&change).is_ok() {
                out.push_str(&format!(
                    "\n\n{}",
                    render_change(&change, &session.map, lang)
                ));
                session.pending_change = Some(change);
            }
        }
        out
    }

    /// `/why`: what the active part's status rests on.
    pub(super) fn why(&self, session: &Session) -> String {
        let Some(part) = session.active_part() else {
            return self.folder_record(session);
        };
        if part.concepts.is_empty() {
            return format!(
                "**{}** has no concepts recorded (the map predates them); its status is {}.",
                part.title,
                part.status.label()
            );
        }
        use crate::domain::{mermaid_id, mermaid_text};
        use crate::i18n::{Line, line, part_status, state};
        let lang = session.language.clone();
        let lang = lang.as_deref();
        let ws = Self::workspace(session);
        let prerequisites = self.store.prerequisites(&ws).unwrap_or_default();
        // The part's concepts, and one level of what they build on.
        let mut ids: Vec<String> = part.concepts.clone();
        for c in &part.concepts {
            for n in prerequisites.get(c).into_iter().flatten() {
                if !ids.contains(n) {
                    ids.push(n.clone());
                }
            }
        }
        let Ok(views) = self.concept_views(session, &ids) else {
            return "Couldn't read the record.".into();
        };
        let labels = self.labels(session);
        let name = |c: &str| labels.get(c).cloned().unwrap_or_else(|| c.to_string());
        let part_node = format!("part_{}", mermaid_id(&part.id));
        let mut out = format!(
            "```mermaid\nflowchart LR\n    {part_node}[\"▶ {}<br/><small>{}</small>\"]\n",
            mermaid_text(&part.title),
            mermaid_text(part_status(lang, part.status)),
        );
        let mut notes = Vec::new();
        for c in &ids {
            let detail = match views.get(c) {
                Some(v) if v.passes + v.fails > 0 => format!(
                    "{} · {}",
                    state(lang, v.state),
                    line(lang, Line::Checks)
                        .replace("{passed}", &v.passes.to_string())
                        .replace("{failed}", &v.fails.to_string())
                ),
                Some(v) => state(lang, v.state).to_string(),
                None => state(lang, ConceptState::Unknown).to_string(),
            };
            out.push_str(&format!(
                "    {}[\"{}<br/><small>{}</small>\"]\n",
                mermaid_id(c),
                mermaid_text(&name(c)),
                mermaid_text(&detail),
            ));
            if let Some(v) = views.get(c) {
                for m in &v.open_misconceptions {
                    if let Ok(Some(found)) = self.store.misconception(m) {
                        notes.push(
                            line(lang, Line::OpenMisconception)
                                .replace("{concept}", &name(c))
                                .replace("{statement}", &found.statement),
                        );
                    }
                }
            }
        }
        for c in &part.concepts {
            out.push_str(&format!("    {} --> {part_node}\n", mermaid_id(c)));
            for n in prerequisites.get(c).into_iter().flatten() {
                out.push_str(&format!("    {} --> {}\n", mermaid_id(n), mermaid_id(c)));
            }
        }
        out.push_str("```");
        for n in notes {
            out.push_str(&format!("\n\n{n}"));
        }
        out.push_str(&format!("\n\n{}", line(lang, Line::EvidenceNote)));
        out
    }

    /// Most ideas the folder-wide record draws.
    const RECORD_MAX: usize = 40;

    /// The folder's record outside a task: every idea with any event, its
    /// state (or "told only"), linked by what builds on what, as Mermaid.
    pub(super) fn folder_record(&self, session: &Session) -> String {
        use crate::i18n::{Line, line, state};
        let lang = session.language.as_deref();
        let ws = Self::workspace(session);
        let concepts = self.store.concepts(&ws).unwrap_or_default();
        let ids: Vec<String> = concepts.iter().map(|c| c.id.clone()).collect();
        let events = self.store.events(&ws, &ids).unwrap_or_default();
        let views = self.concept_views(session, &ids).unwrap_or_default();
        // Ideas with any event, most recently active first.
        let mut active: Vec<(&Concept, i64, bool)> = concepts
            .iter()
            .filter_map(|c| {
                let mine: Vec<&crate::understanding::Event> =
                    events.iter().filter(|e| e.concept == c.id).collect();
                let last = mine.iter().map(|e| e.id).max()?;
                let told = mine.iter().any(|e| e.kind == EventKind::Told);
                Some((c, last, told))
            })
            .collect();
        if active.is_empty() {
            return line(lang, Line::NoRecord).to_string();
        }
        active.sort_by(|a, b| b.1.cmp(&a.1));
        active.truncate(Self::RECORD_MAX);
        let shown_ids: HashSet<&str> = active.iter().map(|(c, ..)| c.id.as_str()).collect();
        let (mut shown, mut told_only, mut shaky) = (0, 0, 0);
        let mut out = format!(
            "{}\n\n```mermaid\nflowchart LR\n",
            line(lang, Line::RecordTitle)
        );
        for (c, _, told) in &active {
            let st = understanding::state_of(&views, &c.id);
            let name = if st == ConceptState::Unknown && *told {
                told_only += 1;
                line(lang, Line::ToldOnly).to_string()
            } else {
                if st.has_evidence() {
                    shown += 1;
                }
                if st == ConceptState::Shaky {
                    shaky += 1;
                }
                state(lang, st).to_string()
            };
            out.push_str(&format!(
                "    {}[\"{}<br/><small>{}</small>\"]\n",
                crate::domain::mermaid_id(&c.id),
                crate::domain::mermaid_text(&c.label),
                crate::domain::mermaid_text(&name),
            ));
        }
        for (concept, needs) in self.store.prerequisites(&ws).unwrap_or_default() {
            for n in needs {
                if shown_ids.contains(concept.as_str()) && shown_ids.contains(n.as_str()) {
                    out.push_str(&format!(
                        "    {} --> {}\n",
                        crate::domain::mermaid_id(&n),
                        crate::domain::mermaid_id(&concept)
                    ));
                }
            }
        }
        out.push_str("```\n\n");
        out.push_str(
            &line(lang, Line::RecordSummary)
                .replace("{shown}", &shown.to_string())
                .replace("{told}", &told_only.to_string())
                .replace("{shaky}", &shaky.to_string()),
        );
        out
    }
}

/// A part id (≤ 12 characters) for a gap concept, unique on the map.
fn part_id_for(concept: &str, taken: &HashSet<String>) -> String {
    let base: String = concept.chars().take(12).collect::<String>();
    let base = base.trim_end_matches('-').to_string();
    if !taken.contains(&base) {
        return base;
    }
    let short: String = base.chars().take(10).collect::<String>();
    let short = short.trim_end_matches('-');
    (2..)
        .map(|n| format!("{short}-{n}"))
        .find(|c| !taken.contains(c))
        .unwrap()
}
