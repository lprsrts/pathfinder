//! Sources only: Pathfinder answers with pointers into sources and short
//! verbatim hits, never with its own reasoning.
//!
//! Everything here is plain code. The model may *propose* hits, but what the
//! user sees is rendered from this module only, after every quote has been
//! found in the cited source.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

/// Longest quote shown by default, in characters (`quote_cap` in the config).
pub const QUOTE_CAP: usize = 220;

/// The configured quote cap, read once.
pub fn quote_cap() -> usize {
    static CAP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CAP.get_or_init(|| crate::config::Config::load().quote_cap.clamp(60, 1000))
}
/// Most hits in one reply.
pub const MAX_HITS: usize = 5;
/// Most suggested search terms, and their longest length.
const MAX_TERMS: usize = 5;
const TERM_CAP: usize = 40;
/// Files larger than this aren't searched.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// A quote may span at most this many source lines.
const MAX_SPAN: usize = 6;

/// Where a hit is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Locator {
    /// A workspace file: relative path, PDF page (1-based) if any, line
    /// (1-based, within the page for PDFs).
    Local {
        file: String,
        page: Option<usize>,
        line: usize,
    },
    /// A web page: URL and the section the quote sits under, if known.
    Web {
        url: String,
        section: Option<String>,
    },
}

impl std::fmt::Display for Locator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Locator::Local {
                file,
                page: Some(p),
                line,
            } => write!(f, "{file} p.{p} l.{line}"),
            Locator::Local {
                file,
                page: None,
                line,
            } => write!(f, "{file}:{line}"),
            Locator::Web {
                url,
                section: Some(s),
            } => write!(f, "{url} § {s}"),
            Locator::Web { url, section: None } => write!(f, "{url}"),
        }
    }
}

/// A verified hit: a pointer and a verbatim snippet from that place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub at: Locator,
    pub quote: String,
}

// ---------------------------------------------------------------- matching

/// The form quotes are compared in: typographic quotes, dashes and
/// ligatures unified, hyphenation at line ends joined, whitespace collapsed.
/// Case is kept: a quote must be the source's own words.
pub fn normalise(text: &str) -> String {
    let text = text
        .replace("-\n", "")
        .replace('\u{ad}', "")
        .replace(['‘', '’', '‚', '′'], "'")
        .replace(['“', '”', '„', '″'], "\"")
        .replace(['–', '—', '‐', '‑', '−'], "-")
        .replace('ﬁ', "fi")
        .replace('ﬂ', "fl")
        .replace('ﬀ', "ff")
        .replace('…', "...");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Latin letters with marks, as the plain letters people type for them.
fn plain(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'ç' | 'ć' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => "i",
        'ł' | 'ľ' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'ř' => "r",
        'ś' | 'š' | 'ş' => "s",
        'ß' => "ss",
        'ť' | 'ţ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        'æ' => "ae",
        'œ' => "oe",
        _ => return None,
    })
}

/// The form spelling variants share: lowercase letters and digits only,
/// marks dropped, and "ae", "oe", "ue" read as "a", "o", "u". "K-means",
/// "k means" and "kmeans" are all "kmeans"; "Schrödinger" and
/// "Schroedinger" are both "schrodinger". With each output char, the first
/// and last index of the input chars it came from.
pub fn loose(text: &str) -> (String, Vec<(usize, usize)>) {
    let mut chars: Vec<(char, usize)> = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let c = c.to_lowercase().next().unwrap_or(c);
        if let Some(p) = plain(c) {
            chars.extend(p.chars().map(|p| (p, i)));
        } else if c.is_alphanumeric() {
            chars.push((c, i));
        }
    }
    let mut out = String::new();
    let mut from: Vec<(usize, usize)> = Vec::new();
    let mut k = 0;
    while k < chars.len() {
        let (c, i) = chars[k];
        // The German way of writing ä, ö, ü without the marks.
        if matches!(c, 'a' | 'o' | 'u') && chars.get(k + 1).is_some_and(|n| n.0 == 'e') {
            out.push(c);
            from.push((i, chars[k + 1].1));
            k += 2;
            continue;
        }
        out.push(c);
        from.push((i, i));
        k += 1;
    }
    (out, from)
}

