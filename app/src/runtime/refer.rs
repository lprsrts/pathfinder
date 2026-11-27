//! §15 and A3 of docs/theory.md, read by code: whether plain words may be
//! a commitment the decision model missed ("I understand this part"), and
//! which map part a message means ("the part in Folie 1 page 19", "this
//! part", a slide, a page, a file or a concept name).
//!
//! Both only ever lead to a question back to you: nothing here records
//! anything.

use std::collections::HashMap;
use std::path::Path;

use crate::domain::{Map, Part};
use crate::sources;

/// A message this long or longer is an explanation, not a bare claim.
const CLAIM_MAX_WORDS: usize = 16;

/// Words people put before a claim ("ok, so I get it").
const FILLER: &[&str] = &[
    "ok", "okay", "so", "yes", "yeah", "well", "now", "alright", "right", "fine", "good", "i",
    "think", "guess", "believe", "ja", "also", "gut", "jetzt", "ich", "glaube", "denke",
];

/// Openings that claim understanding, after any filler, lowercased with
/// plain apostrophes.
const CLAIMS: &[&str] = &[
    "understand",
    "understood",
    "get it",
    "get this",
    "got it",
    "got this",
    "get the",
    "know this",
    "know it",
    "know the",
    "already know",
    "have understood",
    "'ve understood",
    "i'm good with",
    "am good with",
    "makes sense",
    "this makes sense",
    "it makes sense",
    "clear now",
    "it's clear",
    "that's clear",
    "verstehe",
    "verstanden",
    "habe es verstanden",
    "hab es verstanden",
    "weiß ich",
    "kenne ich",
    "kann ich",
];

/// Words that turn a claim into something else: a doubt, a part they
/// don't get, a question.
const HEDGES: &[&str] = &[
    "but", "not", "n't", "except", "don't", "dont", "unless", "why", "how", "nicht", "aber",
    "kein", "warum", "wieso",
];

/// Words that start an explanation after a claim.
const EXPLAINS: &[&str] = &[
    " because ",
    " since ",
    " that ",
    " so that ",
    " which ",
    " means ",
    " weil ",
    " dass ",
];

/// Openings that ask to be told outright.
const TELLS: &[&str] = &[
    "tell me",
    "just tell",
    "can you tell me",
    "could you tell me",
    "please tell me",
    "give me the answer",
    "explain it to me",
    "sag mir",
    "sag es mir",
    "erklär mir",
];

/// Openings that ask the agent to do the part.
const AGENTS: &[&str] = &[
    "do it for me",
    "you do it",
    "let the agent",
    "the agent can",
    "hand it off",
    "hand this off",
    "delegate",
    "mach du",
];

/// The commitment plain words plausibly express, short of explaining
/// anything: `known`, `tell` or `agent` (the Router's step names). None
/// for anything else, an explanation or a question included.
pub(super) fn plain_commitment(text: &str) -> Option<&'static str> {
    let low = text.trim().to_lowercase().replace(['’', '‘'], "'");
    if low.is_empty() {
        return None;
    }
    let starts = |phrases: &[&str]| {
        let mut rest = low.as_str();
        // Skip leading filler words, one at a time.
        loop {
            let t = rest.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '\'');
            if phrases.iter().any(|p| t.starts_with(p)) {
                return true;
            }
            let word_end = t
                .find(|c: char| !c.is_alphanumeric() && c != '\'')
                .unwrap_or(t.len());
            let word = &t[..word_end];
            if word.is_empty() || !FILLER.contains(&word) {
                return false;
            }
            rest = &t[word_end..];
        }
    };
    if starts(TELLS) {
        return Some(super::TELL);
    }
    if starts(AGENTS) {
        return Some(super::AGENT);
    }
    let words: Vec<&str> = low.split_whitespace().collect();
    if words.len() >= CLAIM_MAX_WORDS || low.contains('?') {
        return None;
    }
    // "I get it: it routes by name" explains: a summary, not a bare claim.
    if low.contains([':', ';', '=']) || EXPLAINS.iter().any(|e| low.contains(e)) {
        return None;
    }
    (!hedged(&low) && starts(CLAIMS)).then_some("known")
}

