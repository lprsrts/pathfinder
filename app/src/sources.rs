//! Sources only: Pathfinder answers with pointers into sources and short
//! verbatim hits, never with its own reasoning.
//!
//! Everything here is plain code. The model may *propose* hits, but what the
//! user sees is rendered from this module only, after every quote has been
//! found in the cited source.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Longest quote shown by default, in characters (`quote_cap` in the config).
pub const QUOTE_CAP: usize = 220;

/// The configured quote cap (live from config, clamped to 60..=1000).
pub fn quote_cap() -> usize {
    crate::config::Config::load().quote_cap.clamp(60, 1000)
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

/// Kind of workspace source targeted by `/open`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Text,
    Markdown,
    Code,
    Pdf,
    Image,
    Video,
    Directory,
}

impl SourceKind {
    pub fn from_path(path: &Path) -> Self {
        if path.is_dir() {
            return Self::Directory;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "pdf" => Self::Pdf,
            "md" | "markdown" | "mdx" | "rst" | "org" => Self::Markdown,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tiff" | "heic" => {
                Self::Image
            }
            "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => Self::Video,
            "txt" | "csv" | "tsv" | "log" => Self::Text,
            "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "hpp" | "cc"
            | "java" | "go" | "m" | "r" | "jl" | "sh" | "zsh" | "bash" | "tex" | "bib" | "json"
            | "yaml" | "yml" | "toml" | "html" | "htm" | "css" | "sql" | "ipynb" | "xml" => {
                Self::Code
            }
            _ => Self::Text,
        }
    }
}

/// What `/open` opened: the file, page, line range and kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTarget {
    pub path: PathBuf,
    pub rel: String,
    pub page: Option<usize>,
    pub lines: Option<(usize, usize)>,
    pub quote: Option<String>,
    pub kind: SourceKind,
}

/// Result of opening a workspace source: rendered markdown plus structured target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedSource {
    pub markdown: String,
    pub target: OpenTarget,
}

/// Parsed `/open` argument.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenSpec {
    pub file: String,
    pub at: Option<usize>,
    pub end: Option<usize>,
    pub line: Option<usize>,
}

// ---------------------------------------------------------------- matching