/// A search term in the form lines are searched for: its [`loose`] form
/// with the last word cut to a stem, so "clustering", "clusters" and
/// "cluster" find each other, as do "Gleichung" and "Gleichungen". Only the
/// last word is cut, so the words of a phrase still have to stand together.
pub fn term_form(term: &str) -> String {
    let full = loose(term).0;
    let Some(last) = term
        .split(|c: char| !c.is_alphanumeric())
        .rfind(|w| !w.is_empty())
    else {
        return full;
    };
    let last = loose(last).0;
    match full.strip_suffix(last.as_str()) {
        Some(head) => format!("{head}{}", stem(stem(&last))),
        None => full,
    }
}

/// `word` without one common English or German ending, if at least four
/// letters are left. Applied twice, "clustering" and "clusters" both end
/// at "clust".
fn stem(word: &str) -> &str {
    const ENDINGS: &[&str] = &[
        "ations", "ation", "ungen", "ings", "ing", "ung", "ies", "ers", "es", "ed", "er", "en",
        "ly", "s", "e",
    ];
    for end in ENDINGS {
        if let Some(head) = word.strip_suffix(end)
            && head.chars().count() >= 4
        {
            return head;
        }
    }
    word
}

/// Whether `text` contains `term` up to spelling variants (see [`loose`]).
pub fn mentions(text: &str, term: &str) -> bool {
    let t = loose(term).0;
    !t.is_empty() && loose(text).0.contains(&t)
}

/// Where `quote` occurs in `lines`, spanning at most [`MAX_SPAN`] lines,
/// compared in [`loose`] form: case, punctuation, spacing, hyphens and marks
/// don't matter, the letters and digits and their order do. Returns the
/// 1-based start line and the source's own text for the match.
pub fn locate(lines: &[&str], quote: &str) -> Option<(usize, String)> {
    let want = loose(quote).0;
    let n = want.chars().count();
    // Short enough for a step of maths ("(x-2)(x+2)=0" is "x2x20").
    if n < 4 {
        return None;
    }
    // The first line the quote ends on, then the latest line it can start
    // on: that is where it is.
    for end in 0..lines.len() {
        for start in (end.saturating_sub(MAX_SPAN - 1)..=end).rev() {
            let have = normalise(&lines[start..=end].join("\n"));
            let (folded, from) = loose(&have);
            if let Some(at) = folded.find(&want) {
                let first = folded[..at].chars().count();
                let chars: Vec<char> = have.chars().collect();
                let (mut a, mut b) = (from[first].0, from[first + n - 1].1);
                // Take the punctuation around the words with them.
                while a > 0 && "([{\"'“‘«".contains(chars[a - 1]) {
                    a -= 1;
                }
                while b + 1 < chars.len() && ".,;:!?)]}\"'”’»%".contains(chars[b + 1]) {
                    b += 1;
                }
                let own: String = chars[a..=b].iter().collect();
                return Some((start + 1, own));
            }
        }
    }
    None
}

/// Shortens a verified quote to the cap at a word boundary. A prefix of a
/// verbatim quote is still verbatim.
pub fn cap(quote: &str) -> String {
    cap_to(quote, quote_cap())
}

/// [`cap`] with an explicit limit.
pub fn cap_to(quote: &str, limit: usize) -> String {
    let q = normalise(quote);
    if q.chars().count() <= limit {
        return q;
    }
    let cut: String = q.chars().take(limit).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(a, _)| a);
    format!("{cut} …")
}

// ----------------------------------------------------------------- loading

/// A workspace path the user or model named, confined to the workspace.
pub fn confine(cwd: &Path, file: &str) -> Option<PathBuf> {
    let rel = Path::new(file.trim().trim_start_matches("./"));
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return None;
    }
    let full = cwd.join(rel);
    full.is_file().then_some(full)
}

fn is_pdf(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// A source's text as pages of text (one page for non-PDFs).
pub fn pages(full: &Path, rel: &str) -> Result<Vec<String>, String> {
    if is_pdf(full) {
        return crate::agent::tools::pdf_pages(full, rel).map(|p| (*p).clone());
    }
    let meta = std::fs::metadata(full).map_err(|e| format!("can't read `{rel}`: {e}"))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("`{rel}` is too large to read here"));
    }
    let bytes = std::fs::read(full).map_err(|e| format!("can't read `{rel}`: {e}"))?;
    if bytes.iter().take(4096).any(|b| *b == 0) {
        return Err(format!("`{rel}` isn't text"));
    }
    Ok(vec![String::from_utf8_lossy(&bytes).into_owned()])
}