/// Whether lowercased `low` doubts or denies something ("not", "don't",
/// "weiß nicht", "but"): a whole word, or a word ending in "n't".
pub(super) fn hedged(low: &str) -> bool {
    low.replace(['’', '‘'], "'").split_whitespace().any(|w| {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'');
        HEDGES
            .iter()
            .any(|h| w == *h || (h.starts_with('n') && w.ends_with(h)))
    })
}

/// Which part a message means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PartRef {
    /// It names no part: the active one.
    Active,
    /// Exactly this part.
    One(String),
    /// One of these, best first: ask which.
    Many(Vec<String>),
}

/// Where a part's note points: a workspace file and a page (PDF) or line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pointer {
    pub file: String,
    pub page: Option<usize>,
    pub line: Option<usize>,
}

/// Where `note` points, read back from `sources::pointer_note`'s rendering
/// (`` `file p.3` ``, `` `file:12` `` or `` `file` ``).
pub(super) fn pointer(cwd: &Path, note: &str) -> Option<Pointer> {
    let shown = sources::pointer_note(cwd, note)?;
    let inner = shown.trim().trim_matches('`');
    if let Some((file, page)) = inner.rsplit_once(" p.")
        && let Ok(page) = page.parse()
    {
        return Some(Pointer {
            file: file.into(),
            page: Some(page),
            line: None,
        });
    }
    if let Some((file, line)) = inner.rsplit_once(':')
        && let Ok(line) = line.parse()
    {
        return Some(Pointer {
            file: file.into(),
            page: None,
            line: Some(line),
        });
    }
    Some(Pointer {
        file: inner.into(),
        page: None,
        line: None,
    })
}

/// Words and numbers, lowercased: "FUB_Folien1.pdf" is `fub`, `folien`,
/// `1`, `pdf`.
fn tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut digit = false;
    for c in text.to_lowercase().chars() {
        let d = c.is_ascii_digit();
        if c.is_alphanumeric() {
            if !cur.is_empty() && d != digit {
                out.push(std::mem::take(&mut cur));
            }
            digit = d;
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Two words alike enough to be one name ("folie" and "folien").
pub(super) fn alike(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    common >= 4 && common + 2 >= a.chars().count().max(b.chars().count())
}

/// Words that put a page number after them.
const PAGE_WORDS: &[&str] = &[
    "page", "pages", "p", "pp", "pg", "seite", "s", "slide", "slides",
];

/// What a message says about a source: whether it names `file` (by its
/// name, or by a word and number in it: "Folie 1" for `FUB_Folien1.pdf`),
/// and the page it names, if any. Numbers that name the file aren't pages.
fn source_ref(msg: &[String], file: &str) -> (bool, Option<usize>) {
    let stem = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ft = tokens(&stem);
    let mut named = false;
    let mut used = vec![false; msg.len()];
    // The whole stem, in order.
    if !ft.is_empty() && msg.windows(ft.len()).any(|w| w == ft.as_slice()) {
        named = true;
    }
    // A word of the name followed by its number.
    for pair in ft.windows(2) {
        let (word, num) = (&pair[0], &pair[1]);
        if !num.chars().all(|c| c.is_ascii_digit()) || word.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        for i in 0..msg.len().saturating_sub(1) {
            if alike(&msg[i], word) && msg[i + 1] == *num {
                named = true;
                used[i + 1] = true;
            }
        }
    }
    // A distinctive word of the name on its own (at least 5 letters, and
    // not one a number follows: "Folien 2" is not `FUB_Folien1.pdf`).
    if !named {
        named = ft.iter().enumerate().any(|(i, w)| {
            let numbered = ft
                .get(i + 1)
                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()));
            w.chars().count() >= 5
                && !numbered
                && !w.chars().all(|c| c.is_ascii_digit())
                && msg.iter().any(|m| alike(m, w))
        });
    }
    let page = page_number(msg, &used);
    (named, page)
}

/// The page a message names ("page 19", "p. 19", "slide 19", "Seite 19").
fn page_number(msg: &[String], used: &[bool]) -> Option<usize> {
    (1..msg.len())
        .find(|&i| !used[i] && PAGE_WORDS.contains(&msg[i - 1].as_str()))
        .and_then(|i| msg[i].parse().ok())
}

