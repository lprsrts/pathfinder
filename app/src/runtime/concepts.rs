//! The runtime's side of the understanding model (theory §10, A5): concept
//! matching, events from the loop, candidate targets, gaps and `/why`.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use serde_json::json;

use super::{Runtime, TurnOutput, render_change, render_part};
use crate::decide::Question;
use crate::domain::{MapChange, PartStatus, ProposedConcept, ProposedPart, Session};
use crate::understanding::{
    self, Concept, ConceptState, ConceptView, EventKind, Misconception, NewEvent, PriorState,
    Targets, exact_match, lexical_candidates, slugify, workspace_key,
};

/// A proposed concept is the same as an existing one at this probability (A5).
const SAME_CONCEPT: f64 = 0.7;
/// A belief from your log opens a misconception at this probability (A5).
const STATES_BELIEF: f64 = 0.8;
/// At most this many gaps are proposed as parts at once.
const MAX_GAP_PARTS: usize = 2;
/// Maximum concepts listed in the Mapper prompt.
const MAX_KNOWN_CONCEPTS: usize = 80;
/// Slots reserved for top goal-relevant global concepts when the workspace is large.
const MAX_GLOBAL_RESERVE: usize = 20;
/// Maximum recent same-workspace `Touched` notes passed to the Tutor.
const MAX_TOUCHED_NOTES: usize = 5;
/// Maximum characters stored per `Touched` note snippet.
const MAX_NOTE_CHARS: usize = 200;

impl Runtime {
    pub(super) fn workspace(session: &Session) -> String {
        workspace_key(&session.cwd)
    }

    pub(super) fn concept_views(
        &self,
        session: &Session,
        concepts: &[String],
    ) -> Result<HashMap<String, ConceptView>> {
        let ws = Self::workspace(session);
        let events = self.store.events_for_concepts(concepts)?;
        Ok(understanding::views_in_workspace(
            &events,
            &ws,
            understanding::now(),
        ))
    }

