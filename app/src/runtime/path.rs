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
    /// The next step on the active part (§16): one exact place and one
    /// open question. The question is `question` when a model wrote one
    /// from the part and its source, else a fixed one. None while any
    /// secret is active (a pointer could lead to what settles it), in a
    /// quick check or an exam, or when the part isn't open or has nowhere
    /// to point. The step is recorded as a `step` event when it is `fresh`
    /// (you entered the part or steered) or differs from the last one.
    pub(super) fn next_step(
        &self,
        session: &mut Session,
        steer: Option<Angle>,
        question: Option<&str>,
        fresh: bool,
    ) -> Option<String> {
        if session.quick_check || session.exam {
            return None;
        }
        if !matches!(self.gate_mode(session), gate::GateMode::Open) {
            return None;
        }
        let part = session.active_part()?.clone();
        if part.status != PartStatus::Open {
            return None;
        }
        let question = question.map(str::trim).filter(|q| !q.is_empty());
        let lang = session.language.clone();
        let lang = lang.as_deref();
        // No concept to aim at yet: the part's own source, if it has one.
        let part_step = |session: &Session| {
            let at = sources::pointer_note(&session.cwd, &part.note)?;
            let q = question
                .map(str::to_string)
                .unwrap_or_else(|| i18n::line(lang, i18n::Line::StepPart).to_string());
            Some(
                i18n::line(lang, i18n::Line::NextStep)
                    .replace("{at}", &at)
                    .replace("{question}", &q),
            )
        };
        if part.concepts.is_empty() {
            return part_step(session);
        }
        let Some(views) = self.concept_views(session, &part.concepts).ok() else {
            return part_step(session);
        };
        let Some(target) = understanding::targets(&part.concepts, &views)
            .and_then(|t| t.concepts.into_iter().next())
        else {
            return part_step(session);
        };
        let ws = Self::workspace(session);
        let concepts = self.store.concepts(&ws).unwrap_or_default();
        let ids: Vec<String> = concepts.iter().map(|c| c.id.clone()).collect();
        let stats = understanding::step_stats(&self.store.events(&ws, &ids).unwrap_or_default());
        let view = views
            .get(&target)
            .cloned()
            .unwrap_or_else(|| understanding::concept_view(&[], understanding::now()));
        let Some(mut kind) = understanding::choose_step(&view, steer, &stats) else {
            return part_step(session);
        };
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
        // Avoid repeating the exact same fixed `Next:` question verbatim
        // once the user has already seen or answered a step on this
        // concept/kind without advancing.
        let fixed_for =
            |k: StepKind| i18n::line(lang, step_question(k)).replace("{concept}", &concept.label);
        if steer.is_none()
            && question.is_none()
            && let Some(prev) = session
                .last_step
                .as_ref()
                .filter(|l| l.concept == target && l.kind == kind.as_str())
        {
            let fixed = fixed_for(kind);
            if prev.repeats >= 1 || (!prev.question.is_empty() && prev.question != fixed) {
                kind.angle = next_angle(kind.angle);
            }
        }
        let model_question = question.is_some();
        let question = question
            .map(str::to_string)
            .unwrap_or_else(|| fixed_for(kind));
        let line = match place(&session.cwd, &concept, kind.scope, &part, &session.map) {
            Some(at) => i18n::line(lang, i18n::Line::NextStep)
                .replace("{at}", &at)
                .replace("{question}", &question),
            None => i18n::line(lang, i18n::Line::NextStepNoPlace)
                .replace("{concept}", &concept.label)
                .replace("{question}", &question),
        };
        let prev_repeats = session
            .last_step
            .as_ref()
            .filter(|l| l.concept == target && l.kind == kind.as_str())
            .map_or(0, |l| l.repeats.saturating_add(1));
        let repeated = session
            .last_step
            .as_ref()
            .is_some_and(|l| l.concept == target && l.kind == kind.as_str());
        if fresh || !repeated {
            self.append_events(
                session,
                vec![
                    NewEvent::new(&target, EventKind::Step)
                        .reference(part.id.clone())
                        .detail(json!({
                            "kind": kind.as_str(),
                            "steered": steer.is_some(),
                            // Whose question was asked: the Tutor's, or a
                            // fixed line.
                            "question": if model_question { "model" } else { "fixed" },
                        })),
                ],
            );
        }
        session.last_step = Some(crate::domain::StepShown {
            concept: target,
            kind: kind.as_str(),
            question,
            repeats: if fresh || !repeated { 0 } else { prev_repeats },
        });
        Some(line)
    }
}