/// The form quotes are compared in: typographic quotes, dashes and
/// ligatures unified, a word broken at a line end joined (its hyphen kept:
/// it is in the source, and [`loose`] ignores it), whitespace collapsed.
/// Case is kept: a quote must be the source's own words.
pub fn normalise(text: &str) -> String {
    static BROKEN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(\p{L})-\n[ \t]*(\p{L})").unwrap());
    let text = BROKEN.replace_all(text, "$1-$2");
    let text = text
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
        // The German way of writing ä, ö, ü without the marks, inside a
        // word only: "data entry" keeps its "a e".
        if matches!(c, 'a' | 'o' | 'u')
            && chars
                .get(k + 1)
                .is_some_and(|n| n.0 == 'e' && (n.1 == i || n.1 == i + 1))
        {
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
    // Each line folded once: a window that can't hold the quote is skipped
    // without building it, and a quote that isn't anywhere costs one pass.
    let folded_lines: Vec<String> = lines.iter().map(|l| loose(&normalise(l)).0).collect();
    if !folded_lines.concat().contains(&want) {
        return None;
    }
    // The first line the quote ends on, then the latest line it can start
    // on: that is where it is.
    for end in 0..lines.len() {
        for start in (end.saturating_sub(MAX_SPAN - 1)..=end).rev() {
            if !folded_lines[start..=end].concat().contains(&want) {
                continue;
            }
            let have = normalise(&lines[start..=end].join("\n"));
            let (folded, from) = loose(&have);
            let chars: Vec<char> = have.chars().collect();
            let mut next = 0;
            while let Some(found) = folded[next..].find(&want) {
                let at = next + found;
                // Matches may overlap: look again one char further on.
                next = at + folded[at..].chars().next().map_or(1, char::len_utf8);
                let first = folded[..at].chars().count();
                let (mut a, mut b) = (from[first].0, from[first + n - 1].1);
                // Whole words and numbers only: "stable" isn't quoted out of
                // "unstable", nor "5 ms" out of "15 ms" or "1.5 ms".
                if !starts_clean(&chars, a) || !ends_clean(&chars, b) {
                    continue;
                }
                // A sign belongs to its number: "-5", not "5".
                if a > 0 && "-+".contains(chars[a - 1]) && chars[a].is_ascii_digit() {
                    a -= 1;
                }
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

/// Whether a match starting at char `a` starts a word or number.
fn starts_clean(chars: &[char], a: usize) -> bool {
    let Some(&before) = a.checked_sub(1).and_then(|i| chars.get(i)) else {
        return true;
    };
    if before.is_alphanumeric() {
        return false;
    }
    // A decimal point or separator inside a number: "1.5", "1,000".
    let in_number = ".,'".contains(before)
        && chars[a].is_ascii_digit()
        && a >= 2
        && chars[a - 2].is_ascii_digit();
    !in_number
}

/// Whether a match ending at char `b` ends a word or number.
fn ends_clean(chars: &[char], b: usize) -> bool {
    let Some(&after) = chars.get(b + 1) else {
        return true;
    };
    if after.is_alphanumeric() {
        return false;
    }
    let in_number = ".,'".contains(after)
        && chars[b].is_ascii_digit()
        && chars.get(b + 2).is_some_and(|c| c.is_ascii_digit());
    !in_number
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
/// Accepts relative paths and workspace-internal absolute paths.
pub fn confine(cwd: &Path, file: &str) -> Option<PathBuf> {
    let clean = strip_file_uri(file.trim());
    let clean = clean.trim().trim_start_matches("./");
    if clean.is_empty() {
        return None;
    }
    // Symlinks and `..` are resolved through the longest existing ancestor.
    let resolved = crate::agent::confine::confine(cwd, clean, false, &[]).ok()?;
    resolved.is_file().then_some(resolved)
}

fn strip_file_uri(s: &str) -> String {
    let Some(rest) = s.strip_prefix("file://") else {
        return s.to_string();
    };
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8_lossy(&out).into_owned();
    // `file:///C:/x` names `C:/x` on Windows.
    let drive = path.as_bytes();
    if cfg!(windows)
        && drive.len() >= 3
        && drive[0] == b'/'
        && drive[1].is_ascii_alphabetic()
        && drive[2] == b':'
    {
        return path[1..].to_string();
    }
    path
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Whether `path` has a file extension that `/open` handles cleanly.
pub fn is_openable(path: &Path) -> bool {
    const EXTS: &[&str] = &[
        "md", "markdown", "mdx", "txt", "tex", "bib", "rst", "org", "csv", "tsv", "log", "json",
        "yaml", "yml", "toml", "html", "htm", "css", "sql", "xml", "rs", "py", "js", "ts", "tsx",
        "jsx", "c", "h", "cpp", "hpp", "cc", "java", "go", "m", "r", "jl", "sh", "zsh", "bash",
        "ipynb", "pdf", "png", "jpg", "jpeg", "gif", "webp", "svg", "mp4", "mov", "mkv", "webm",
        "avi", "m4v",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

pub fn is_pdf(p: &Path) -> bool {
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
    let raw = p.file.trim().trim_start_matches("./");
    let full = confine(cwd, raw).ok_or_else(|| Rejected::NoSuchFile(raw.to_string()))?;
    let root = dunce::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let file = full
        .strip_prefix(&root)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| raw.to_string());
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
    let Ok(root) = dunce::canonicalize(cwd) else {
        return out;
    };
    for entry in ignore::WalkBuilder::new(cwd).build().flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        // A linked file counts only if it really is inside the folder.
        if entry.path_is_symlink() && !dunce::canonicalize(p).is_ok_and(|r| r.starts_with(&root)) {
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

/// Most files one search reads.
const MAX_FIND_FILES: usize = 5000;

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
    // Most terms first, then the earliest; only the best `limit` are kept.
    let best = |scored: &mut Vec<(usize, usize, Hit)>| {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.truncate(limit);
    };
    let qcap = quote_cap();
    for (full, rel) in source_files(cwd).into_iter().take(MAX_FIND_FILES) {
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
                if scored.len() >= limit.max(64) * 4 {
                    best(&mut scored);
                }
                scored.push((
                    n,
                    seq,
                    Hit {
                        at: Locator::Local {
                            file: rel.clone(),
                            page: pdf.then_some(pi + 1),
                            line: li + 1,
                        },
                        quote: cap_to(line, qcap),
                    },
                ));
            }
        }
    }
    // Need every term when there are enough such lines; else the best.
    best(&mut scored);
    scored.into_iter().map(|(_, _, h)| h).collect()
}

/// Parses `/open` arguments:
/// - `"my notes.md" 12`, `'my notes.md':10-20`, `my\ notes.md 12`, `` `notes.md:12` ``
/// - `paper.pdf 3`, `paper.pdf p.3`, `paper.pdf p. 3 l. 12`, `paper.pdf page 3`
/// - `notes.md:120`, `notes.md:10-20`, `notes.md:10:5`, `notes.md 10-20`, `notes.md: 12`
pub fn parse_open_spec(raw: &str) -> OpenSpec {
    let mut s = raw.trim();
    if s.len() >= 2 && s.starts_with('`') && s.ends_with('`') {
        s = s[1..s.len() - 1].trim();
    }
    if s.is_empty() {
        return OpenSpec::default();
    }
    // Quoted path followed by optional page/line/range suffix.
    if let Some((quoted_path, rest)) = split_leading_quoted(s) {
        let mut spec = parse_open_suffix(rest);
        spec.file = normalize_open_file(&quoted_path);
        return spec;
    }
    // Unquoted: try PDF `p.N [l.M]` suffix first, then `:line`, then ` line`.
    let (file_raw, spec) = extract_open_suffix(s);
    let file = normalize_open_file(&crate::commands::unescape_shell_path(file_raw));
    OpenSpec { file, ..spec }
}

fn normalize_open_file(raw: &str) -> String {
    let expanded = crate::commands::expand_tilde(raw.trim());
    strip_file_uri(&expanded)
}

fn split_leading_quoted(s: &str) -> Option<(String, &str)> {
    let quote = s.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let mut out = String::new();
    let mut chars = s[1..].char_indices();
    while let Some((idx, c)) = chars.next() {
        if quote == '"'
            && c == '\\'
            && let Some((_, next)) = chars.next()
        {
            out.push(next);
            continue;
        }
        if c == quote {
            let rest = &s[1 + idx + c.len_utf8()..];
            return Some((out, rest.trim()));
        }
        out.push(c);
    }
    None
}

fn parse_range_nums(s: &str) -> Option<(usize, Option<usize>)> {
    let s = s.trim();
    if let Some((a, b)) = s.split_once(['-', '–']) {
        let start = a.trim().parse::<usize>().ok()?;
        let end = b.trim().parse::<usize>().ok()?;
        return Some((start, Some(end)));
    }
    Some((s.parse::<usize>().ok()?, None))
}

fn parse_open_suffix(rest: &str) -> OpenSpec {
    let rest = rest.trim();
    if rest.is_empty() {
        return OpenSpec::default();
    }
    let (_, spec) = extract_open_suffix(&format!("_ {rest}"));
    if spec.at.is_some() || spec.line.is_some() {
        return spec;
    }
    if let Some(colon) = rest.strip_prefix(':') {
        let (_, spec) = extract_open_suffix(&format!("_:{colon}"));
        return spec;
    }
    OpenSpec::default()
}

fn extract_open_suffix(s: &str) -> (&str, OpenSpec) {
    static PDF_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^(.+?)\s+(?:pp?\.|pages?|seite|s\.)\s*(\d+)(?:\s*[-–]\s*(\d+))?(?:\s+(?:ll?\.|lines?)\s*(\d+)(?:\s*[-–]\s*(\d+))?)?$",
        )
        .unwrap()
    });
    if let Some(caps) = PDF_RE.captures(s)
        && let Some(file) = caps.get(1)
        && let Ok(page) = caps[2].parse::<usize>()
    {
        let end = caps.get(3).and_then(|m| m.as_str().parse::<usize>().ok());
        let line = caps.get(4).and_then(|m| m.as_str().parse::<usize>().ok());
        let line_end = caps.get(5).and_then(|m| m.as_str().parse::<usize>().ok());
        return (
            file.as_str().trim(),
            OpenSpec {
                file: String::new(),
                at: Some(page),
                end: line_end.or(end),
                line,
            },
        );
    }
    // `file:line:col`, `file:start-end`, `file:line`, `file: 12`
    if let Some((before_last, last)) = s.rsplit_once(':') {
        let last_trim = last.trim();
        if let Ok(col_or_line) = last_trim.parse::<usize>() {
            if let Some((file_part, line_str)) = before_last.rsplit_once(':')
                && let Ok(line_num) = line_str.trim().parse::<usize>()
            {
                let _ = col_or_line;
                return (
                    file_part.trim(),
                    OpenSpec {
                        file: String::new(),
                        at: Some(line_num),
                        end: None,
                        line: None,
                    },
                );
            }
            if !before_last.trim().is_empty() {
                return (
                    before_last.trim(),
                    OpenSpec {
                        file: String::new(),
                        at: Some(col_or_line),
                        end: None,
                        line: None,
                    },
                );
            }
        } else if let Some((a, Some(b))) = parse_range_nums(last_trim)
            && !before_last.trim().is_empty()
        {
            return (
                before_last.trim(),
                OpenSpec {
                    file: String::new(),
                    at: Some(a),
                    end: Some(b),
                    line: None,
                },
            );
        }
    }
    // `file 12` or `file 10-20`
    if let Some((f, tail)) = s.rsplit_once(char::is_whitespace)
        && !f.trim().is_empty()
        && let Some((a, b)) = parse_range_nums(tail)
    {
        return (
            f.trim(),
            OpenSpec {
                file: String::new(),
                at: Some(a),
                end: b,
                line: None,
            },
        );
    }
    (s.trim(), OpenSpec::default())
}

fn lang_tag(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" => "javascript",
        "ts" => "typescript",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "md" | "markdown" => "markdown",
        "json" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "sh" | "zsh" | "bash" => "bash",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" => "cpp",
        "go" => "go",
        "java" => "java",
        "tex" => "latex",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" => "css",
        _ => "text",
    }
}

fn safe_fence(body: &str) -> String {
    let mut max_run = 2usize;
    let mut run = 0usize;
    for ch in body.chars() {
        if ch == '`' {
            run += 1;
            max_run = max_run.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((max_run + 1).max(3))
}

fn suggest_similar(cwd: &Path, wanted: &str) -> Vec<String> {
    let want_name = Path::new(wanted)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| wanted.to_lowercase());
    if want_name.is_empty() {
        return Vec::new();
    }
    let stem = want_name
        .rsplit_once('.')
        .map_or(want_name.as_str(), |(s, _)| s)
        .to_string();
    let Ok(root) = dunce::canonicalize(cwd) else {
        return Vec::new();
    };
    let mut near: Vec<(u8, String)> = Vec::new();
    for entry in ignore::WalkBuilder::new(&root)
        .build()
        .flatten()
        .take(10_000)
    {
        if !entry.path().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let rank = if name == want_name {
            0
        } else if stem.len() >= 3 && name.contains(&stem) {
            1
        } else if name.len() >= 4
            && want_name.contains(name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s))
        {
            2
        } else {
            continue;
        };
        if let Ok(rel) = entry.path().strip_prefix(&root) {
            near.push((rank, rel.to_string_lossy().replace('\\', "/")));
        }
    }
    near.sort();
    near.dedup();
    near.into_iter().take(4).map(|(_, p)| p).collect()
}

/// `/open`: numbered view of a file, PDF page, range, directory, or media hint.
pub fn open_spec(cwd: &Path, spec: &OpenSpec) -> Result<OpenedSource, String> {
    let raw_rel = spec.file.trim().trim_start_matches("./");
    if raw_rel.is_empty() {
        return Err("`/open <file> [page or line]`".into());
    }
    let resolved = crate::agent::confine::confine(cwd, raw_rel, false, &[])
        .map_err(|_| format!("No file `{raw_rel}` in this workspace."))?;
    if !resolved.exists() {
        let near = suggest_similar(cwd, raw_rel);
        let mut msg = format!("No file `{raw_rel}` in this workspace.");
        if !near.is_empty() {
            let formatted: Vec<String> = near.into_iter().map(|p| format!("`{p}`")).collect();
            msg.push_str(&format!(" Did you mean {}?", formatted.join(", ")));
        }
        return Err(msg);
    }
    let root = dunce::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let rel = resolved
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| raw_rel.to_string());
    let rel_display = if rel.is_empty() { ".".to_string() } else { rel };

    if resolved.is_dir() {
        let mut items: Vec<(bool, String)> = Vec::new();
        for entry in ignore::WalkBuilder::new(&resolved)
            .max_depth(Some(1))
            .build()
            .flatten()
            .skip(1)
        {
            let is_dir = entry.path().is_dir();
            let name = entry.file_name().to_string_lossy().into_owned();
            items.push((is_dir, name));
        }
        items.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.to_ascii_lowercase().cmp(&b.1.to_ascii_lowercase()))
        });
        let total = items.len();
        let shown: Vec<String> = items
            .into_iter()
            .take(40)
            .map(|(d, name)| {
                let prefix = if rel_display == "." {
                    String::new()
                } else {
                    format!("{rel_display}/")
                };
                format!("- `{prefix}{name}{}`", if d { "/" } else { "" })
            })
            .collect();
        let mut markdown = if shown.is_empty() {
            format!("`{rel_display}/` is an empty folder.")
        } else {
            format!("`{rel_display}/` ({total} items):\n\n{}", shown.join("\n"))
        };
        if total > 40 {
            markdown.push_str(&format!("\n\n… and {} more.", total - 40));
        }
        return Ok(OpenedSource {
            markdown,
            target: OpenTarget {
                path: resolved,
                rel: rel_display,
                page: None,
                lines: None,
                quote: None,
                kind: SourceKind::Directory,
            },
        });
    }

    let kind = SourceKind::from_path(&resolved);
    match kind {
        SourceKind::Image => {
            return Ok(OpenedSource {
                markdown: format!(
                    "`{rel_display}` is an image. Attach it with `/attach {}`.",
                    if rel_display.contains(char::is_whitespace) {
                        format!("\"{rel_display}\"")
                    } else {
                        rel_display.clone()
                    }
                ),
                target: OpenTarget {
                    path: resolved,
                    rel: rel_display,
                    page: None,
                    lines: None,
                    quote: None,
                    kind,
                },
            });
        }
        SourceKind::Video => {
            return Ok(OpenedSource {
                markdown: format!(
                    "`{rel_display}` is a video file, which Pathfinder cannot show as text."
                ),
                target: OpenTarget {
                    path: resolved,
                    rel: rel_display,
                    page: None,
                    lines: None,
                    quote: None,
                    kind,
                },
            });
        }
        _ => {}
    }

    let pages = pages(&resolved, &rel_display)?;
    const WINDOW: usize = 60;
    const MAX_RANGE: usize = 200;
    if is_pdf(&resolved) {
        if pages.is_empty() || pages.iter().all(|p| p.trim().is_empty()) {
            return Ok(OpenedSource {
                markdown: format!(
                    "`{rel_display}` has no extractable text (scanned PDF; attach with `/attach {}` or open in the PDF viewer).",
                    if rel_display.contains(char::is_whitespace) {
                        format!("\"{rel_display}\"")
                    } else {
                        rel_display.clone()
                    }
                ),
                target: OpenTarget {
                    path: resolved,
                    rel: rel_display,
                    page: spec.at.or(Some(1)),
                    lines: spec.line.map(|l| (l, spec.end.unwrap_or(l))),
                    quote: None,
                    kind: SourceKind::Pdf,
                },
            });
        }
        let n = spec.at.unwrap_or(1);
        let page = pages
            .get(n.wrapping_sub(1))
            .ok_or_else(|| format!("`{rel_display}` has {} pages.", pages.len()))?;
        let mut body = String::new();
        for (i, l) in page.lines().enumerate() {
            body.push_str(&format!("{:>4}  {}\n", i + 1, l.trim_end()));
        }
        let fence = safe_fence(&body);
        let out = format!(
            "`{rel_display}` page {n} of {}\n\n{fence}text\n{body}{fence}",
            pages.len()
        );
        return Ok(OpenedSource {
            markdown: out,
            target: OpenTarget {
                path: resolved,
                rel: rel_display,
                page: Some(n),
                lines: spec.line.map(|l| (l, spec.end.unwrap_or(l))),
                quote: None,
                kind: SourceKind::Pdf,
            },
        });
    }

    let lines: Vec<&str> = pages[0].lines().collect();
    if lines.is_empty() {
        return Err(format!("`{rel_display}` is empty."));
    }
    if let Some(l) = spec.at
        && (l == 0 || l > lines.len())
    {
        return Err(format!("`{rel_display}` has {} lines.", lines.len()));
    }
    let (start, end, target_lines) = match (spec.at, spec.end) {
        (Some(a), Some(b)) => {
            if b < a {
                return Err(format!("Invalid line range `{a}–{b}` for `{rel_display}`."));
            }
            let s = a - 1;
            let e = b.min(lines.len()).min(s + MAX_RANGE);
            (s, e, Some((a, e)))
        }
        (Some(l), None) => {
            let s = l.saturating_sub(WINDOW / 4).saturating_sub(1);
            let e = s.saturating_add(WINDOW).min(lines.len());
            (s, e, Some((l, l)))
        }
        (None, _) => {
            let e = WINDOW.min(lines.len());
            (0, e, Some((1, e)))
        }
    };
    let mut body = String::new();
    for (i, l) in lines.iter().enumerate().take(end).skip(start) {
        body.push_str(&format!("{:>5}  {}\n", i + 1, l.trim_end()));
    }
    let fence = safe_fence(&body);
    let lang = lang_tag(&resolved);
    let out = format!(
        "`{rel_display}` lines {}–{end} of {}\n\n{fence}{lang}\n{body}{fence}",
        start + 1,
        lines.len()
    );
    Ok(OpenedSource {
        markdown: out,
        target: OpenTarget {
            path: resolved,
            rel: rel_display,
            page: None,
            lines: target_lines,
            quote: None,
            kind,
        },
    })
}