    /// Appends events and returns their ids (empty if recording failed).
    pub(super) fn append_events(&self, session: &Session, events: Vec<NewEvent>) -> Vec<i64> {
        if events.is_empty() {
            return Vec::new();
        }
        match self.store.append_events(
            &Self::workspace(session),
            &session.id,
            &events,
            understanding::now(),
        ) {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!("recording understanding events: {e:#}");
                Vec::new()
            }
        }
    }

    pub(super) fn labels(&self, _session: &Session) -> HashMap<String, String> {
        self.store
            .all_concepts()
            .unwrap_or_default()
            .into_iter()
            .map(|c| (c.id, c.label))
            .collect()
    }

    /// The part's concepts and their states, for the Tutor's view (A2).
    /// Surfaces cross-workspace `prior` with domain tags only (never raw paths
    /// or repo names) and includes a concise digest of recent same-workspace
    /// `Touched` notes from earlier sessions.
    pub(super) fn concepts_prompt(&self, session: &Session, part: &crate::domain::Part) -> String {
        if part.concepts.is_empty() {
            return String::new();
        }
        let all_concepts = self.store.all_concepts().unwrap_or_default();
        let meta: HashMap<&str, (&str, &str)> = all_concepts
            .iter()
            .map(|c| (c.id.as_str(), (c.label.as_str(), c.domain.as_str())))
            .collect();
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
            let (label, domain) = meta.get(c.as_str()).copied().unwrap_or((c.as_str(), ""));
            let dom = if domain.is_empty() {
                String::new()
            } else {
                format!(" [{domain}]")
            };
            format!("- `{c}`: {label}{dom} ({})", prompt_state(&views, c))
        };
        let own: Vec<String> = part.concepts.iter().map(line).collect();
        let other: Vec<String> = all[part.concepts.len()..].iter().map(line).collect();
        let mut out = format!(
            "\nConcept ids this part rests on (the user's state):\n{}",
            own.join("\n")
        );
        let has_prior = part.concepts.iter().any(|c| {
            views
                .get(c)
                .is_some_and(|v| !v.state.has_evidence() && v.prior.is_some())
        });
        if has_prior {
            out.push_str(
                "\nConcepts marked `prior` were already demonstrated in another workspace; skip basic definitions and focus on how they apply in this workspace.",
            );
        }
        if !other.is_empty() {
            out.push_str(&format!(
                "\nOther concept ids on the map:\n{}",
                other.join("\n")
            ));
        }
        let ws = Self::workspace(session);
        let ws_events = self.store.events(&ws, &part.concepts).unwrap_or_default();
        let mut notes: Vec<(String, String)> = Vec::new();
        let mut seen_notes: HashSet<String> = HashSet::new();
        for e in ws_events.iter().rev() {
            if e.kind != EventKind::Touched {
                continue;
            }
            let Some(note) = e.detail["note"]
                .as_str()
                .map(str::trim)
                .filter(|n| !n.is_empty())
            else {
                continue;
            };
            if part
                .log
                .iter()
                .any(|l| l.trim() == note || l.contains(note))
            {
                continue;
            }
            let key = format!("{}:{note}", e.concept);
            if seen_notes.insert(key) {
                notes.push((e.concept.clone(), note.to_string()));
                if notes.len() >= MAX_TOUCHED_NOTES {
                    break;
                }
            }
        }
        notes.reverse();
        if !notes.is_empty() {
            out.push_str("\nRecent notes from earlier sessions in this workspace:");
            for (c, note) in notes {
                out.push_str(&format!("\n- `{c}`: {note}"));
            }
        }
        out
    }

    /// Current-workspace concepts plus top goal-relevant global concepts
    /// (capped at ~80 total), showing state (`shown`, `solid`, `prior`, …) and
    /// short `domain` tag. Never includes raw filesystem paths or repo names
    /// from other workspaces.
    pub(super) fn known_concepts_prompt(&self, session: &Session) -> String {
        let ws = Self::workspace(session);
        let all_concepts = self.store.all_concepts().unwrap_or_default();
        if all_concepts.is_empty() {
            return String::new();
        }
        let ws_ids: HashSet<String> = self
            .store
            .workspace_concepts(&ws)
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.id)
            .collect();
        let ids: Vec<String> = all_concepts.iter().map(|c| c.id.clone()).collect();
        let views = self.concept_views(session, &ids).unwrap_or_default();
        let state_rank = |c: &Concept| match views.get(&c.id) {
            Some(v) if v.state.has_evidence() => 0u8,
            Some(v) if v.prior.is_some() => 1,
            Some(v) if v.state == ConceptState::Shaky => 2,
            Some(v) if matches!(v.state, ConceptState::Claimed | ConceptState::Delegated) => 3,
            _ => 4,
        };

        let mut ws_ordered: Vec<&Concept> = all_concepts
            .iter()
            .filter(|c| ws_ids.contains(&c.id))
            .collect();
        ws_ordered.sort_by(|a, b| {
            state_rank(a)
                .cmp(&state_rank(b))
                .then_with(|| a.id.cmp(&b.id))
        });

        let query_tokens = goal_query_tokens(session);
        let mut global_scored: Vec<(usize, u8, &Concept)> = all_concepts
            .iter()
            .filter(|c| !ws_ids.contains(&c.id))
            .filter_map(|c| {
                let score = relevance_score(c, &query_tokens);
                (score > 0).then_some((score, state_rank(c), c))
            })
            .collect();
        global_scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.id.cmp(&b.2.id))
        });

        let global_reserve = global_scored.len().min(MAX_GLOBAL_RESERVE);
        let ws_take = ws_ordered
            .len()
            .min(MAX_KNOWN_CONCEPTS.saturating_sub(global_reserve));
        let global_take = MAX_KNOWN_CONCEPTS.saturating_sub(ws_take);

        let mut selected: Vec<&Concept> = Vec::with_capacity(MAX_KNOWN_CONCEPTS);
        selected.extend(ws_ordered.into_iter().take(ws_take));
        selected.extend(
            global_scored
                .into_iter()
                .take(global_take)
                .map(|(_, _, c)| c),
        );
        if selected.is_empty() {
            return String::new();
        }
        let list: Vec<String> = selected
            .into_iter()
            .map(|c| {
                let dom = if c.domain.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", c.domain)
                };
                format!(
                    "- `{}`: {}{dom} ({})",
                    c.id,
                    c.label,
                    prompt_state(&views, &c.id)
                )
            })
            .collect();
        format!(
            "\n\nConcepts already recorded, with the user's state on each (shown and solid mean passed locally; prior means passed in another workspace):\n{}\nWhen a part rests on one of these ideas in the same domain, list its id exactly, even if you would word or scope it a little differently; don't declare a new concept for it under another name. The user's checks count only under the same id. Only reuse an id when it is the same idea in the same domain; declare a new concept for an idea none of them is.",
            list.join("\n")
        )
    }

    /// Maps each proposed name to a global concept id: an exact/same-name match
    /// (respecting domain compatibility so homonyms across domains don't merge);
    /// else a decision model picks among lexical candidates (with domain tags,
    /// same at p ≥ 0.7); else a new concept. Links matched/new concepts to the
    /// current workspace via `upsert_workspace_concept`.
    pub(super) async fn resolve_concepts(
        &self,
        session: &Session,
        proposed: &[ProposedConcept],
        created_by: &str,
    ) -> Result<HashMap<String, String>> {
        let ws = Self::workspace(session);
        let existing = self.store.all_concepts()?;
        let ws_ids: HashSet<String> = self
            .store
            .workspace_concepts(&ws)?
            .into_iter()
            .map(|c| c.id)
            .collect();
        let ws_existing: Vec<Concept> = existing
            .iter()
            .filter(|c| ws_ids.contains(&c.id))
            .cloned()
            .collect();
        let mut out = HashMap::new();
        let mut ask: Vec<(usize, Vec<String>)> = Vec::new();
        let mut questions = Vec::new();
        for (i, p) in proposed.iter().enumerate() {
            let label = if p.label.trim().is_empty() {
                p.id.clone()
            } else {
                p.label.clone()
            };
            // Prefer current-workspace matches first, then global matches,
            // never auto-merging across conflicting domains (homonym safety).
            if let Some(c) = compatible_match(&p.id, &label, &p.domain, &ws_existing)
                .or_else(|| compatible_match(&p.id, &label, &p.domain, &existing))
            {
                out.insert(p.id.clone(), c.id.clone());
                continue;
            }
            let mut lex = lexical_candidates(&label, &existing);
            for extra in [
                exact_match(&p.id, &existing),
                exact_match(&label, &existing),
                understanding::same_named(&p.id, &existing),
                understanding::same_named(&label, &existing),
            ]
            .into_iter()
            .flatten()
            {
                if !lex.iter().any(|l| l.id == extra.id) {
                    lex.insert(0, extra);
                }
            }
            if lex.is_empty() {
                continue;
            }
            let mut options: Vec<(String, String)> = lex
                .iter()
                .map(|c| {
                    let desc = if c.domain.is_empty() {
                        c.label.clone()
                    } else {
                        format!("{} [domain: {}]", c.label, c.domain)
                    };
                    (c.id.clone(), desc)
                })
                .collect();
            options.push(("new".into(), "None of these: a different concept".into()));
            let proposed_desc = if p.domain.trim().is_empty() {
                format!("\"{label}\"")
            } else {
                format!("\"{label}\" (domain: {})", p.domain.trim())
            };
            questions.push(Question {
                name: format!("c{i}"),
                instructions: format!(
                    "Is the concept {proposed_desc} the same idea as one of these recorded concepts? The same idea in other words or with an abbreviation counts (\"GMM\" and \"Gaussian mixture model\"); a related, broader or narrower idea, or a homonym from a different domain (e.g. physics \"field\" vs struct \"field\"), doesn't."
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
            let domain = p.domain.trim().to_string();
            match out.get(&p.id) {
                Some(id) => {
                    let aliases = [p.id.clone(), label]
                        .into_iter()
                        .filter(|a| a != id)
                        .collect();
                    self.store.upsert_workspace_concept(
                        &ws,
                        &Concept {
                            id: id.clone(),
                            label: String::new(),
                            aliases,
                            domain,
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
                    self.store.upsert_workspace_concept(
                        &ws,
                        &Concept {
                            id: id.clone(),
                            label,
                            aliases: vec![],
                            domain,
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
                    self.store.add_prerequisite(id, &need, created_by)?;
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
                        domain: String::new(),
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
                domain: String::new(),
                needs: vec![],
            })
            .collect();
        if let Some(m) = misconception
            && !names.iter().any(|n| n.id == m.concept)
        {
            names.push(ProposedConcept {
                id: m.concept.clone(),
                label: m.concept.clone(),
                domain: String::new(),
                needs: vec![],
            });
        }
        if names.is_empty() {
            return Ok(());
        }
        let resolved = self.resolve_concepts(session, &names, "extractor").await?;
        let mut events: Vec<NewEvent> = Vec::new();
        let mut seen = HashSet::new();
        let note = truncate_note(text, MAX_NOTE_CHARS);
        for t in touched {
            if let Some(id) = resolved.get(t)
                && seen.insert(id.clone())
            {
                let mut ev = NewEvent::new(id, EventKind::Touched);
                if !note.is_empty() {
                    ev = ev.detail(json!({"note": note}));
                }
                events.push(ev);
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
            // A misconception closes only by passing a flawed candidate built
            // to embody it (§10, A5): a correct candidate or an exam answer
            // passes its concepts but leaves the belief open.
            if flawed
                && let Some(m) = &pending.misconception
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

    /// `claim` or `delegated` on the core concepts of a part (§6). Returns
    /// (event id, concept) for each, so a claim can be undone (§15).
    ///
    /// A claim is your word on every concept the part rests on. Handing a
    /// part off says nothing about a concept another open part still rests
    /// on: you may yet show it there, so it keeps the state it earned
    /// rather than turning `delegated` by association.
    pub(super) fn record_skip(
        &self,
        session: &Session,
        part_id: &str,
        kind: EventKind,
    ) -> Vec<(i64, String)> {
        let Some(part) = session.map.get(part_id) else {
            return Vec::new();
        };
        let concepts: Vec<String> = part
            .concepts
            .iter()
            .filter(|c| {
                kind != EventKind::Delegated
                    || !session.map.parts.iter().any(|p| {
                        p.id != part.id && p.status == PartStatus::Open && p.concepts.contains(c)
                    })
            })
            .cloned()
            .collect();
        let events = concepts
            .iter()
            .map(|c| NewEvent::new(c, kind).reference(part_id.to_string()))
            .collect();
        self.append_events(session, events)
            .into_iter()
            .zip(concepts)
            .collect()
    }

    /// The active part as you enter it: its card, what it rests on, any
    /// gaps underneath it (A5), then the horizon and the next step (§16).
    /// One pointer only: the next step's place, or the card's "Read:" when
    /// there's no next step.
    pub(super) fn enter_part(&self, session: &mut Session, hint: bool) -> String {
        self.enter_part_after(session, hint, None)
    }

    /// `enter_part`, right after the agent did a part (`ran`: the concepts
    /// it rests on): only gaps the run opened are raised now (a concept of
    /// that part the next one needs). Any other gap waits for the next part
    /// you enter, so an agent run never ends in an unrelated map change.
    pub(super) fn enter_part_after(
        &self,
        session: &mut Session,
        hint: bool,
        ran: Option<&[String]>,
    ) -> String {
        if session.quick_check || session.exam {
            return self.part_card(session, hint, true, ran);
        }
        let next = self.next_step(session, None, None, true);
        let mut out = self.part_card(session, hint, next.is_none(), ran);
        if let Some(part) = session.active_part().cloned()
            && let Some(h) = super::path::horizon(session, &part)
        {
            out.push_str(&format!("\n\n{h}"));
        }
        if let Some(next) = next {
            out.push_str(&format!("\n\n{next}"));
        }
        out
    }

    /// The part's card, what it rests on, and any gaps underneath it (A5),
    /// each raised once, proposed as small parts.
    fn part_card(
        &self,
        session: &mut Session,
        hint: bool,
        read_at: bool,
        ran: Option<&[String]>,
    ) -> String {
        let Some(part) = session.active_part().cloned() else {
            return String::new();
        };
        let lang = session.language.clone();
        let lang = lang.as_deref();
        let mut out = render_part(&part, hint, read_at, lang, &session.cwd);
        if part.concepts.is_empty() {
            return out;
        }
        let labels = self.labels(session);
        let label = |id: &str| labels.get(id).cloned().unwrap_or_else(|| id.to_string());
        let prerequisites = self.store.prerequisites().unwrap_or_default();
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
                .filter(|g| ran.is_none_or(|r| r.contains(&g.concept)))
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

    /// `/why` (local + elsewhere/prior) or `/why --all` (global knowledge graph).
    pub(super) fn why(&self, session: &Session, arg: &str) -> String {
        if arg.split_whitespace().any(|w| w == "--all") {
            return self.global_record(session);
        }
        let Some(part) = session.active_part() else {
            return self.folder_record(session);
        };
        if part.concepts.is_empty() {
            let out = format!(
                "**{}** has no concepts recorded (the map predates them); its status is {}.",
                part.title,
                part.status.label()
            );
            return match agent_work(session) {
                Some(w) => format!("{out}\n\n{w}"),
                None => out,
            };
        }
        use crate::domain::{mermaid_id, mermaid_text};
        use crate::i18n::{Line, line, part_status, state};
        let lang = session.language.clone();
        let lang = lang.as_deref();
        let ws = Self::workspace(session);
        let prerequisites = self.store.prerequisites().unwrap_or_default();
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
        let mut priors = Vec::new();
        for c in &ids {
            let detail = match views.get(c) {
                Some(v) if v.passes + v.fails > 0 => format!(
                    "{} · {}",
                    state(lang, v.state),
                    line(lang, Line::Checks)
                        .replace("{passed}", &v.passes.to_string())
                        .replace("{failed}", &v.fails.to_string())
                ),
                Some(v) => match &v.prior {
                    Some(p) => format!("{} · {}", state(lang, v.state), prior_display(p)),
                    None => state(lang, v.state).to_string(),
                },
                None => state(lang, ConceptState::Unknown).to_string(),
            };
            out.push_str(&format!(
                "    {}[\"{}<br/><small>{}</small>\"]\n",
                mermaid_id(c),
                mermaid_text(&name(c)),
                mermaid_text(&detail),
            ));
            if let Some(v) = views.get(c) {
                if let Some(p) = &v.prior {
                    priors.push(format!("**{}** ({})", name(c), prior_display(p)));
                }
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
        if !priors.is_empty() {
            out.push_str(&format!(
                "\n\nPrior from another workspace: {}. Use `/revisit <concept>` to ignore prior here.",
                priors.join(", ")
            ));
        }
        for n in notes {
            out.push_str(&format!("\n\n{n}"));
        }
        out.push_str(&format!("\n\n{}", line(lang, Line::EvidenceNote)));
        // A10: of the ideas drawn above, how many passed a check and how
        // many rest on your claim alone, so the line matches the drawing.
        // A withdrawn claim no longer counts but stays visible as
        // withdrawn (§15).
        out.push_str(&format!("\n\n{}", evidence_ratio(&ids, &views, lang)));
        let events = self.store.events(&ws, &ids).unwrap_or_default();
        for w in withdrawn_lines(&events, lang, &name) {
            out.push_str(&format!("\n{w}"));
        }
        if let Some(w) = agent_work(session) {
            out.push_str(&format!("\n\n{w}"));
        }
        if let Some(h) = self.step_history(session) {
            out.push_str(&format!("\n\n{h}"));
        }
        out
    }

    /// `/revisit <concept>`: appends `EventKind::Revisit` for the matched
    /// concept in the current workspace (strictly append-only) so prior
    /// knowledge from other workspaces won't be assumed for it here.
    pub(super) fn handle_revisit(&self, session: &mut Session, query: &str) -> TurnOutput {
        let query = query
            .trim()
            .trim_start_matches('`')
            .trim_end_matches('`')
            .trim();
        if query.is_empty() {
            return TurnOutput::notice(
                "Usage: `/revisit <concept>` — ignore prior knowledge from other workspaces for that concept here.",
            );
        }
        let ws = Self::workspace(session);
        let all = self.store.all_concepts().unwrap_or_default();
        if all.is_empty() {
            return TurnOutput::notice("No concepts are recorded yet.");
        }
        let ws_ids: HashSet<String> = self
            .store
            .workspace_concepts(&ws)
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.id)
            .collect();
        let ws_concepts: Vec<Concept> = all
            .iter()
            .filter(|c| ws_ids.contains(&c.id))
            .cloned()
            .collect();

        let matched = exact_match(query, &ws_concepts)
            .or_else(|| understanding::same_named(query, &ws_concepts))
            .or_else(|| exact_match(query, &all))
            .or_else(|| understanding::same_named(query, &all))
            .cloned();

        let concept = match matched {
            Some(c) => c,
            None => {
                let low = query.to_lowercase();
                let slug = slugify(query);
                let mut prefix: Vec<&Concept> = all
                    .iter()
                    .filter(|c| {
                        (!slug.is_empty() && c.id.starts_with(&slug))
                            || c.label.to_lowercase().starts_with(&low)
                    })
                    .collect();
                // Prefer workspace-local matches if there is a unique one.
                let ws_prefix: Vec<&Concept> = prefix
                    .iter()
                    .copied()
                    .filter(|c| ws_ids.contains(&c.id))
                    .collect();
                if ws_prefix.len() == 1 {
                    prefix = ws_prefix;
                }
                match prefix.as_slice() {
                    [one] => (*one).clone(),
                    [] => {
                        return TurnOutput::notice(format!(
                            "No concept matches `{query}`. `/why` or `/why --all` shows recorded concepts."
                        ));
                    }
                    many => {
                        let list = many
                            .iter()
                            .take(6)
                            .map(|c| format!("`{}`", c.id))
                            .collect::<Vec<_>>()
                            .join(", ");
                        return TurnOutput::notice(format!(
                            "`{query}` matches {list}. Type more of the concept name or id."
                        ));
                    }
                }
            }
        };

        self.append_events(
            session,
            vec![NewEvent::new(&concept.id, EventKind::Revisit)],
        );
        if session
            .last_step
            .as_ref()
            .is_some_and(|s| s.concept == concept.id)
        {
            session.last_step = None;
        }
        TurnOutput::text(format!(
            "Marked **{}** (`{}`) for revisit here. Prior knowledge from other workspaces won't be assumed for it in this workspace.",
            concept.label, concept.id
        ))
    }

    /// §16: what Pathfinder keeps about how you learn, all of it: which
    /// kinds of step led you to passes, and how often you steered.
    pub(super) fn step_history(&self, session: &Session) -> Option<String> {
        use crate::i18n::{Line, line};
        let lang = session.language.as_deref();
        let ws = Self::workspace(session);
        let ids: Vec<String> = self
            .store
            .concepts(&ws)
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.id)
            .collect();
        let events = self.store.events(&ws, &ids).unwrap_or_default();
        // Per angle, in plain words, and only angles a verdict followed:
        // a step nobody was judged after says nothing yet.
        let mut by_angle: Vec<(understanding::Angle, u32, u32)> = Vec::new();
        for (kind, (passes, judged)) in understanding::step_stats(&events) {
            let Some(k) = understanding::StepKind::parse(&kind) else {
                continue;
            };
            match by_angle.iter_mut().find(|(a, ..)| *a == k.angle) {
                Some((_, p, j)) => {
                    *p += passes;
                    *j += judged;
                }
                None => by_angle.push((k.angle, passes, judged)),
            }
        }
        let mut kinds: Vec<String> = understanding::Angle::ALL
            .into_iter()
            .filter_map(|a| by_angle.iter().find(|(b, ..)| *b == a))
            .filter(|(_, _, judged)| *judged > 0)
            .map(|(angle, passes, judged)| {
                line(lang, Line::StepKindRate)
                    .replace("{kind}", line(lang, angle_line(*angle)))
                    .replace("{passes}", &passes.to_string())
                    .replace("{judged}", &judged.to_string())
            })
            .collect();
        let steered = understanding::steered_steps(&events);
        if steered > 0 {
            kinds.push(line(lang, Line::SteeredTimes).replace("{n}", &steered.to_string()));
        }
        if kinds.is_empty() {
            return None;
        }
        Some(format!(
            "{}\n{}",
            line(lang, Line::StepHistory).replace("{kinds}", &kinds.join(" · ")),
            line(lang, Line::StepHistoryNote)
        ))
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
        // Ideas with any local event, or workspace-linked with cross-workspace prior,
        // most recently active first.
        let mut active: Vec<(&Concept, i64, Option<Line>)> = concepts
            .iter()
            .filter_map(|c| {
                // A step shown is Pathfinder's activity, not yours.
                let mine: Vec<&crate::understanding::Event> = events
                    .iter()
                    .filter(|e| e.concept == c.id && e.kind != EventKind::Step)
                    .collect();
                if let Some(last) = mine.iter().map(|e| e.id).max() {
                    let told = if mine.iter().any(|e| e.kind == EventKind::Told) {
                        Some(Line::ToldOnly)
                    } else if mine
                        .iter()
                        .all(|e| matches!(e.kind, EventKind::Read | EventKind::Searched))
                    {
                        Some(Line::ReadOnly)
                    } else {
                        None
                    };
                    Some((c, last, told))
                } else if views.get(&c.id).is_some_and(|v| v.prior.is_some()) {
                    Some((c, 0, None))
                } else {
                    None
                }
            })
            .collect();
        if active.is_empty() {
            let none = line(lang, Line::NoRecord).to_string();
            return match agent_work(session) {
                Some(w) => format!("{none}\n\n{w}"),
                None => none,
            };
        }
        active.sort_by_key(|a| std::cmp::Reverse(a.1));
        active.truncate(Self::RECORD_MAX);
        let shown_ids: HashSet<&str> = active.iter().map(|(c, ..)| c.id.as_str()).collect();
        let (mut shown, mut told_only, mut shaky) = (0, 0, 0);
        let mut out = format!(
            "{}\n\n```mermaid\nflowchart LR\n",
            line(lang, Line::RecordTitle)
        );
        for (c, _, told) in &active {
            let st = understanding::state_of(&views, &c.id);
            let prior = views.get(&c.id).and_then(|v| v.prior.as_ref());
            let name = if st == ConceptState::Unknown
                && let Some(p) = prior
            {
                format!("{} · {}", state(lang, st), prior_display(p))
            } else if st == ConceptState::Unknown
                && let Some(mark) = told
            {
                if *mark == Line::ToldOnly {
                    told_only += 1;
                }
                line(lang, *mark).to_string()
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
        for (concept, needs) in self.store.prerequisites().unwrap_or_default() {
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
        let drawn: Vec<String> = active.iter().map(|(c, ..)| c.id.clone()).collect();
        out.push_str(&format!("\n\n{}", evidence_ratio(&drawn, &views, lang)));
        let labels: HashMap<&str, &str> = concepts
            .iter()
            .map(|c| (c.id.as_str(), c.label.as_str()))
            .collect();
        let name = |c: &str| {
            labels
                .get(c)
                .map(|l| l.to_string())
                .unwrap_or(c.to_string())
        };
        for w in withdrawn_lines(&events, lang, &name) {
            out.push_str(&format!("\n{w}"));
        }
        if let Some(w) = agent_work(session) {
            out.push_str(&format!("\n\n{w}"));
        }
        if let Some(h) = self.step_history(session) {
            out.push_str(&format!("\n\n{h}"));
        }
        out
    }

    /// `/why --all`: the user's global knowledge graph across all workspaces.
    pub(super) fn global_record(&self, session: &Session) -> String {
        use crate::i18n::{Line, line, state};
        let lang = session.language.as_deref();
        let ws = Self::workspace(session);
        let concepts = self.store.all_concepts().unwrap_or_default();
        if concepts.is_empty() {
            return line(lang, Line::NoRecord).to_string();
        }
        let ids: Vec<String> = concepts.iter().map(|c| c.id.clone()).collect();
        let events = self.store.events_for_concepts(&ids).unwrap_or_default();
        let now = understanding::now();
        let views = understanding::views_in_workspace(&events, &ws, now);

        let mut by_concept: HashMap<&str, Vec<&understanding::Event>> = HashMap::new();
        let mut all_workspaces: HashSet<&str> = HashSet::new();
        for e in &events {
            if e.kind != EventKind::Step {
                by_concept.entry(e.concept.as_str()).or_default().push(e);
                all_workspaces.insert(e.workspace.as_str());
            }
        }

        let mut active: Vec<(&Concept, i64)> = concepts
            .iter()
            .filter_map(|c| {
                let last = by_concept
                    .get(c.id.as_str())
                    .and_then(|es| es.iter().map(|e| e.id).max())?;
                Some((c, last))
            })
            .collect();
        if active.is_empty() {
            return line(lang, Line::NoRecord).to_string();
        }
        active.sort_by_key(|a| std::cmp::Reverse(a.1));
        active.truncate(Self::RECORD_MAX);
        let shown_ids: HashSet<&str> = active.iter().map(|(c, _)| c.id.as_str()).collect();

        let (mut local_shown, mut elsewhere_shown) = (0usize, 0usize);
        let mut out =
            String::from("Global knowledge record (all workspaces)\n\n```mermaid\nflowchart LR\n");
        for (c, _) in &active {
            let v = views.get(&c.id);
            let local_st = v.map(|v| v.state).unwrap_or(ConceptState::Unknown);
            let status = if local_st.has_evidence() {
                local_shown += 1;
                format!("{} (here)", state(lang, local_st))
            } else if let Some(p) = v.and_then(|v| v.prior.as_ref()) {
                elsewhere_shown += 1;
                prior_display(p)
            } else if local_st != ConceptState::Unknown {
                format!("{} (here)", state(lang, local_st))
            } else if let Some(es) = by_concept.get(c.id.as_str())
                && let Some(last_ev) = es.last()
            {
                let other_ws = last_ev.workspace.as_str();
                let other_events: Vec<understanding::Event> = es
                    .iter()
                    .filter(|e| e.workspace == other_ws)
                    .map(|e| (*e).clone())
                    .collect();
                let other_st = understanding::concept_view(&other_events, now).state;
                format!("{} in {}", state(lang, other_st), workspace_name(other_ws))
            } else {
                state(lang, ConceptState::Unknown).to_string()
            };
            let detail = if c.domain.is_empty() {
                status
            } else {
                format!("{} · {status}", c.domain)
            };
            out.push_str(&format!(
                "    {}[\"{}<br/><small>{}</small>\"]\n",
                crate::domain::mermaid_id(&c.id),
                crate::domain::mermaid_text(&c.label),
                crate::domain::mermaid_text(&detail),
            ));
        }
        for (concept, needs) in self.store.prerequisites().unwrap_or_default() {
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
        out.push_str(&format!(
            "```\n\n{} concepts across {} workspaces: {local_shown} shown here, {elsewhere_shown} shown elsewhere.",
            active.len(),
            all_workspaces.len().max(1)
        ));
        out
    }
}

/// Concept state string for LLM prompts: never includes raw workspace paths or
/// repo names from other workspaces.
fn prompt_state(views: &HashMap<String, ConceptView>, concept: &str) -> &'static str {
    match views.get(concept) {
        Some(v) if v.state != ConceptState::Unknown => v.state.label(),
        Some(v) => match &v.prior {
            Some(PriorState::SolidElsewhere { .. }) => "prior: solid",
            Some(PriorState::ShownElsewhere { .. }) => "prior: shown",
            None => ConceptState::Unknown.label(),
        },
        None => ConceptState::Unknown.label(),
    }
}

/// Short display name of a workspace path for local user-facing inspection
/// (`/why`, `/why --all`). Never used in LLM prompts.
fn workspace_name(ws: &str) -> String {
    std::path::Path::new(ws)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(ws)
        .to_string()
}

fn prior_display(prior: &PriorState) -> String {
    match prior {
        PriorState::SolidElsewhere { workspace } => {
            format!("solid in {}", workspace_name(workspace))
        }
        PriorState::ShownElsewhere { workspace } => {
            format!("shown in {}", workspace_name(workspace))
        }
    }
}

/// Whether two topic/domain tags clearly belong to different domains.
/// Empty domains never conflict (legacy concepts or untagged extractor items).
fn domains_conflict(a: &str, b: &str) -> bool {
    let na = slugify(a);
    let nb = slugify(b);
    if na.is_empty() || nb.is_empty() || na == nb {
        return false;
    }
    let ta: HashSet<&str> = na.split('-').filter(|w| w.len() >= 2).collect();
    let tb: HashSet<&str> = nb.split('-').filter(|w| w.len() >= 2).collect();
    ta.is_disjoint(&tb)
}

fn compatible_match<'a>(
    id: &str,
    label: &str,
    domain: &str,
    concepts: &'a [Concept],
) -> Option<&'a Concept> {
    let compatible: Vec<Concept> = concepts
        .iter()
        .filter(|c| !domains_conflict(domain, &c.domain))
        .cloned()
        .collect();
    let matched_id = exact_match(id, &compatible)
        .or_else(|| exact_match(label, &compatible))
        .or_else(|| understanding::same_named(label, &compatible))?
        .id
        .clone();
    concepts.iter().find(|c| c.id == matched_id)
}

fn query_words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(|w| {
            let low = w.to_lowercase();
            if low.chars().count() > 3 && low.ends_with('s') && !low.ends_with("ss") {
                low[..low.len() - 1].to_string()
            } else {
                low
            }
        })
        .collect()
}

fn goal_query_tokens(session: &Session) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Some(goal) = &session.goal {
        out.extend(query_words(goal));
    }
    for part in &session.map.parts {
        out.extend(query_words(&part.title));
    }
    for turn in session
        .history
        .iter()
        .rev()
        .filter(|t| t.role == "user")
        .take(6)
    {
        out.extend(query_words(&turn.content));
    }
    out
}

fn relevance_score(concept: &Concept, query: &HashSet<String>) -> usize {
    if query.is_empty() {
        return 0;
    }
    let mut have = query_words(&concept.id);
    have.extend(query_words(&concept.label));
    have.extend(query_words(&concept.domain));
    for a in &concept.aliases {
        have.extend(query_words(a));
    }
    query.intersection(&have).count()
}

fn truncate_note(text: &str, max_chars: usize) -> String {
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= max_chars {
        cleaned
    } else {
        let mut s: String = cleaned.chars().take(max_chars).collect();
        s.push('…');
        s
    }
}

/// A10's evidence ratio over exactly the ideas a record draws: those with
/// a passed check, and those resting on your claim alone (a claimed idea
/// you later passed counts as passed, as it is drawn).
fn evidence_ratio(
    ids: &[String],
    views: &HashMap<String, understanding::ConceptView>,
    lang: Option<&str>,
) -> String {
    use crate::i18n::{Line, line};
    let passed = ids
        .iter()
        .filter(|c| views.get(*c).is_some_and(|v| v.passes > 0))
        .count();
    let claimed = ids
        .iter()
        .filter(|c| understanding::state_of(views, c) == ConceptState::Claimed)
        .count();
    line(lang, Line::EvidenceRatio)
        .replace("{passes}", &passed.to_string())
        .replace("{claims}", &claimed.to_string())
}

/// §12 labels for the work handed to the agent, so a part handed off or
/// delegated stays in the record after you move on: reviewable when you
/// had shown what it rests on, delegated when you hadn't.
fn agent_work(session: &Session) -> Option<String> {
    use crate::i18n::{Line, line};
    let lang = session.language.as_deref();
    let rows: Vec<String> = session
        .map
        .parts
        .iter()
        .filter_map(|p| {
            let which = match p.status {
                PartStatus::InReview => Line::AgentInReview,
                PartStatus::Done if p.review_missed => Line::AgentMissedCheck,
                PartStatus::Done if p.review_caught => Line::AgentCaughtCheck,
                PartStatus::Done => Line::AgentApproved,
                PartStatus::Delegated => Line::AgentDelegated,
                _ => return None,
            };
            Some(format!(
                "- {}",
                line(lang, which).replace("{title}", &p.title)
            ))
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    Some(format!(
        "{}\n{}",
        line(lang, Line::AgentWork),
        rows.join("\n")
    ))
}

/// The step history's plain words for an angle.
fn angle_line(a: understanding::Angle) -> crate::i18n::Line {
    use crate::i18n::Line;
    match a {
        understanding::Angle::Example => Line::AngleExample,
        understanding::Angle::Purpose => Line::AnglePurpose,
        understanding::Angle::Mechanism => Line::AngleMechanism,
    }
}

/// Most withdrawn claims `/why` lists, the newest first.
const WITHDRAWN_MAX: usize = 5;

/// "Claim on **X** withdrawn (2026-11-27)" for each claim `/undo` took
/// back: never silently erased (§15).
fn withdrawn_lines(
    events: &[understanding::Event],
    lang: Option<&str>,
    name: &dyn Fn(&str) -> String,
) -> Vec<String> {
    use crate::i18n::{Line, line};
    let withdrawn = understanding::retracted_claims(events);
    let mut out: Vec<(i64, String)> = events
        .iter()
        .filter(|e| e.kind == EventKind::Retracted)
        .filter(|e| {
            e.reference
                .as_deref()
                .and_then(|r| r.parse::<i64>().ok())
                .is_some_and(|id| withdrawn.contains(&id))
        })
        .map(|e| {
            let date = jiff::Timestamp::from_second(e.at)
                .map(|t| {
                    let local = t.to_zoned_datetime(jiff::tz::TimeZone::system());
                    jiff::fmt::strtime::format("%Y-%m-%d", &local).unwrap_or_default()
                })
                .unwrap_or_default();
            (
                e.id,
                line(lang, Line::ClaimWithdrawn)
                    .replace("{concept}", &name(&e.concept))
                    .replace("{date}", &date),
            )
        })
        .collect();
    out.sort_by_key(|(id, _)| std::cmp::Reverse(*id));
    out.dedup_by(|a, b| a.1 == b.1);
    out.into_iter()
        .take(WITHDRAWN_MAX)
        .map(|(_, l)| l)
        .collect()
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