/// Rotates to the next angle when a fixed step question would otherwise
/// repeat verbatim.
fn next_angle(a: Angle) -> Angle {
    match a {
        Angle::Purpose => Angle::Mechanism,
        Angle::Mechanism => Angle::Example,
        Angle::Example => Angle::Purpose,
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
/// either side, in the file the part's note points to and within the
/// part's own pages (from its page to just before the next part's, at most
/// `NEAR_PAGES` on), nearest its page; else the place the note points to;
/// else, for a part with no pointer, lines anywhere in your sources. Found
/// by code (`/find`'s search), never invented. One place: the part's
/// pointer and the concept's never compete, and a step never sends you
/// into another part's slides (§16).
fn place(
    cwd: &std::path::Path,
    concept: &Concept,
    scope: Scope,
    part: &crate::domain::Part,
    map: &crate::domain::Map,
) -> Option<String> {
    let mut terms = sources::keywords(&concept.label);
    for alias in &concept.aliases {
        for k in sources::keywords(alias) {
            if !terms.contains(&k) {
                terms.push(k);
            }
        }
    }
    let span = match scope {
        Scope::Wide => WIDE_SPAN,
        Scope::Narrow => NARROW_SPAN,
    };
    let render = |file: &str, page: Option<usize>, line: usize| {
        let last = sources::line_count(cwd, file, page)
            .unwrap_or(line)
            .max(line);
        let (from, to) = (line.saturating_sub(span).max(1), (line + span).min(last));
        match page {
            Some(p) => format!("`{file} p.{p} l.{from}-{to}`"),
            None => format!("`{file}:{from}-{to}`"),
        }
    };
    let hits: Vec<Hit> = sources::find(cwd, &terms, PLACE_HITS)
        .into_iter()
        .filter_map(|h| match h.at {
            Locator::Local { file, page, line } => Some((file, page, line)),
            Locator::Web { .. } => None,
        })
        .collect();
    if let Some(at) = super::refer::pointer(cwd, &part.note) {
        let others: Vec<super::refer::Pointer> = super::refer::pointers(map, cwd)
            .into_iter()
            .filter(|(id, _)| *id != part.id)
            .map(|(_, p)| p)
            .collect();
        let last = last_page(&at, page_range_end(&part.note), &others);
        if let Some((file, page, line)) = near_hit(&hits, &at, last) {
            return Some(render(file, *page, *line));
        }
        return sources::pointer_note(cwd, &part.note);
    }
    hits.first()
        .map(|(file, page, line)| render(file, *page, *line))
}

/// A search hit: file, page (PDFs), line.
type Hit = (String, Option<usize>, usize);

/// The last page a step on the part at `at` may point to: its own page
/// plus `NEAR_PAGES`, cut at the end of the range its note gives
/// (`range_end`) and just before the next part's first page in the same
/// file. None for a pointer without a page.
fn last_page(
    at: &super::refer::Pointer,
    range_end: Option<usize>,
    others: &[super::refer::Pointer],
) -> Option<usize> {
    let first = at.page?;
    let mut last = first + NEAR_PAGES;
    if let Some(end) = range_end.filter(|e| *e >= first) {
        last = last.min(end);
    }
    if let Some(next) = others
        .iter()
        .filter(|o| o.file == at.file)
        .filter_map(|o| o.page)
        .filter(|p| *p > first)
        .min()
    {
        last = last.min(next - 1);
    }
    Some(last)
}

/// The hit nearest the part's own place in its own file: on a page from
/// the part's page to `last`, the exact page first; for a text file, the
/// nearest line. None when no hit is on the part.
fn near_hit<'a>(
    hits: &'a [Hit],
    at: &super::refer::Pointer,
    last: Option<usize>,
) -> Option<&'a Hit> {
    let mine = hits.iter().filter(|(f, _, _)| *f == at.file);
    match (at.page, last) {
        (Some(first), Some(last)) => mine
            .filter(|(_, page, _)| page.is_some_and(|p| (first..=last).contains(&p)))
            .min_by_key(|(_, page, line)| (page.unwrap_or(first) - first, *line)),
        _ => mine.min_by_key(|(_, _, line)| at.line.map_or(*line, |want| want.abs_diff(*line))),
    }
}