// ------------------------------------------------------------ verification

/// A hit as the model proposes it. Only `file`, `page` and `quote` are
/// used; line numbers are computed here.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProposedHit {
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub page: Option<usize>,
    #[serde(default)]
    pub quote: String,
}

/// The only reply the model may give to a question (the reply contract).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CitedReply {
    #[serde(default)]
    pub hits: Vec<ProposedHit>,
    #[serde(default)]
    pub search_terms: Vec<String>,
}

/// Parses the model's reply. Anything but the contract's JSON (prose before
/// or after it, extra fields such as an "answer") is rejected.
pub fn parse_reply(raw: &str) -> Result<CitedReply, String> {
    let t = raw.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .map(|s| s.trim().trim_end_matches("```").trim())
        .unwrap_or(t);
    if !(t.starts_with('{') && t.ends_with('}')) {
        return Err("reply is not a bare JSON object".into());
    }
    serde_json::from_str(t).map_err(|e| format!("reply breaks the contract: {e}"))
}

/// Why a proposed hit was dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    NoSuchFile(String),
    Unreadable(String),
    NotInSource { file: String, page: Option<usize> },
    TooShort,
}

/// Checks a proposed local hit against the file it cites and returns it with
/// the real page and line, or why it was dropped.
pub fn verify_local(cwd: &Path, p: &ProposedHit) -> Result<Hit, Rejected> {
    if normalise(&p.quote).chars().count() < 8 {
        return Err(Rejected::TooShort);
    }
    let file = p.file.trim().trim_start_matches("./").to_string();
    let full = confine(cwd, &file).ok_or_else(|| Rejected::NoSuchFile(file.clone()))?;
    let pages = pages(&full, &file).map_err(Rejected::Unreadable)?;
    let pdf = is_pdf(&full);
    // The cited page first, then the rest: a wrong page number is corrected,
    // a quote that isn't in the file at all is dropped.
    let order: Vec<usize> = p
        .page
        .filter(|n| pdf && *n >= 1 && *n <= pages.len())
        .map(|n| n - 1)
        .into_iter()
        .chain(0..pages.len())
        .collect();
    for i in order {
        let lines: Vec<&str> = pages[i].lines().collect();
        if let Some((line, own)) = locate(&lines, &p.quote) {
            return Ok(Hit {
                at: Locator::Local {
                    file,
                    page: pdf.then_some(i + 1),
                    line,
                },
                quote: cap(&own),
            });
        }
    }
    Err(Rejected::NotInSource { file, page: p.page })
}

/// Checks a web hit against the page text that was actually fetched.
pub fn verify_web(url: &str, section: Option<&str>, page_text: &str, quote: &str) -> Option<Hit> {
    let lines: Vec<&str> = page_text.lines().collect();
    let (_, own) = locate(&lines, quote)?;
    Some(Hit {
        at: Locator::Web {
            url: url.to_string(),
            section: section.map(str::to_string),
        },
        quote: cap(&own),
    })
}

/// Search terms the model may suggest: short keyword phrases, never
/// sentences. Anything else is dropped.
pub fn clean_terms(terms: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in terms {
        let t = t.trim();
        if t.is_empty()
            || t.chars().count() > TERM_CAP
            || t.split_whitespace().count() > 4
            || t.contains(['.', '!', '?', ':', ';', '='])
            || out.iter().any(|o| o.eq_ignore_ascii_case(t))
        {
            continue;
        }
        out.push(t.to_string());
        if out.len() == MAX_TERMS {
            break;
        }
    }
    out
}

/// Whether a chat message asks something (and so gets sources), rather
/// than logging the user's own thinking (which gets none).
pub fn is_question(text: &str) -> bool {
    const ASK: &[&str] = &[
        "what", "why", "how", "where", "when", "which", "who", "is", "are", "does", "do", "can",
        "could", "should", "was", "wie", "warum", "wieso", "weshalb", "wo", "wann", "welche",
        "welcher", "welches", "wer", "ist", "sind", "kann", "gibt",
    ];
    let t = text.trim();
    t.contains('?')
        || t.split_whitespace()
            .next()
            .is_some_and(|w| ASK.contains(&w.to_lowercase().as_str()))
}

