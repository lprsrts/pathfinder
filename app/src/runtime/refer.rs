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
    "already understand",
    "already get",
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
    "just do it",
    "you do it",
    "let the agent",
    "have the agent",
    "get the agent",
    "the agent can",
    "the agent should",
    "can the agent",
    "hand it off",
    "hand this off",
    "hand that off",
    "hand it to the agent",
    "hand this to the agent",
    "give it to the agent",
    "give this to the agent",
    "delegate",
    "mach du",
    "lass den agent",
    "der agent soll",
    "gib es dem agent",
];

/// Openings that ask for something to be written into your files: work
/// only the agent can do (the Tutor and a told answer can't write).
const WRITES: &[&str] = &[
    "write the ",
    "write a ",
    "write an ",
    "write me ",
    "write it",
    "write this",
    "write that",
    "draft the ",
    "draft a ",
    "draft me ",
    "save it",
    "save this",
    "save the ",
    "create the file",
    "create a file",
    "schreib den ",
    "schreib die ",
    "schreib das ",
    "schreib mir ",
    "schreib es",
    "speicher",
];

/// Words before a request to write ("ok, can you please write the
/// letter"). Never "I": "I write the total as…" is your own work.
const ASKING: &[&str] = &[
    "ok",
    "okay",
    "so",
    "yes",
    "yeah",
    "well",
    "now",
    "alright",
    "right",
    "fine",
    "good",
    "also",
    "then",
    "just",
    "ja",
    "jetzt",
    "can",
    "could",
    "would",
    "will",
    "you",
    "please",
    "pls",
    "kannst",
    "könntest",
    "du",
    "bitte",
];

/// Words that say the agent's result needn't be reviewed: a delegation
/// even when the part could be handed off with review.
const NO_REVIEW: &[&str] = &[
    "without me checking",
    "without me reviewing",
    "without me looking",
    "without checking",
    "without reviewing",
    "without review",
    "without a review",
    "without my review",
    "no review",
    "skip the review",
    "don't need to review",
    "don't need to check",
    "don't want to review",
    "don't want to check",
    "won't review",
    "won't check",
    "ohne prüfung",
    "ohne dass ich",
    "ohne review",
    "ohne kontrolle",
];

/// Questions about your own record: answered with `/why`, never with
/// sources. Matched whole, after `RECORD_FILLER` is dropped.
const RECORDS: &[&str] = &[
    "what do i understand",
    "what have i understood",
    "what have i shown",
    "what have i proven",
    "what have i proved",
    "what do i know",
    "how am i doing",
    "how do i stand",
    "where do i stand",
    "my record",
    "my progress",
    "show me my record",
    "show my record",
    "show me my progress",
    "whats my record",
    "what is my record",
    "what does my record say",
    "was habe ich verstanden",
    "was verstehe ich",
    "was kann ich",
    "wie stehe ich",
    "mein stand",
    "mein fortschritt",
];

/// Words a record question may carry around its core ("so what do I
/// actually understand now?").
const RECORD_FILLER: &[&str] = &[
    "so",
    "ok",
    "okay",
    "well",
    "and",
    "then",
    "now",
    "actually",
    "really",
    "exactly",
    "yet",
    "so far",
    "far",
    "by",
    "at",
    "this",
    "point",
    "here",
    "already",
    "currently",
    "overall",
    "also",
    "jetzt",
    "eigentlich",
    "schon",
    "bisher",
    "denn",
    "nun",
];

/// Whether `t` (lowercased) opens with one of `phrases`, after any
/// leading filler words.
fn opens_with(t: &str, phrases: &[&str], filler: &[&str]) -> bool {
    let mut rest = t;
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
        if word.is_empty() || !filler.contains(&word) {
            return false;
        }
        rest = &t[word_end..];
    }
}