/// Where each part's note points, by part id.
pub(super) fn pointers(map: &Map, cwd: &Path) -> HashMap<String, Pointer> {
    map.parts
        .iter()
        .filter_map(|p| pointer(cwd, &p.note).map(|ptr| (p.id.clone(), ptr)))
        .collect()
}

/// Which part `text` means, from the parts' titles and concept labels
/// (`labels`), where their notes point (`at`, from [`pointers`]), and the
/// active part. Code alone, no model.
pub(super) fn resolve_part(
    map: &Map,
    active: Option<&str>,
    at: &HashMap<String, Pointer>,
    labels: &HashMap<String, String>,
    text: &str,
) -> PartRef {
    let msg = tokens(text);
    let open: Vec<&Part> = map.parts.iter().collect();
    if open.is_empty() {
        return PartRef::Active;
    }
    // 1. A source: a file, a page, or both.
    let pointers: Vec<(&Part, Pointer)> = open
        .iter()
        .filter_map(|p| at.get(&p.id).map(|ptr| (*p, ptr.clone())))
        .collect();
    let mut by_source: Vec<(&Part, usize)> = Vec::new();
    let mut page_asked = None;
    for (part, ptr) in &pointers {
        let (named, page) = source_ref(&msg, &ptr.file);
        page_asked = page_asked.or(page);
        let score = match (named, page, ptr.page) {
            (true, Some(want), Some(at)) if want == at => 4,
            (true, Some(_), _) => 2,
            (true, None, _) => 2,
            (false, Some(want), Some(at)) if want == at => 3,
            _ => 0,
        };
        if score > 0 {
            by_source.push((part, score));
        }
    }
    // A page with no part starting on it: the part in that file whose
    // pointer comes last before it.
    if let Some(want) = page_asked
        && !by_source.iter().any(|(_, s)| *s >= 3)
    {
        let named: Vec<&(&Part, Pointer)> = pointers
            .iter()
            .filter(|(p, _)| by_source.iter().any(|(q, _)| q.id == p.id))
            .collect();
        let before = named
            .iter()
            .filter(|(_, ptr)| ptr.page.is_some_and(|at| at <= want))
            .max_by_key(|(_, ptr)| ptr.page);
        if let Some((part, _)) = before {
            return PartRef::One(part.id.clone());
        }
    }
    if let Some(best) = by_source.iter().map(|(_, s)| *s).max() {
        let ids: Vec<String> = by_source
            .iter()
            .filter(|(_, s)| *s == best)
            .map(|(p, _)| p.id.clone())
            .collect();
        return one_or_many(ids);
    }
    // 2. A name: the part's title, or a concept it rests on.
    let words: Vec<String> = sources::keywords(text);
    let covers = |name: &str| {
        let need = sources::keywords(name);
        if need.is_empty() {
            return 0.0;
        }
        let hit = need
            .iter()
            .filter(|n| words.iter().any(|w| alike(w, n)))
            .count();
        hit as f64 / need.len() as f64
    };
    let mut by_name: Vec<(String, f64)> = open
        .iter()
        .map(|p| {
            let title = covers(&p.title);
            let concept = p
                .concepts
                .iter()
                .map(|c| covers(labels.get(c).map(String::as_str).unwrap_or(c)))
                .fold(0.0, f64::max);
            (p.id.clone(), title.max(concept))
        })
        .filter(|(_, s)| *s >= 0.6)
        .collect();
    by_name.sort_by(|a, b| b.1.total_cmp(&a.1));
    if let Some(best) = by_name.first().map(|(_, s)| *s) {
        let ids: Vec<String> = by_name
            .into_iter()
            .filter(|(_, s)| (best - s).abs() < 1e-9)
            .map(|(id, _)| id)
            .collect();
        // The active part among equals is the one you're on.
        if let Some(a) = active.filter(|a| ids.iter().any(|i| i == a)) {
            return PartRef::One(a.to_string());
        }
        return one_or_many(ids);
    }
    // 3. "This part", "it", nothing: the one you're on.
    PartRef::Active
}