/// Keywords from a question, for a search done by code alone.
pub fn keywords(question: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "what", "which", "where", "when", "does", "this", "that", "with", "from", "have", "about",
        "there", "their", "they", "into", "than", "then", "how", "why", "the", "and", "for", "are",
        "was", "is", "of", "in", "on", "to", "a", "an", "do", "it", "be", "can", "say", "says",
        "paper", "wie", "was", "der", "die", "das", "und", "ist", "ein", "eine", "warum", "mit",
    ];
    let mut out: Vec<String> = Vec::new();
    for w in question.split(|c: char| !c.is_alphanumeric() && c != '-') {
        let lw = w.to_lowercase();
        if lw.chars().count() < 3 || STOP.contains(&lw.as_str()) || out.contains(&lw) {
            continue;
        }
        out.push(lw);
    }
    out
}

// ----------------------------------------------------------------- finding

/// Source files in the workspace (gitignore respected, hidden files and
/// Pathfinder's own state skipped).
pub fn source_files(cwd: &Path) -> Vec<(PathBuf, String)> {
    const TEXT: &[&str] = &[
        "md", "txt", "tex", "bib", "rst", "org", "csv", "tsv", "json", "yaml", "yml", "toml",
        "html", "htm", "rs", "py", "js", "ts", "c", "h", "cpp", "java", "go", "m", "r", "jl", "sh",
        "ipynb", "pdf",
    ];
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(cwd).build().flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let ok = p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| TEXT.contains(&e.to_ascii_lowercase().as_str()));
        if !ok {
            continue;
        }
        let Ok(rel) = p.strip_prefix(cwd) else {
            continue;
        };
        out.push((p.to_path_buf(), rel.to_string_lossy().replace('\\', "/")));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// `/find`: lines in the workspace's sources that contain the terms, most
/// terms first. Raw lines, nothing else.
pub fn find(cwd: &Path, terms: &[String], limit: usize) -> Vec<Hit> {
    let terms: Vec<String> = terms
        .iter()
        .map(|t| term_form(t))
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, usize, Hit)> = Vec::new();
    let mut seq = 0;
    for (full, rel) in source_files(cwd) {
        let Ok(pages) = pages(&full, &rel) else {
            continue;
        };
        let pdf = is_pdf(&full);
        for (pi, page) in pages.iter().enumerate() {
            for (li, line) in page.lines().enumerate() {
                let low = loose(line).0;
                let n = terms.iter().filter(|t| low.contains(t.as_str())).count();
                if n == 0 {
                    continue;
                }
                seq += 1;
                scored.push((
                    n,
                    seq,
                    Hit {
                        at: Locator::Local {
                            file: rel.clone(),
                            page: pdf.then_some(pi + 1),
                            line: li + 1,
                        },
                        quote: cap(line),
                    },
                ));
            }
        }
    }
    // Need every term when there are enough such lines; else the best.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, _, h)| h).collect()
}

/// `/open`: a numbered view of a file, a PDF page, or lines around a line.
pub fn open(cwd: &Path, file: &str, at: Option<usize>) -> Result<String, String> {
    let rel = file.trim().trim_start_matches("./");
    let full = confine(cwd, rel).ok_or_else(|| format!("No file `{rel}` in this workspace."))?;
    let pages = pages(&full, rel)?;
    const WINDOW: usize = 60;
    if is_pdf(&full) {
        let n = at.unwrap_or(1);
        let page = pages
            .get(n.wrapping_sub(1))
            .ok_or_else(|| format!("`{rel}` has {} pages.", pages.len()))?;
        let mut out = format!("`{rel}` page {n} of {}\n\n```text\n", pages.len());
        for (i, l) in page.lines().enumerate() {
            out.push_str(&format!("{:>4}  {}\n", i + 1, l.trim_end()));
        }
        out.push_str("```");
        return Ok(out);
    }
    let lines: Vec<&str> = pages[0].lines().collect();
    let start = at.map_or(0, |l| l.saturating_sub(WINDOW / 4).saturating_sub(1));
    let end = (start + WINDOW).min(lines.len());
    let mut out = format!(
        "`{rel}` lines {}–{end} of {}\n\n```text\n",
        start + 1,
        lines.len()
    );
    for (i, l) in lines.iter().enumerate().take(end).skip(start) {
        out.push_str(&format!("{:>5}  {}\n", i + 1, l.trim_end()));
    }
    out.push_str("```");
    Ok(out)
}