/// The last page of a range a note gives ("pp. 24–28", "S. 3-5").
fn page_range_end(note: &str) -> Option<usize> {
    let low = note.to_lowercase();
    for key in ["pp.", "pp ", "pages ", "seiten ", "s. ", "p. ", "p."] {
        let mut rest = low.as_str();
        while let Some(i) = rest.find(key) {
            let after = rest[i + key.len()..].trim_start();
            let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
            let tail = after[digits.len()..].trim_start();
            if !digits.is_empty()
                && let Some(t) = tail.strip_prefix(['–', '-', '—'])
            {
                let end: String = t
                    .trim_start()
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                if let Ok(end) = end.parse() {
                    return Some(end);
                }
            }
            rest = &rest[i + key.len()..];
        }
    }
    None
}

/// Lines `place` weighs, at most.
const PLACE_HITS: usize = 200;

/// How many pages after the part's own page a hit may be and still be
/// on that part, when nothing nearer ends it.
const NEAR_PAGES: usize = 3;

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

#[cfg(test)]
mod tests {
    use super::super::refer::Pointer;
    use super::*;

    fn at(file: &str, page: usize) -> Pointer {
        Pointer {
            file: file.into(),
            page: Some(page),
            line: None,
        }
    }

    #[test]
    fn a_step_stays_within_the_parts_own_slides() {
        // FUB: blackbody p.21, photoeffect p.22, bohr-lines p.23.
        let f = "FUB_Folien1.pdf";
        let others = [at(f, 22), at(f, 23), at(f, 24), at("other.pdf", 21)];
        let last = last_page(&at(f, 21), None, &others);
        assert_eq!(last, Some(21));
        let hits: Vec<Hit> = vec![
            (f.into(), Some(23), 1),
            (f.into(), Some(20), 3),
            ("other.pdf".into(), Some(21), 2),
        ];
        // Only hits on another part's slides: none, so the part's pointer.
        assert_eq!(near_hit(&hits, &at(f, 21), last), None);
        let mut hits = hits;
        hits.push((f.into(), Some(21), 9));
        assert_eq!(
            near_hit(&hits, &at(f, 21), last),
            Some(&(f.to_string(), Some(21), 9))
        );
        // Alone in its file: up to NEAR_PAGES on, the exact page first.
        let last = last_page(&at(f, 21), None, &[]);
        assert_eq!(last, Some(21 + NEAR_PAGES));
        let hits: Vec<Hit> = vec![(f.into(), Some(23), 1), (f.into(), Some(22), 5)];
        assert_eq!(
            near_hit(&hits, &at(f, 21), last),
            Some(&(f.to_string(), Some(22), 5))
        );
        // A range in the note ends it sooner.
        assert_eq!(page_range_end("FUB_Folien1.pdf pp. 24–25"), Some(25));
        assert_eq!(last_page(&at(f, 24), Some(25), &[]), Some(25));
        assert_eq!(page_range_end("FUB_Folien1.pdf p. 21"), None);
    }
}