/// `/open`: a numbered view of a file, a PDF page, or lines around a line.
pub fn open(cwd: &Path, file: &str, at: Option<usize>) -> Result<String, String> {
    let spec = OpenSpec {
        file: file.to_string(),
        at,
        end: None,
        line: None,
    };
    open_spec(cwd, &spec).map(|o| o.markdown)
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
        // A word broken at a line end is joined; its hyphen is the source's.
        assert_eq!(normalise("trans-\nform"), "trans-form");
        assert_eq!(normalise("b -\nc"), "b - c");
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
    fn open_stays_inside_the_file() {
        let body: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        let d = ws(&[("notes.md", body.as_str()), ("empty.md", "")]);
        assert!(
            open(d.path(), "notes.md", Some(90))
                .unwrap()
                .contains("lines 75–100 of 100")
        );
        assert_eq!(
            open(d.path(), "notes.md", Some(500)).unwrap_err(),
            "`notes.md` has 100 lines."
        );
        assert!(open(d.path(), "notes.md", Some(usize::MAX)).is_err());
        assert!(open(d.path(), "notes.md", Some(0)).is_err());
        assert_eq!(
            open(d.path(), "empty.md", None).unwrap_err(),
            "`empty.md` is empty."
        );
    }

    #[test]
    fn a_quote_is_whole_words_from_the_source() {
        let unstable = ["The closed loop is unstable for all k > 1."];
        assert_eq!(locate(&unstable, "stable for all k > 1"), None);
        assert_eq!(
            locate(&unstable, "unstable for all k > 1").unwrap().1,
            "unstable for all k > 1."
        );
        let ms = ["Each request takes 15 ms on average here, 1.5 ms at best."];
        assert_eq!(locate(&ms, "5 ms on average here"), None);
        assert_eq!(locate(&ms, "5 ms at best"), None);
        assert_eq!(
            locate(&["The offset is -5 mm here."], "5 mm here")
                .unwrap()
                .1,
            "-5 mm here."
        );
        // The same words later in the line, standing on their own, are found.
        let twice = ["unstable at first, then stable for all k > 1."];
        assert_eq!(
            locate(&twice, "stable for all k > 1").unwrap().1,
            "stable for all k > 1."
        );
    }

    #[test]
    fn nothing_is_shown_that_isnt_in_the_source() {
        // A minus at a line end stays in what is shown.
        let minus = ["The result is a = b -", "c, as shown."];
        assert_eq!(
            locate(&minus, "result is a = b c").unwrap().1,
            "result is a = b - c,"
        );
        // A word broken over two lines is joined, with its hyphen.
        let broken = ["a well-", "known result"];
        assert_eq!(
            locate(&broken, "well-known result").unwrap().1,
            "well-known result"
        );
        assert_eq!(
            locate(&["the tech-", "nology works"], "the technology works")
                .unwrap()
                .1,
            "the tech-nology works"
        );
    }

    #[test]
    fn umlaut_spellings_fold_only_inside_a_word() {
        assert!(mentions("convert it into equations", "equations"));
        assert_eq!(
            locate(
                &["We feed the data entry form to the model."],
                "entry form to the model"
            )
            .unwrap()
            .1,
            "entry form to the model."
        );
        assert!(
            loose("zu entscheiden")
                .0
                .contains(&term_form("entscheiden"))
        );
        assert!(mentions("Die Schrödinger-Gleichung", "Schroedinger"));
        assert!(mentions("Die Schroedinger-Gleichung", "Schrödinger"));
    }

    #[test]
    fn paths_outside_the_workspace_are_refused() {
        let d = ws(&[("notes.md", NOTES)]);
        assert!(confine(d.path(), "../notes.md").is_none());
        assert!(confine(d.path(), "/etc/hosts").is_none());
        assert!(confine(d.path(), "./notes.md").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn links_out_of_the_folder_are_outside_it() {
        let outside = ws(&[("id_rsa", "secret key\n")]);
        let d = tempdir::Dir::new();
        std::os::unix::fs::symlink(outside.path(), d.path().join("home")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("id_rsa"), d.path().join("key.txt"))
            .unwrap();
        std::fs::write(d.path().join("notes.md"), "secret notes\n").unwrap();
        assert!(confine(d.path(), "home/id_rsa").is_none());
        assert!(confine(d.path(), "key.txt").is_none());
        let files: Vec<String> = source_files(d.path()).into_iter().map(|f| f.1).collect();
        assert_eq!(files, ["notes.md"]);
        let hits = find(d.path(), &["secret".to_string()], 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].quote, "secret notes");
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

    #[test]
    fn parse_open_spec_handles_quotes_escapes_pdf_citations_and_ranges() {
        assert_eq!(
            parse_open_spec(r#""my notes.md" 12"#),
            OpenSpec {
                file: "my notes.md".into(),
                at: Some(12),
                end: None,
                line: None,
            }
        );
        assert_eq!(
            parse_open_spec(r#"'my notes.md':10-20"#),
            OpenSpec {
                file: "my notes.md".into(),
                at: Some(10),
                end: Some(20),
                line: None,
            }
        );
        // Backslash escapes are a Unix shell's; on Windows `\` separates folders.
        let escaped = parse_open_spec(r#"my\ notes.md 12"#);
        let expected_file = if cfg!(windows) {
            r"my\ notes.md"
        } else {
            "my notes.md"
        };
        assert_eq!(
            escaped,
            OpenSpec {
                file: expected_file.into(),
                at: Some(12),
                end: None,
                line: None,
            }
        );
        assert_eq!(
            parse_open_spec("`notes.md:12`"),
            OpenSpec {
                file: "notes.md".into(),
                at: Some(12),
                end: None,
                line: None,
            }
        );
        assert_eq!(
            parse_open_spec("slides-4.pdf p. 12"),
            OpenSpec {
                file: "slides-4.pdf".into(),
                at: Some(12),
                end: None,
                line: None,
            }
        );
        assert_eq!(
            parse_open_spec("file.pdf p.3 l.12"),
            OpenSpec {
                file: "file.pdf".into(),
                at: Some(3),
                end: None,
                line: Some(12),
            }
        );
        assert_eq!(
            parse_open_spec("src/main.rs:42:5"),
            OpenSpec {
                file: "src/main.rs".into(),
                at: Some(42),
                end: None,
                line: None,
            }
        );
        assert_eq!(
            parse_open_spec("notes.md: 15"),
            OpenSpec {
                file: "notes.md".into(),
                at: Some(15),
                end: None,
                line: None,
            }
        );
    }

    #[test]
    fn open_spec_handles_ranges_dirs_internal_abs_paths_fences_and_hints() {
        let body: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        let md_with_fences = "# Title\n\n```rust\nfn main() {}\n```\n";
        let d = ws(&[
            ("sub/notes.md", body.as_str()),
            ("README.md", md_with_fences),
            ("pic.png", "png"),
        ]);

        // Line range + language tag.
        let r = open_spec(d.path(), &parse_open_spec("sub/notes.md:5-10")).unwrap();
        assert!(r.markdown.contains("lines 5–10 of 40"), "{}", r.markdown);
        assert!(r.markdown.contains("```markdown\n"), "{}", r.markdown);
        assert_eq!(r.target.lines, Some((5, 10)));
        assert_eq!(r.target.kind, SourceKind::Markdown);

        // Safe fence escaping when markdown contains ```.
        let fenced = open_spec(d.path(), &parse_open_spec("README.md")).unwrap();
        assert!(
            fenced.markdown.contains("````markdown\n"),
            "{}",
            fenced.markdown
        );

        // Directory listing instead of "No file".
        let dir = open_spec(d.path(), &parse_open_spec("sub")).unwrap();
        assert_eq!(dir.target.kind, SourceKind::Directory);
        assert!(dir.markdown.contains("sub/notes.md"), "{}", dir.markdown);

        // Workspace-internal absolute path allowed.
        let abs = d.path().join("sub/notes.md").display().to_string();
        let opened_abs = open_spec(d.path(), &parse_open_spec(&abs)).unwrap();
        assert_eq!(opened_abs.target.rel, "sub/notes.md");

        // Image handled cleanly.
        let img = open_spec(d.path(), &parse_open_spec("pic.png")).unwrap();
        assert_eq!(img.target.kind, SourceKind::Image);

        // Missing file suggests near match.
        let err = open_spec(d.path(), &parse_open_spec("notes.md")).unwrap_err();
        assert!(err.contains("Did you mean `sub/notes.md`?"), "{err}");
    }
}