/// A part's note reduced to a pointer: the first workspace file it names,
/// with a page (`p. 12`, `page 12`, `S. 12`) or line (`file:40`) if it gives
/// one that exists. Anything else in the note is dropped.
pub fn pointer_note(cwd: &Path, note: &str) -> Option<String> {
    let page_re = regex::Regex::new(r"(?i)\b(?:pp?\.|pages?|seite|s\.)\s*(\d+)").ok()?;
    for raw in note.split_whitespace() {
        let token = raw.trim_matches(|c: char| "`'\"()[],;".contains(c));
        let (file, line) = match token.rsplit_once(':') {
            Some((f, l)) if l.parse::<usize>().is_ok() => (f, l.parse::<usize>().ok()),
            _ => (token.trim_end_matches('.'), None),
        };
        let Some(full) = confine(cwd, file) else {
            continue;
        };
        let count = pages(&full, file).ok()?;
        if is_pdf(&full) {
            let page = page_re
                .captures(note)
                .and_then(|c| c[1].parse::<usize>().ok())
                .filter(|p| *p >= 1 && *p <= count.len());
            return Some(match page {
                Some(p) => format!("`{file} p.{p}`"),
                None => format!("`{file}`"),
            });
        }
        let lines = count[0].lines().count();
        return Some(match line.filter(|l| *l >= 1 && *l <= lines) {
            Some(l) => format!("`{file}:{l}`"),
            None => format!("`{file}`"),
        });
    }
    None
}

// --------------------------------------------------------------------- web

/// A raw web result: the URL, the section heading it sits under, and the
/// fetched text the quote is checked against.
#[derive(Debug, Clone)]
pub struct WebResult {
    pub url: String,
    pub section: Option<String>,
    pub page_text: String,
    pub quote: String,
}

/// Web results that pass verification against their fetched text.
pub fn web_hits(results: &[WebResult]) -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::new();
    for h in results
        .iter()
        .filter_map(|r| verify_web(&r.url, r.section.as_deref(), &r.page_text, &r.quote))
    {
        // The same abstract from two catalogues is one hit.
        if !hits.iter().any(|o| loose(&o.quote).0 == loose(&h.quote).0) {
            hits.push(h);
        }
    }
    hits.truncate(MAX_HITS);
    hits
}

/// Marks a heading line in page text, so a hit can name its section.
pub const HEADING: char = '\u{1}';

/// The page line holding the most search terms (at least half of them, and
/// at least one), and the heading it sits under: (section, line).
#[cfg(test)]
pub fn pick_quote(page_text: &str, terms: &[String]) -> Option<(Option<String>, String)> {
    quote_candidates(page_text, terms, 1).into_iter().next()
}

/// Up to `k` page lines holding at least half of the search terms, best
/// first, each with the heading it sits under.
pub fn quote_candidates(
    page_text: &str,
    terms: &[String],
    k: usize,
) -> Vec<(Option<String>, String)> {
    let terms: Vec<String> = terms
        .iter()
        .map(|t| term_form(t))
        .filter(|t| t.len() > 1)
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let need = terms.len().div_ceil(2).max(1);
    let mut section = None;
    let mut found: Vec<(usize, usize, Option<String>, String)> = Vec::new();
    for (i, line) in page_text.lines().enumerate() {
        if let Some(h) = line.strip_prefix(HEADING) {
            section = Some(h.trim().to_string()).filter(|h| !h.is_empty());
            continue;
        }
        if line.split_whitespace().count() < 5 {
            continue;
        }
        let low = loose(line).0;
        let n = terms.iter().filter(|t| low.contains(t.as_str())).count();
        // A title only wins over body text with more terms.
        let score = n * 2 + usize::from(section.as_deref() != Some("Title"));
        if n >= need {
            found.push((score, i, section.clone(), line.trim().to_string()));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    found
        .into_iter()
        .take(k)
        .map(|(_, _, s, l)| (s, l))
        .collect()
}

/// Search queries the model proposes for the web: `{"queries": [...]}`,
/// at most 3, each a short query (no sentences of reasoning, no question
/// marks). Anything else is dropped.
pub fn parse_queries(raw: &str) -> Vec<String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Q {
        queries: Vec<String>,
    }
    let Ok(q) = serde_json::from_str::<Q>(strip_fence(raw)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for q in q.queries {
        let q = q.trim();
        if q.is_empty()
            || q.chars().count() > 100
            || q.split_whitespace().count() > 10
            || q.contains(['?', '!', ';'])
            || out.iter().any(|o| o.eq_ignore_ascii_case(q))
        {
            continue;
        }
        out.push(q.to_string());
        if out.len() == 3 {
            break;
        }
    }
    out
}

/// The candidate numbers the model picks as relevant: `{"pick": [..]}`, in
/// range, no repeats, at most [`MAX_HITS`]. `None` if the reply is broken.
pub fn parse_pick(raw: &str, count: usize) -> Option<Vec<usize>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        pick: Vec<usize>,
    }
    let p: P = serde_json::from_str(strip_fence(raw)).ok()?;
    let mut out: Vec<usize> = Vec::new();
    for i in p.pick {
        if i < count && !out.contains(&i) {
            out.push(i);
        }
    }
    out.truncate(MAX_HITS);
    Some(out)
}