/// The sentences and clauses of lowercased `low`.
fn sentences(low: &str) -> impl Iterator<Item = &str> {
    low.split(['.', '!', '?', ';', ',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Whether lowercased `sentence` asks for something to be written into
/// your files ("can you write the letter", "draft me a reply").
fn asks_to_write(sentence: &str) -> bool {
    opens_with(sentence, WRITES, ASKING)
        && !sentence.contains("write out")
        && !sentence.contains("write down")
}

/// Whether lowercased `sentence` asks for the agent ("let the agent do
/// it", "hand the letter to the agent").
fn asks_agent(sentence: &str) -> bool {
    opens_with(sentence, AGENTS, FILLER)
        || (opens_with(sentence, &["hand ", "give ", "pass "], ASKING)
            && sentence.contains("agent"))
}

/// The sentence of `text` that asks for the agent (or for writing), so
/// the part it names is read from it alone ("I get the bill now. Let the
/// agent write the letter": the letter). The whole text when none does.
pub(super) fn agent_clause(text: &str) -> String {
    let low = text.trim().to_lowercase().replace(['’', '‘'], "'");
    let found = sentences(&low)
        .find(|s| asks_agent(s) || asks_to_write(s))
        .map(str::to_string);
    found.unwrap_or(low)
}

/// Words that hand the agent everything still open ("do the rest").
const REST: &[&str] = &[
    "the rest",
    "remaining parts",
    "remaining part",
    "all remaining",
    "the remaining ones",
    "everything else",
    "all the parts",
    "all parts",
    "all of them",
    "every part",
    "each part",
    "all the other parts",
    "den rest",
    "restlichen",
    "alle teile",
    "alles andere",
    "alles übrige",
];

/// Openings by which a piece of a request is your own work, not the
/// agent's ("…, I'll do the summary").
const MINE: &[&str] = &[
    "i ",
    "i'll",
    "i will",
    "i'd",
    "i am",
    "i'm",
    "ich ",
    "mache ich",
];

/// The sentences of `text`, kept as written: a stop ends one only before
/// a capital letter or the end, so "eq. 44" and "fusion_check.py" stay
/// whole. A line break always ends one.
fn written_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' {
            out.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(c);
        if matches!(c, '.' | '!' | '?') {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            let spaced = chars.get(i + 1).is_none_or(|c| c.is_whitespace());
            if spaced && next.is_none_or(|n| n.is_uppercase()) {
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    out.push(cur);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The pieces of a request for the agent, lowercased: from the clause
/// that asks for the agent to the end of its sentence, cut at commas,
/// semicolons and colons ("let the agent do the rest: the quadrature
/// check, the component count and the L2 distance"). A piece that is
/// your own work ("…, I'll do the summary") is left out. When no clause
/// asks for the agent in so many words ("do all the remaining parts in one
/// go: …", read as a request for the agent by the decision model), every
/// piece of the message.
pub(super) fn agent_items(text: &str) -> Vec<String> {
    let sentences = written_sentences(text);
    for sentence in &sentences {
        let low = sentence.to_lowercase().replace(['’', '‘'], "'");
        let pieces: Vec<String> = low
            .split([',', ';', ':'])
            .map(|p| split_exclusion(p.trim()).0)
            .filter(|p| !p.is_empty())
            .collect();
        let Some(first) = pieces
            .iter()
            .position(|p| asks_agent(p) || asks_to_write(p))
        else {
            continue;
        };
        return pieces[first..]
            .iter()
            .map(|p| p.trim_start_matches("and ").trim().to_string())
            .filter(|p| !MINE.iter().any(|m| p.starts_with(m)))
            .collect();
    }
    sentences
        .iter()
        .flat_map(|s| {
            s.to_lowercase()
                .replace(['’', '‘'], "'")
                .split([',', ';', ':'])
                .map(|p| {
                    split_exclusion(p.trim())
                        .0
                        .trim_start_matches("and ")
                        .trim()
                        .to_string()
                })
                .collect::<Vec<_>>()
        })
        .filter(|p| !p.is_empty() && !MINE.iter().any(|m| p.starts_with(m)))
        .collect()
}

/// Words that leave what follows them out of a request for the agent
/// ("…, but not the letter", "the rest except the letter"). "Without" is
/// not one: "without me checking" asks for a delegation.
const EXCLUDE: &[&str] = &[
    "but not",
    "except for",
    "except",
    "apart from",
    "other than",
    "excluding",
    "leaving out",
    "aber nicht",
    "außer",
    "ausgenommen",
    "bis auf",
];

/// A piece of a request cut at its first exclusion word: what it asks
/// for, and what it leaves out. A piece that starts with "not" ("…, not
/// the letter") leaves out all of it.
fn split_exclusion(piece: &str) -> (String, Option<String>) {
    let padded = format!(" {piece} ");
    let found = EXCLUDE
        .iter()
        .filter_map(|m| padded.find(&format!(" {m} ")).map(|at| (at, m.len())))
        .min_by_key(|(at, len)| (*at, std::cmp::Reverse(*len)));
    if let Some((at, len)) = found {
        let kept = padded[..at].trim().to_string();
        let out = padded[at + len + 1..].trim().to_string();
        return (kept, (!out.is_empty()).then_some(out));
    }
    for lead in ["not ", "and not ", "nicht "] {
        if let Some(rest) = piece.strip_prefix(lead) {
            return (String::new(), Some(rest.trim().to_string()));
        }
    }
    (piece.to_string(), None)
}

/// The pieces of a request for the agent that it leaves out ("…, but not
/// the letter"; "the rest except the letter"), lowercased.
pub(super) fn agent_exclusions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for sentence in written_sentences(text) {
        let low = sentence.to_lowercase().replace(['’', '‘'], "'");
        for piece in low.split([',', ';', ':']) {
            if let (_, Some(out_piece)) = split_exclusion(piece.trim()) {
                out.push(out_piece);
            }
        }
    }
    out
}

/// Whether the request for the agent hands it everything still open
/// ("let the agent do the rest", "all remaining parts in one go").
pub(super) fn asks_rest(text: &str) -> bool {
    let items = agent_items(text);
    let low = format!(" {} ", items.join(" "));
    REST.iter().any(|r| {
        // "the rest of the letter" is a piece of one part.
        low.match_indices(&format!(" {r}"))
            .any(|(at, m)| !low[at + m.len()..].starts_with(" of "))
    })
}

/// Phrases that ask to be checked, as the last words of a message.
const CHECK_ME: &[&str] = &[
    "check me",
    "check me on it",
    "check me on this",
    "check me on that",
    "check me on this part",
    "check me now",
    "test me",
    "test me on it",
    "test me on this",
    "quiz me",
    "quiz me on it",
    "quiz me on this",
    "see if i got it",
    "see if i'm right",
    "prüf mich",
    "prüfe mich",
    "prüf mich darauf",
    "frag mich ab",
];

/// Words around a closing "check me" ("so please check me now").
const CHECK_FILLER: &[&str] = &[
    "so", "please", "pls", "now", "then", "and", "ok", "okay", "just", "bitte", "jetzt", "dann",
];

/// Whether `text` ends by asking to be checked ("… I think I understand
/// it now, check me"): Some(true) when words come before it (your
/// explanation, what the check is on), Some(false) for the bare request.
/// The last clause only, so "check me on X, then tell me Y" is not.
pub(super) fn asks_check(text: &str) -> Option<bool> {
    let low = text.trim().to_lowercase().replace(['’', '‘'], "'");
    let clauses: Vec<&str> = sentences(&low).collect();
    let last = clauses.last()?;
    let mut words: Vec<&str> = last
        .split(|c: char| c.is_whitespace())
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\''))
        .filter(|w| !w.is_empty())
        .collect();
    while words.first().is_some_and(|w| CHECK_FILLER.contains(w)) {
        words.remove(0);
    }
    while words.last().is_some_and(|w| CHECK_FILLER.contains(w)) {
        words.pop();
    }
    let core = words.join(" ");
    CHECK_ME
        .contains(&core.as_str())
        .then_some(clauses.len() > 1)
}

/// Whether plain words ask for something to be written into the
/// workspace: work for the agent, never for a told answer (§11).
pub(super) fn wants_writing(text: &str) -> bool {
    let low = text.trim().to_lowercase().replace(['’', '‘'], "'");
    sentences(&low).any(asks_to_write)
}

/// Whether plain words say the agent's result needn't be reviewed ("just
/// do it without me checking").
pub(super) fn wants_no_review(text: &str) -> bool {
    let low = text.to_lowercase().replace(['’', '‘'], "'");
    NO_REVIEW.iter().any(|p| low.contains(p))
}

/// Words that accept the agent's result as it is.
const APPROVALS: &[&str] = &[
    "approve",
    "approved",
    "looks good",
    "look good",
    "looks great",
    "looks fine",
    "looks right",
    "lgtm",
    "ship it",
    "accept it",
    "accept the result",
    "accept that",
    "i accept",
    "that's fine",
    "it's fine",
    "all good",
    "sieht gut aus",
    "passt so",
    "genehmig",
    "nimm es an",
    "akzeptier",
    "angenommen",
];

/// Words that turn an approval into something else: a doubt, a question,
/// a "but".
const NOT_APPROVAL: &[&str] = &[
    "not ", "n't", "dont ", "never", "wrong", "but ", "except", "however", "instead", "nicht",
    "kein", "aber", "falsch",
];

/// Whether plain words accept the agent's result waiting for your review
/// ("looks good, approve it"): a short message with no doubt, question or
/// "but" in it. Code alone; it is shown back before anything runs.
pub(super) fn asks_approve(text: &str) -> bool {
    let low = text.trim().to_lowercase().replace(['’', '‘'], "'");
    if low.contains('?') || low.split_whitespace().count() > 12 {
        return false;
    }
    let padded = format!("{low} ");
    if NOT_APPROVAL.iter().any(|w| padded.contains(w)) {
        return false;
    }
    APPROVALS.iter().any(|p| low.contains(p))
}

/// Whether plain words ask about your own record ("so what do I actually
/// understand now?", "how am I doing?"): answered with `/why`.
pub(super) fn asks_record(text: &str) -> bool {
    let low = text
        .trim()
        .to_lowercase()
        .replace(['’', '‘'], "'")
        .replace("'", "");
    let words: Vec<&str> = low
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !RECORD_FILLER.contains(w))
        .collect();
    if words.is_empty() || words.len() > 8 {
        return false;
    }
    let core = words.join(" ");
    RECORDS.contains(&core.as_str())
}

/// The commitment plain words plausibly express, short of explaining
/// anything: `known`, `tell` or `agent` (the Router's step names). None
/// for anything else, an explanation or a question included. Asking for
/// the agent, or for something to be written into your files, counts in
/// any sentence ("I get the bill now. Let the agent write the letter").
pub(super) fn plain_commitment(text: &str) -> Option<&'static str> {
    let low = text.trim().to_lowercase().replace(['’', '‘'], "'");
    if low.is_empty() {
        return None;
    }
    let starts = |phrases: &[&str]| opens_with(&low, phrases, FILLER);
    if starts(TELLS) && !wants_writing(&low) {
        return Some(super::TELL);
    }
    if sentences(&low).any(|s| asks_agent(s) || asks_to_write(s)) {
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
    /// All of these, in map order: "parts 1 and 2", "the first two parts".
    All(Vec<String>),
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

/// Words that name a part by its place on the map ("part 2").
const PART_WORDS: &[&str] = &["part", "parts", "teil", "teile", "teilen"];

/// Words between numbers in a list of parts.
const AND_WORDS: &[&str] = &["and", "und", "&", ",", "or", "oder"];

/// Words between the ends of a range of parts.
const TO_WORDS: &[&str] = &["-", "to", "through", "thru", "bis"];

/// A cardinal number written out or in digits.
fn cardinal(w: &str) -> Option<usize> {
    const WORDS: &[(&str, usize)] = &[
        ("one", 1),
        ("two", 2),
        ("three", 3),
        ("four", 4),
        ("five", 5),
        ("six", 6),
        ("seven", 7),
        ("eight", 8),
        ("nine", 9),
        ("ten", 10),
        ("eins", 1),
        ("zwei", 2),
        ("drei", 3),
        ("vier", 4),
        ("fünf", 5),
        ("both", 2),
        ("beide", 2),
        ("beiden", 2),
    ];
    w.parse()
        .ok()
        .or_else(|| WORDS.iter().find(|(n, _)| *n == w).map(|(_, v)| *v))
}

/// An ordinal ("first", "2nd", "dritten"); `usize::MAX` for "last".
fn ordinal(w: &str) -> Option<usize> {
    const WORDS: &[(&str, usize)] = &[
        ("first", 1),
        ("second", 2),
        ("third", 3),
        ("fourth", 4),
        ("fifth", 5),
        ("sixth", 6),
        ("erste", 1),
        ("ersten", 1),
        ("zweite", 2),
        ("zweiten", 2),
        ("dritte", 3),
        ("dritten", 3),
        ("vierte", 4),
        ("vierten", 4),
        ("last", usize::MAX),
        ("letzte", usize::MAX),
        ("letzten", usize::MAX),
    ];
    WORDS.iter().find(|(n, _)| *n == w).map(|(_, v)| *v)
}

/// Words, digits and list marks, lowercased: "parts 1-3, 5" is `parts`,
/// `1`, `-`, `3`, `,`, `5`.
fn list_tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut digit = false;
    for c in text.to_lowercase().chars() {
        if c.is_alphanumeric() {
            let d = c.is_ascii_digit();
            if !cur.is_empty() && d != digit {
                out.push(std::mem::take(&mut cur));
            }
            digit = d;
            cur.push(c);
            continue;
        }
        if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        match c {
            '-' | '–' => out.push("-".into()),
            ',' => out.push(",".into()),
            '&' => out.push("&".into()),
            _ => {}
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    // "1st", "2nd": the number alone.
    out.retain(|w| !matches!(w.as_str(), "st" | "nd" | "rd" | "th"));
    out
}

/// The places on the map (1-based, in order) a message names: "part 2",
/// "parts 1 and 2", "parts 1-3", "the first two parts", "the second and
/// third parts", "the last part". Empty when it names none.
fn part_numbers(text: &str, len: usize) -> Vec<usize> {
    let t = list_tokens(text);
    let mut out: Vec<usize> = Vec::new();
    let place = |n: usize| if n == usize::MAX { len } else { n };
    for (i, w) in t.iter().enumerate() {
        if !PART_WORDS.contains(&w.as_str()) {
            continue;
        }
        // After the word: "part 2", "parts 1 and 2", "parts 1-3".
        let mut j = i + 1;
        let mut after: Vec<usize> = Vec::new();
        while let Some(n) = t.get(j).and_then(|w| cardinal(w)) {
            if let (Some(sep), Some(end)) = (t.get(j + 1), t.get(j + 2).and_then(|w| cardinal(w)))
                && TO_WORDS.contains(&sep.as_str())
                && end >= n
            {
                after.extend(n..=end);
                j += 3;
            } else {
                after.push(n);
                j += 1;
            }
            if t.get(j).is_some_and(|w| AND_WORDS.contains(&w.as_str())) {
                j += 1;
            }
        }
        // Before the word: "the first two parts", "the second and third
        // part", "the last part".
        let mut k = i;
        let mut before: Vec<&str> = Vec::new();
        while k > 0 {
            let w = t[k - 1].as_str();
            if ordinal(w).is_some() || cardinal(w).is_some() || AND_WORDS.contains(&w) {
                before.push(w);
                k -= 1;
            } else {
                break;
            }
        }
        before.reverse();
        match before.as_slice() {
            [first, count] if ordinal(first).is_some() && cardinal(count).is_some() => {
                let n = cardinal(count).unwrap();
                if ordinal(first) == Some(usize::MAX) {
                    out.extend(len.saturating_sub(n) + 1..=len);
                } else {
                    let from = ordinal(first).unwrap();
                    out.extend(from..from + n);
                }
            }
            words => out.extend(words.iter().filter_map(|w| ordinal(w)).map(place)),
        }
        out.extend(after);
    }
    out.retain(|n| (1..=len).contains(n));
    let mut seen = Vec::new();
    out.retain(|n| {
        let new = !seen.contains(n);
        seen.push(*n);
        new
    });
    out
}

/// Words that ask for a step, never name a part ("let the agent do the
/// work", "I understand this part").
const STEP_WORDS: &[&str] = &[
    "understand",
    "understood",
    "know",
    "known",
    "already",
    "agent",
    "hand",
    "handoff",
    "delegate",
    "part",
    "parts",
    "this",
    "that",
    "just",
    "please",
    "make",
    "think",
    "sure",
    "good",
    "okay",
    "yeah",
    "then",
    "now",
    "check",
    "checking",
    "review",
    "reviewing",
    "without",
    "them",
    "these",
    "those",
    "whole",
    "work",
    "task",
    "thing",
    "stuff",
    "rest",
    "everything",
    "it's",
    "clear",
    "sense",
    "makes",
    "have",
    "here",
    "there",
    "verstehe",
    "verstanden",
    "kenne",
    "agenten",
    "mach",
    "lass",
    "bitte",
    "jetzt",
    "schon",
    "teil",
];

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
    // 0. A place on the map: "part 2", "the first two parts".
    let places = part_numbers(text, open.len());
    if !places.is_empty() {
        let ids: Vec<String> = places.iter().map(|n| open[n - 1].id.clone()).collect();
        return match ids.len() {
            1 => PartRef::One(ids[0].clone()),
            _ => PartRef::All(ids),
        };
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
    // 3. One distinctive word of a title ("the letter" for "Write the
    // complaint letter"), when it fits that part alone.
    let named: Vec<String> = words
        .iter()
        .filter(|w| w.chars().count() >= 4 && !STEP_WORDS.contains(&w.as_str()))
        .flat_map(|w| {
            open.iter()
                .filter(|p| {
                    sources::keywords(&p.title)
                        .iter()
                        .any(|t| !STEP_WORDS.contains(&t.as_str()) && alike(w, t))
                })
                .map(|p| p.id.clone())
                .collect::<Vec<_>>()
        })
        .collect();
    let mut distinct = named.clone();
    distinct.sort();
    distinct.dedup();
    if let [one] = distinct.as_slice() {
        return PartRef::One(one.clone());
    }
    // 4. "This part", "it", nothing: the one you're on.
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
    fn a_request_for_the_agent_reads_its_pieces_and_the_rest() {
        assert_eq!(
            agent_items(
                "I get it. Let the agent do the rest in fusion_check.py: the quadrature check, the count over eq. 44 and the L2 distance, and run it"
            ),
            vec![
                "let the agent do the rest in fusion_check.py",
                "the quadrature check",
                "the count over eq. 44 and the l2 distance",
                "run it",
            ]
        );
        assert_eq!(
            agent_items("Let the agent write the letter, I'll check the charges"),
            vec!["let the agent write the letter"]
        );
        assert_eq!(
            agent_items("Do all the remaining parts in one go: the check, the letter"),
            vec![
                "do all the remaining parts in one go",
                "the check",
                "the letter"
            ]
        );
        assert_eq!(
            agent_items("let the agent check the charges, but not the letter"),
            vec!["let the agent check the charges"]
        );
        assert_eq!(
            agent_exclusions("let the agent check the charges, but not the letter"),
            vec!["the letter"]
        );
        assert_eq!(
            agent_items("let the agent do the rest except the letter"),
            vec!["let the agent do the rest"]
        );
        assert_eq!(
            agent_exclusions("Let the agent do the rest except the letter"),
            vec!["the letter"]
        );
        assert_eq!(
            agent_exclusions("let the agent do it without me checking"),
            Vec::<String>::new()
        );
        assert!(asks_rest("let the agent do the rest except the letter"));
        assert!(asks_rest("let the agent do the rest"));
        assert!(asks_rest("do all the remaining parts in one go"));
        assert!(asks_rest("let the agent do all remaining parts"));
        assert!(!asks_rest("let the agent write the rest of the letter"));
        assert!(!asks_rest("let the agent fix the remaining bug"));
    }

    #[test]
    fn check_me_closes_an_explanation() {
        assert_eq!(asks_check("check me"), Some(false));
        assert_eq!(asks_check("Check me on this part."), Some(false));
        assert_eq!(
            asks_check(
                "Expand the square: each integral is a double sum. I think I understand it now, check me"
            ),
            Some(true)
        );
        assert_eq!(asks_check("So please test me now!"), Some(false));
        assert_eq!(
            asks_check("check me on routing, then tell me about VAT"),
            None
        );
        assert_eq!(asks_check("can you check my reasoning?"), None);
        assert_eq!(asks_check("I checked me twice"), None);
    }

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

    #[test]
    fn the_agent_or_writing_in_any_sentence() {
        for t in [
            "I get the bill now. let the agent write the letter",
            "I already understand parts 1 and 2. hand the letter to the agent",
            "hand this to the agent",
            "can you write the letter?",
            "ok, could you please draft a reply to the supplier",
            "just do it without me checking, let the agent do it",
        ] {
            assert_eq!(plain_commitment(t), Some("agent"), "{t}");
        }
        assert_eq!(plain_commitment("just tell me the answer"), Some("tell"));
        // Your own work, not a request.
        assert_eq!(plain_commitment("I write the total as 42 kWh"), None);
        assert!(!wants_writing("can you write out the formula"));
        assert!(wants_writing("Can you write the letter for me?"));
        assert_eq!(
            agent_clause("I get the bill now. Let the agent write the letter"),
            "let the agent write the letter"
        );
        assert_eq!(agent_clause("do it for me"), "do it for me");
        assert!(wants_no_review("just do it without me checking"));
        assert!(!wants_no_review("let the agent do it"));
    }

    #[test]
    fn record_questions() {
        for t in [
            "so what do I actually understand now?",
            "What have I shown so far?",
            "how am I doing?",
            "my record",
            "Was habe ich bisher verstanden?",
        ] {
            assert!(asks_record(t), "{t}");
        }
        for t in [
            "what do I know about tariffs?",
            "what does the standing charge mean?",
            "I understand the bill",
            "",
        ] {
            assert!(!asks_record(t), "{t}");
        }
    }

    #[test]
    fn parts_by_their_place_on_the_map() {
        let part =
            |id: &str, title: &str| Part::new(id.into(), title.into(), String::new(), vec![]);
        let map = Map {
            parts: vec![
                part("bill", "Read the bill"),
                part("check", "Check the charges"),
                part("letter", "Write the complaint letter"),
            ],
        };
        let at = HashMap::new();
        let labels = HashMap::new();
        let r = |t: &str| resolve_part(&map, Some("bill"), &at, &labels, t);
        let all = |ids: &[&str]| PartRef::All(ids.iter().map(|s| s.to_string()).collect());
        assert_eq!(
            r("I already know the first two parts"),
            all(&["bill", "check"])
        );
        assert_eq!(
            r("I already understand parts 1 and 2"),
            all(&["bill", "check"])
        );
        assert_eq!(r("parts 1-3"), all(&["bill", "check", "letter"]));
        assert_eq!(r("the second and third parts"), all(&["check", "letter"]));
        assert_eq!(r("part 2"), PartRef::One("check".into()));
        assert_eq!(r("the last part"), PartRef::One("letter".into()));
        assert_eq!(r("Teil 3"), PartRef::One("letter".into()));
        // Out of range: no place named.
        assert_eq!(r("part 7"), PartRef::Active);
        // One distinctive word of a title.
        assert_eq!(
            r("let the agent write the letter"),
            PartRef::One("letter".into())
        );
        assert_eq!(r("I get the bill"), PartRef::One("bill".into()));
        assert_eq!(r("I understand this part"), PartRef::Active);
        assert_eq!(r("let the agent do the work"), PartRef::Active);
    }

    #[test]
    fn an_approval_is_read_only_without_doubt() {
        for yes in [
            "looks good, approve it",
            "LGTM",
            "ok, ship it",
            "I approve",
            "sieht gut aus",
            "That’s fine, accept it.",
        ] {
            assert!(asks_approve(yes), "{yes}");
        }
        for no in [
            "does it look good?",
            "looks good but the date is wrong",
            "I don't approve this",
            "not approved, the total is off",
            "sieht nicht gut aus",
            "the letter says the meter reading was estimated and I want to check the approved tariff against the bill before anything else",
            "the kWh line is the usage",
        ] {
            assert!(!asks_approve(no), "{no}");
        }
    }
}
