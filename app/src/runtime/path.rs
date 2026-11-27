//! §16 of docs/theory.md: the next step, exactly (a place in your sources
//! and an open question to take there), and the horizon, loosely (the
//! map's parts and what they depend on).
//!
//! The kind of step comes from the understanding model's state, your steer
//! and the kinds that led you to passes before (`understanding::choose_step`).
//! None of it goes into a model's prompt, and the only thing kept about how
//! you learn is the `step` events, which `/why` shows.

use serde_json::json;

use super::*;
use crate::sources::{self, Locator};
use crate::understanding::{self, Angle, Concept, NewEvent, Scope, StepKind};

/// Lines shown around a hit, either side: wider for an orienting step.
const WIDE_SPAN: usize = 10;
const NARROW_SPAN: usize = 4;

impl Runtime {
    /// The next step on the active part, recorded as a `step` event: an
    /// exact place and an open question. None while any secret is active
    /// (a pointer could lead to what settles it), in a quick check or an
    /// exam, or when no core concept needs a step. `only_if_new`: None when
    /// it is the step shown last.
    pub(super) fn next_step(
        &self,
        session: &mut Session,
        steer: Option<Angle>,
        only_if_new: bool,
    ) -> Option<String> {
        if session.quick_check || session.exam {
            return None;
        }
        if !matches!(self.gate_mode(session), gate::GateMode::Open) {
            return None;
        }
        let part = session.active_part()?.clone();
        if part.concepts.is_empty() || part.status != PartStatus::Open {
            return None;
        }
        let views = self.concept_views(session, &part.concepts).ok()?;
        let target = understanding::targets(&part.concepts, &views)?
            .concepts
            .into_iter()
            .next()?;
        let ws = Self::workspace(session);
        let concepts = self.store.concepts(&ws).unwrap_or_default();
        let ids: Vec<String> = concepts.iter().map(|c| c.id.clone()).collect();
        let stats = understanding::step_stats(&self.store.events(&ws, &ids).unwrap_or_default());
        let view = views
            .get(&target)
            .cloned()
            .unwrap_or_else(|| understanding::concept_view(&[], understanding::now()));
        let kind = understanding::choose_step(&view, steer, &stats)?;
        if only_if_new
            && session
                .last_step
                .as_ref()
                .is_some_and(|l| l.concept == target && l.kind == kind.as_str())
        {
            return None;
        }
        let concept = concepts
            .iter()
            .find(|c| c.id == target)
            .cloned()
            .unwrap_or(Concept {
                id: target.clone(),
                label: target.replace('-', " "),
                aliases: vec![],
                created_by: "map".into(),
            });
        let lang = session.language.as_deref();
        let question = i18n::line(lang, step_question(kind)).replace("{concept}", &concept.label);
        let line = match place(&session.cwd, &concept, kind.scope, &part.note) {
            Some(at) => i18n::line(lang, i18n::Line::NextStep)
                .replace("{at}", &at)
                .replace("{question}", &question),
            None => i18n::line(lang, i18n::Line::NextStepNoPlace)
                .replace("{concept}", &concept.label)
                .replace("{question}", &question),
        };
        self.append_events(
            session,
            vec![
                NewEvent::new(&target, EventKind::Step)
                    .reference(part.id.clone())
                    .detail(json!({"kind": kind.as_str(), "steered": steer.is_some()})),
            ],
        );
        session.last_step = Some(crate::domain::StepShown {
            concept: target,
            kind: kind.as_str(),
        });
        Some(line)
    }
}

/// The open question for a kind of step.
fn step_question(kind: StepKind) -> i18n::Line {
    use i18n::Line;
    match (kind.scope, kind.angle) {
        (Scope::Wide, Angle::Example) => Line::StepWideExample,
        (Scope::Wide, Angle::Purpose) => Line::StepWidePurpose,
        (Scope::Wide, Angle::Mechanism) => Line::StepWideMechanism,
        (Scope::Narrow, Angle::Example) => Line::StepNarrowExample,
        (Scope::Narrow, Angle::Purpose) => Line::StepNarrowPurpose,
        (Scope::Narrow, Angle::Mechanism) => Line::StepNarrowMechanism,
    }
}

/// Where to look for a concept: the source lines that name it, a few
/// either side; else the place the part's note points to. Found by code
/// (`/find`'s search), never invented.
fn place(cwd: &std::path::Path, concept: &Concept, scope: Scope, note: &str) -> Option<String> {
    let mut terms = sources::keywords(&concept.label);
    for alias in &concept.aliases {
        for k in sources::keywords(alias) {
            if !terms.contains(&k) {
                terms.push(k);
            }
        }
    }
    if let Some(sources::Hit {
        at: Locator::Local { file, page, line },
        ..
    }) = sources::find(cwd, &terms, 1).into_iter().next()
    {
        let span = match scope {
            Scope::Wide => WIDE_SPAN,
            Scope::Narrow => NARROW_SPAN,
        };
        let last = sources::line_count(cwd, &file, page)
            .unwrap_or(line)
            .max(line);
        let (from, to) = (line.saturating_sub(span).max(1), (line + span).min(last));
        return Some(match page {
            Some(p) => format!("`{file} p.{p} l.{from}-{to}`"),
            None => format!("`{file}:{from}-{to}`"),
        });
    }
    sources::pointer_note(cwd, note)
}

/// §16's horizon: what the part needs and what it leads to, with their
/// statuses. No order of steps. None when it stands alone.
pub(super) fn horizon(session: &Session, part: &crate::domain::Part) -> Option<String> {
    use i18n::Line;
    let lang = session.language.as_deref();
    let needs: Vec<String> = part
        .depends_on
        .iter()
        .filter_map(|id| session.map.get(id))
        .map(|p| format!("**{}** ({})", p.title, i18n::part_status(lang, p.status)))
        .collect();
    let leads: Vec<String> = session
        .map
        .parts
        .iter()
        .filter(|p| p.depends_on.contains(&part.id))
        .map(|p| format!("**{}**", p.title))
        .collect();
    let mut what = Vec::new();
    if !needs.is_empty() {
        what.push(i18n::line(lang, Line::HorizonNeeds).replace("{parts}", &needs.join(", ")));
    }
    if !leads.is_empty() {
        what.push(i18n::line(lang, Line::HorizonLeads).replace("{parts}", &leads.join(", ")));
    }
    (!what.is_empty()).then(|| i18n::line(lang, Line::Horizon).replace("{what}", &what.join(" · ")))
}