fn strip_fence(raw: &str) -> &str {
    let t = raw.trim();
    t.strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .map(|s| s.trim().trim_end_matches("```").trim())
        .unwrap_or(t)
}

// --------------------------------------------------------------- rendering

/// The only shape a sourced reply takes: pointers and raw hits, or "not
/// found" with places to look. `skipped` lists services that couldn't be
/// searched.
pub fn render(
    lang: Option<&str>,
    hits: &[Hit],
    terms: &[String],
    online: bool,
    skipped: &[String],
) -> String {
    use crate::i18n::{Line, line};
    let mut out = if hits.is_empty() {
        let mut out = String::from(line(
            lang,
            if online {
                Line::NotFoundOnline
            } else {
                Line::NotFoundHere
            },
        ));
        if !terms.is_empty() {
            let finds = terms
                .iter()
                .map(|t| format!("`/find {t}`"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str("\n\n");
            out.push_str(&line(lang, Line::TryTerms).replace("{terms}", &finds));
        }
        out
    } else {
        let mut out = format!("{}\n", line(lang, Line::WhereToLook));
        for h in hits {
            out.push_str(&format!("\n- `{}`\n  > {}", h.at, h.quote));
        }
        out
    };
    if !skipped.is_empty() {
        out.push_str("\n\n");
        out.push_str(&line(lang, Line::NotSearched).replace("{services}", &skipped.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(files: &[(&str, &str)]) -> tempdir::Dir {
        let d = tempdir::Dir::new();
        for (name, body) in files {
            let p = d.path().join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        d
    }

    mod tempdir {
        pub struct Dir(std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let p = std::env::temp_dir().join(format!("pf-src-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    const NOTES: &str = "# Notes\n\nThe Laplace transform turns\ndifferential equations into algebraic ones.\nIt is linear.\n";

    fn hit(file: &str, page: Option<usize>, quote: &str) -> ProposedHit {
        ProposedHit {
            file: file.into(),
            page,
            quote: quote.into(),
        }
    }

    #[test]
    fn exact_quote_is_found_with_its_line() {
        let d = ws(&[("notes.md", NOTES)]);
        let h = verify_local(d.path(), &hit("notes.md", None, "It is linear.")).unwrap();
        assert_eq!(
            h.at,
            Locator::Local {
                file: "notes.md".into(),
                page: None,
                line: 5
            }
        );
        assert_eq!(h.quote, "It is linear.");
    }

    #[test]
    fn quote_across_a_line_break_and_typography_is_normalised() {
        let d = ws(&[("notes.md", NOTES)]);
        let h = verify_local(
            d.path(),
            &hit("notes.md", None, "turns  differential equations into"),
        )
        .unwrap();
        assert!(matches!(h.at, Locator::Local { line: 3, .. }));
        let lines = ["it’s a “phasor” — see", "page 3"];
        assert_eq!(
            locate(&lines, "It's a \"Phasor\" - see page 3"),
            Some((1, "it's a \"phasor\" - see page 3".to_string()))
        );
        assert_eq!(normalise("trans-\nform"), "transform");
    }

    #[test]
    fn paraphrase_and_invented_quotes_are_rejected() {
        let d = ws(&[("notes.md", NOTES)]);
        // A paraphrase is not a quote.
        assert!(matches!(
            verify_local(d.path(), &hit("notes.md", None, "The transform is linear.")),
            Err(Rejected::NotInSource { .. })
        ));
        // Case is ignored, but what is shown is the source's own words.
        let h = verify_local(d.path(), &hit("notes.md", None, "it is LINEAR.")).unwrap();
        assert_eq!(h.quote, "It is linear.");
        assert!(matches!(
            verify_local(d.path(), &hit("missing.md", None, "It is linear.")),
            Err(Rejected::NoSuchFile(_))
        ));
        assert_eq!(
            verify_local(d.path(), &hit("notes.md", None, "It")),
            Err(Rejected::TooShort)
        );
    }

    #[test]
    fn paths_outside_the_workspace_are_refused() {
        let d = ws(&[("notes.md", NOTES)]);
        assert!(confine(d.path(), "../notes.md").is_none());
        assert!(confine(d.path(), "/etc/hosts").is_none());
        assert!(confine(d.path(), "./notes.md").is_some());
    }

    #[test]
    fn long_quotes_are_capped_but_stay_a_prefix() {
        let long = "word ".repeat(200);
        let c = cap_to(&long, QUOTE_CAP);
        assert!(c.chars().count() <= QUOTE_CAP + 2);
        assert!(cap_to(&long, 120).chars().count() <= 122);
        assert!(normalise(&long).starts_with(c.trim_end_matches(" …")));
    }

    #[test]
    fn the_contract_rejects_prose_and_extra_fields() {
        assert!(parse_reply("The answer is 42.").is_err());
        assert!(parse_reply("Sure! {\"hits\": []}").is_err());
        assert!(parse_reply(r#"{"hits": [], "answer": "it is linear"}"#).is_err());
        let ok = parse_reply(
            "```json\n{\"hits\":[{\"file\":\"a.md\",\"page\":null,\"quote\":\"x y z\"}],\"search_terms\":[\"laplace\"]}\n```",
        )
        .unwrap();
        assert_eq!(ok.hits.len(), 1);
        assert_eq!(ok.search_terms, ["laplace"]);
    }

    #[test]
    fn suggested_terms_must_be_terms_not_reasoning() {
        let t = clean_terms(&[
            "Laplace transform".into(),
            "laplace transform".into(),
            "Because it is linear, the answer is yes.".into(),
            "x = 3".into(),
            "region of convergence".into(),
        ]);
        assert_eq!(t, ["Laplace transform", "region of convergence"]);
    }

    #[test]
    fn rendered_replies_hold_only_pointers_and_quotes() {
        let d = ws(&[("notes.md", NOTES)]);
        let h = verify_local(d.path(), &hit("notes.md", None, "It is linear.")).unwrap();
        let out = render(None, std::slice::from_ref(&h), &[], false, &[]);
        assert_eq!(
            out,
            "**Where to look:**\n\n- `notes.md:5`\n  > It is linear."
        );
        let none = render(
            None,
            &[],
            &["laplace".into()],
            true,
            &["Brave (no key)".into()],
        );
        assert!(none.starts_with("**Not found** here or online."));
        assert!(none.contains("`/find laplace`"));
        let tr = render(Some("tr"), &[], &["laplace".into()], false, &[]);
        assert_eq!(tr, "**Bulunamadı**, burada yok.\n\nDene: `/find laplace`");
        assert!(none.ends_with("Not searched: Brave (no key)."));
    }

    #[test]
    fn find_returns_raw_lines_and_open_numbers_them() {
        let d = ws(&[
            ("notes.md", NOTES),
            ("sub/b.txt", "nothing about it\nlinear algebra\n"),
        ]);
        let hits = find(d.path(), &["linear".into()], 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].quote, "It is linear.");
        let shown = open(d.path(), "notes.md", Some(5)).unwrap();
        assert!(shown.contains("    5  It is linear."));
        assert!(open(d.path(), "../x", None).is_err());
    }

    #[test]
    fn spelling_variants_match_and_show_the_source() {
        assert!(mentions("We run K-means on the data.", "kmeans"));
        assert!(mentions("We run k means on the data.", "K-Means"));
        assert!(mentions("Schrödinger's equation", "schrodinger"));
        assert!(mentions("Die Straße", "strasse"));
        assert!(mentions("Schrödinger's equation", "Schroedinger"));
        assert!(mentions("Schroedinger's equation", "schrödinger"));
        assert!(mentions("Übung 3: Lösung", "uebung"));
        assert!(mentions("die Lösung", "loesung"));
        assert_eq!(
            locate(
                &["Die Schroedinger-Gleichung gilt."],
                "Schrödinger-Gleichung"
            ),
            Some((1, "Schroedinger-Gleichung".to_string()))
        );
        assert!(!mentions("We run k-medoids.", "kmeans"));
    }

    #[test]
    fn word_forms_find_each_other() {
        let f = |t: &str| term_form(t);
        assert_eq!(f("clustering"), "clust");
        assert_eq!(f("clusters"), "clust");
        assert_eq!(f("cluster"), "clust");
        assert_eq!(f("K-means clustering"), "kmeansclust");
        assert_eq!(f("Gleichungen"), "gleich");
        assert_eq!(f("Gleichung"), "gleich");
        assert_eq!(f("theories"), "theor");
        assert!(loose("the theory of").0.contains(&f("theories")));
        // Too short to cut.
        assert_eq!(f("uses"), "uses");
        assert_eq!(f("head"), "head");
        let lines = [
            "Then K-means assigns each point",
            "to its nearest centroid.",
        ];
        assert_eq!(
            locate(&lines, "kmeans assigns each point to its nearest centroid"),
            Some((
                1,
                "K-means assigns each point to its nearest centroid.".to_string()
            ))
        );
        let d = ws(&[("c.md", "Intro\nWe use k means clustering here.\n")]);
        let hits = find(d.path(), &["K-Means".into()], 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].quote, "We use k means clustering here.");
    }

    #[test]
    fn model_queries_and_picks_are_kept_to_their_shape() {
        assert_eq!(
            parse_queries(
                r#"{"queries": ["k-means initialization", "K-Means Initialization", "Why is it slow? Because..."]}"#
            ),
            ["k-means initialization"]
        );
        assert!(parse_queries(r#"{"queries": ["a"], "answer": "x"}"#).is_empty());
        assert!(parse_queries("Search for k-means.").is_empty());
        assert_eq!(parse_pick(r#"{"pick": [2, 0, 2, 9]}"#, 3), Some(vec![2, 0]));
        assert_eq!(parse_pick(r#"{"pick": []}"#, 3), Some(vec![]));
        assert_eq!(parse_pick("none of them", 3), None);
        let page = "\u{1}A\nkmeans picks centroids at random first.\nThen kmeans moves each centroid to the mean.\n";
        let c = quote_candidates(page, &["kmeans".into(), "centroid".into()], 5);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn notes_keep_only_a_pointer() {
        let d = ws(&[("notes.md", NOTES)]);
        assert_eq!(
            pointer_note(d.path(), "Read notes.md:5 first.").as_deref(),
            Some("`notes.md:5`")
        );
        assert_eq!(
            pointer_note(
                d.path(),
                "See `notes.md`, the transform is linear because..."
            )
            .as_deref(),
            Some("`notes.md`")
        );
        assert_eq!(
            pointer_note(d.path(), "notes.md:99"),
            Some("`notes.md`".into())
        );
        assert_eq!(
            pointer_note(d.path(), "The weights scale each input."),
            None
        );
    }

    #[test]
    fn web_hits_are_checked_against_the_fetched_page() {
        let results = vec![
            WebResult {
                url: "https://example.org/a".into(),
                section: Some("Definition".into()),
                page_text: "Intro\nA phasor is a complex number.\n".into(),
                quote: "A phasor is a complex number.".into(),
            },
            WebResult {
                url: "https://example.org/b".into(),
                section: None,
                page_text: "Something else entirely.".into(),
                quote: "Phasors make AC analysis easy.".into(),
            },
        ];
        let mut twice = results.clone();
        twice.push(results[0].clone());
        let hits = web_hits(&twice);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].at.to_string(), "https://example.org/a § Definition");
    }

    #[test]
    fn the_quote_is_the_line_with_most_terms_under_its_heading() {
        let page = "\u{1}Intro\nK-means is old.\n\u{1}Algorithm\nThe kmeans algorithm assigns each point to the nearest centroid.\nCentroids are recomputed as the mean of their assigned points.\n";
        let (section, line) = pick_quote(page, &["Kmeans".into(), "centroid".into()]).unwrap();
        assert_eq!(section.as_deref(), Some("Algorithm"));
        assert!(line.starts_with("The kmeans algorithm"));
        assert!(pick_quote(page, &["laplace".into()]).is_none());
    }
}