fn one_or_many(mut ids: Vec<String>) -> PartRef {
    ids.dedup();
    match ids.len() {
        0 => PartRef::Active,
        1 => PartRef::One(ids.remove(0)),
        _ => PartRef::Many(ids),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Part;

    #[test]
    fn bare_claims_are_possible_commitments() {
        for t in [
            "I understand the part in Folie 1 page 19",
            "I get it",
            "ok, I got it now",
            "I know this",
            "I already know this part",
            "Understood.",
            "ich hab es verstanden",
            "So I think I understand this part",
        ] {
            assert_eq!(plain_commitment(t), Some("known"), "{t}");
        }
        assert_eq!(plain_commitment("just tell me the answer"), Some("tell"));
        assert_eq!(
            plain_commitment("can you tell me what it is?"),
            Some("tell")
        );
        assert_eq!(
            plain_commitment("let the agent do the tests"),
            Some("agent")
        );
    }

    #[test]
    fn explanations_doubts_and_questions_are_not() {
        for t in [
            "Maxwells equations describe the EM fields in time and space. Through these equations we can derive the Helmholtz equation which describes the EM waves in space, without any localized current or charge distribution.",
            "I understand the first half but not the boundary conditions",
            "I don't understand this",
            "I understand why it decays?",
            "the router keeps a table",
            "I think the field decays exponentially",
            "I get it: it routes by name",
            "I understand that the field decays",
            "I know that it routes by name",
            "",
        ] {
            assert_eq!(plain_commitment(t), None, "{t}");
        }
    }

    fn workspace() -> (HashMap<String, Pointer>, Map) {
        let part =
            |id: &str, title: &str| Part::new(id.into(), title.into(), String::new(), vec![]);
        let mut a = part("maxwell", "Maxwell's equations");
        a.concepts = vec!["helmholtz".into()];
        let map = Map {
            parts: vec![
                a,
                part("waves", "Plane waves and standing waves"),
                part("fresnel", "Fresnel equations"),
                part("intro", "Course notes"),
            ],
        };
        let at = |file: &str, page: Option<usize>, line: Option<usize>| Pointer {
            file: file.into(),
            page,
            line,
        };
        let pointers = HashMap::from([
            ("maxwell".to_string(), at("FUB_Folien1.pdf", Some(19), None)),
            ("waves".to_string(), at("FUB_Folien1.pdf", Some(30), None)),
            ("fresnel".to_string(), at("FUB_Folien2.pdf", Some(3), None)),
            ("intro".to_string(), at("notes.md", None, Some(2))),
        ]);
        (pointers, map)
    }

    #[test]
    fn a_slide_and_page_name_the_part_there() {
        let (at, map) = workspace();
        let labels = HashMap::new();
        let r = |t: &str| resolve_part(&map, Some("intro"), &at, &labels, t);
        assert_eq!(
            r("I understand the part in Folie 1 page 19"),
            PartRef::One("maxwell".into())
        );
        assert_eq!(r("FUB_Folien1.pdf p. 30"), PartRef::One("waves".into()));
        // A page inside a part's range: the part that starts before it.
        assert_eq!(r("Folien 1, Seite 34"), PartRef::One("waves".into()));
        assert_eq!(r("slide 3 of Folien 2"), PartRef::One("fresnel".into()));
        // A file alone, with two parts in it: ask which.
        assert_eq!(
            r("the stuff in Folien 1"),
            PartRef::Many(vec!["maxwell".into(), "waves".into()])
        );
        assert_eq!(r("notes.md"), PartRef::One("intro".into()));
    }

    #[test]
    fn a_name_or_this_part() {
        let (at, map) = workspace();
        let mut labels = HashMap::new();
        labels.insert("helmholtz".to_string(), "Helmholtz equation".to_string());
        let r = |t: &str| resolve_part(&map, Some("waves"), &at, &labels, t);
        assert_eq!(
            r("I get the Fresnel equations"),
            PartRef::One("fresnel".into())
        );
        assert_eq!(
            r("I know the Helmholtz equation"),
            PartRef::One("maxwell".into())
        );
        assert_eq!(r("I understand this part"), PartRef::Active);
        assert_eq!(r("I get it"), PartRef::Active);
    }
}
