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

/// How far each page of [`pages`] can be trusted: a PDF page's math may
/// not have extracted cleanly (see [`crate::pdftext`]). Other sources are
/// all clean.
pub fn unclean_pages(full: &Path, rel: &str) -> Vec<crate::pdftext::PageTrust> {
    if !is_pdf(full) {
        return Vec::new();
    }
    crate::agent::tools::pdf_text(full, rel)
        .map(|t| t.unclean.clone())
        .unwrap_or_default()
}

/// How far each page of a source can be trusted, with what quoting from
/// a [`crate::pdftext::PageTrust::Lettered`] page is checked against.
#[derive(Debug, Default)]
pub struct PageChecks {
    trust: Vec<crate::pdftext::PageTrust>,
    /// Per page: the [`loose`] forms of its trusted segments.
    trusted: Vec<Vec<String>>,
    /// Per page, once needed: its whole text in [`loose`] form.
    folded: Vec<std::sync::OnceLock<String>>,
}

impl PageChecks {
    /// The checks for a source's pages (all clean for anything but a PDF).
    pub fn of(full: &Path, rel: &str) -> Self {
        if !is_pdf(full) {
            return Self::default();
        }
        let Ok(t) = crate::agent::tools::pdf_text(full, rel) else {
            return Self::default();
        };
        Self {
            trust: t.unclean.clone(),
            trusted: t
                .trusted
                .iter()
                .map(|segs| segs.iter().map(|s| loose(s).0).collect())
                .collect(),
            folded: (0..t.unclean.len()).map(|_| Default::default()).collect(),
        }
    }

    fn trust(&self, i: usize) -> crate::pdftext::PageTrust {
        self.trust.get(i).copied().unwrap_or_default()
    }

    /// Whether `span`, text of page `i` (0-based) whose full text is
    /// `page`, can stand as the source's own words: on a page whose math
    /// didn't extract cleanly, only plain prose can; on one whose math font
    /// came out as ordinary letters (`X` for ∑, `m` for μ), only plain prose
    /// none of whose letters came from that font, and on one where those
    /// letters can't be told apart, nothing.
    fn quotable(&self, i: usize, page: &str, span: &str) -> bool {
        use crate::pdftext::{PageTrust, plain_prose};
        match self.trust(i) {
            PageTrust::Clean => true,
            PageTrust::Shaky => plain_prose(span),
            PageTrust::Lettered => plain_prose(span) && self.untouched(i, page, span),
            PageTrust::Unquotable => false,
        }
    }

    /// Whether every place `span` occurs on page `i` is in its trusted
    /// text: a span with a wrong letter in it, or the same words elsewhere
    /// next to one, is not.
    fn untouched(&self, i: usize, page: &str, span: &str) -> bool {
        let want = loose(&normalise(span)).0;
        if want.is_empty() {
            return false;
        }
        let Some(cell) = self.folded.get(i) else {
            return false;
        };
        let folded = cell.get_or_init(|| loose(&normalise(page)).0);
        let on_page = occurrences(folded, &want);
        let trusted: usize = self
            .trusted
            .get(i)
            .map_or(0, |segs| segs.iter().map(|s| occurrences(s, &want)).sum());
        trusted > 0 && on_page <= trusted
    }
}

/// How many times `want` occurs in `hay`, overlaps included.
fn occurrences(hay: &str, want: &str) -> usize {
    let mut n = 0;
    let mut from = 0;
    while let Some(at) = hay[from..].find(want) {
        n += 1;
        from += at + hay[from + at..].chars().next().map_or(1, char::len_utf8);
    }
    n
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
    let checks = PageChecks::of(&full, &file);
    // The cited page first, then the rest: a wrong page number is corrected,
    // a quote that isn't in the file at all is dropped.
    let cited = p
        .page
        .filter(|n| pdf && *n >= 1 && *n <= pages.len())
        .map(|n| n - 1);
    let mut order: Vec<usize> = cited.into_iter().chain(0..pages.len()).collect();
    let mut seen = std::collections::HashSet::new();
    order.retain(|i| seen.insert(*i));
    let hit = |i: usize, line: usize, own: &str| Hit {
        at: Locator::Local {
            file: file.clone(),
            page: pdf.then_some(i + 1),
            line,
        },
        quote: cap(own),
    };
    // The quote as it stands. Garbled math is never shown as what the
    // source says.
    for &i in &order {
        let lines: Vec<&str> = pages[i].lines().collect();
        if let Some((line, own)) = locate(&lines, &p.quote)
            && checks.quotable(i, &pages[i], &own)
        {
            return Ok(hit(i, line, &own));
        }
    }
    // A quote that runs into math the page can't vouch for: its own prose,
    // or, for the math itself, the clean line that introduces it (the
    // equation's lead-in, its proposition's title, its section heading).
    for &i in &order {
        let lines: Vec<&str> = pages[i].lines().collect();
        if let Some((line, own)) =
            prose_part(&lines, &p.quote, |own| checks.quotable(i, &pages[i], own))
        {
            return Ok(hit(i, line, &own));
        }
        let at = locate(&lines, &p.quote)
            .map(|(line, _)| line - 1)
            .or_else(|| {
                (Some(i) == cited)
                    .then(|| locate_math(&lines, &p.quote))
                    .flatten()
            });
        if let Some(at) = at
            && let Some((line, own)) = lead_in(&lines, at, |own| checks.quotable(i, &pages[i], own))
        {
            return Ok(hit(i, line, &own));
        }
    }
    Err(Rejected::NotInSource { file, page: p.page })
}

/// Fewest words, and characters, a part of a quote must have to stand
/// for it.
const PART_WORDS: usize = 3;
const PART_CHARS: usize = 15;

/// The longest run of plain words in `quote` (cut at math symbols and
/// sentence ends) that is in `lines` and passes `ok`, with its line.
fn prose_part(lines: &[&str], quote: &str, ok: impl Fn(&str) -> bool) -> Option<(usize, String)> {
    let q = normalise(quote);
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = q.chars().collect();
    for (k, &c) in chars.iter().enumerate() {
        let sentence_end = matches!(c, '.' | ':' | ';' | '?' | '!')
            && chars.get(k + 1).is_none_or(|n| n.is_whitespace());
        if crate::pdftext::prose_char(c) {
            cur.push(c);
            if sentence_end {
                parts.push(std::mem::take(&mut cur));
            }
        } else {
            parts.push(std::mem::take(&mut cur));
        }
    }
    parts.push(cur);
    let mut parts: Vec<String> = parts
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| {
            p.split_whitespace()
                .filter(|w| w.chars().any(char::is_alphabetic))
                .count()
                >= PART_WORDS
                && p.chars().count() >= PART_CHARS
        })
        .collect();
    // The whole quote was already tried. A part stands for the quote only
    // when it holds most of its words: a real phrase never vouches for
    // words around it the source doesn't have ("we prove = the L2
    // distance between mixtures = is always zero").
    let words = |t: &str| {
        t.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 2 && w.chars().any(char::is_alphabetic))
            .count()
    };
    let all = words(&q);
    parts.retain(|p| *p != q && words(p) * 2 > all);
    parts.sort_by_key(|p| std::cmp::Reverse(p.chars().count()));
    parts
        .into_iter()
        .find_map(|part| locate(lines, &part).filter(|(_, own)| ok(own)))
}

/// Most lines above a piece of math its lead-in, and the title of the
/// statement it belongs to, are looked for in.
const LEAD_IN_LINES: usize = 12;
const TITLE_LINES: usize = 24;

/// The clean line introducing the math at line `at` (0-based), looked for
/// from there upwards: the title of the proposition, theorem or definition
/// it is part of (`Proposition B.3 (Bayesian Fusion for GMMs).`), else the
/// nearest line, or its first sentences, of at least [`PART_WORDS`] words
/// that pass `ok`. A line that only finishes a word broken off the line
/// before it (`sity of X, then:`) is skipped. Returns its 1-based line and
/// text.
fn lead_in(lines: &[&str], at: usize, ok: impl Fn(&str) -> bool) -> Option<(usize, String)> {
    static TITLE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"^(Proposition|Theorem|Lemma|Corollary|Definition|Remark|Example|Assumption|Satz|Beispiel|Bemerkung)\s+([A-Z]\.?)?[0-9]+(\.[0-9]+)*\b",
        )
        .unwrap()
    });
    let last = at.min(lines.len().saturating_sub(1));
    let title = (at.saturating_sub(TITLE_LINES)..=last)
        .rev()
        .filter(|k| TITLE.is_match(lines[*k].trim_start()))
        .find_map(|k| first_sentences(lines, k, true, &ok));
    if title.is_some() {
        return title;
    }
    (at.saturating_sub(LEAD_IN_LINES)..=last)
        .rev()
        .find_map(|k| first_sentences(lines, k, false, &ok))
}

/// Line `k` (0-based), or its longest run of whole first sentences (the
/// shortest, for a title), if it is at least [`PART_WORDS`] words that pass
/// `ok` and doesn't just finish a word broken off the line before it. Its
/// 1-based line and text.
fn first_sentences(
    lines: &[&str],
    k: usize,
    shortest: bool,
    ok: impl Fn(&str) -> bool,
) -> Option<(usize, String)> {
    let broken = k > 0 && {
        let prev: Vec<char> = lines[k - 1].trim_end().chars().collect();
        prev.len() >= 2 && prev[prev.len() - 1] == '-' && prev[prev.len() - 2].is_alphabetic()
    };
    if broken {
        return None;
    }
    let text = normalise(lines[k]);
    // The whole line, else its longest run of whole sentences.
    let chars: Vec<char> = text.chars().collect();
    let mut ends: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(j, c)| {
            matches!(c, '.' | ':' | ';' | '?' | '!')
                && chars.get(j + 1).is_none_or(|n| n.is_whitespace())
        })
        .map(|(j, _)| j + 1)
        .collect();
    ends.push(chars.len());
    ends.sort_unstable();
    ends.dedup();
    if !shortest {
        ends.reverse();
    }
    for end in ends {
        let cand: String = chars[..end].iter().collect();
        let cand = cand.trim();
        let words = cand
            .split_whitespace()
            .filter(|w| w.chars().any(char::is_alphabetic))
            .count();
        if words >= PART_WORDS && ok(cand) {
            return Some((k + 1, cand.to_string()));
        }
    }
    None
}

/// Where a piece of math quoted from a rendered page (`c_{ab} = …`, with
/// sub- and superscripts the text layout puts on lines of their own) most
/// likely is in `lines`: the 0-based first line of the smallest window of
/// up to [`MAX_SPAN`] lines, one of them math, holding at least 80% of its
/// letters and digits. Only a quote that is math itself (an `=`, a sub- or
/// superscript, a LaTeX command or a relation) is looked for so: prose
/// that isn't in the source is still not found.
fn locate_math(lines: &[&str], quote: &str) -> Option<usize> {
    let math = quote.chars().any(|c| {
        matches!(
            c,
            '=' | '_' | '^' | '\\' | '≤' | '≥' | '≈' | '∝' | '≠' | '⁻' | 'ᵀ'
        ) || ('₀'..='₉').contains(&c)
    });
    let want = math_letters(quote);
    if !math || loose(&want).0.chars().count() < 8 {
        return None;
    }
    let bag = |s: &str| {
        let mut m: std::collections::HashMap<char, usize> = std::collections::HashMap::new();
        for c in loose(&normalise(s)).0.chars() {
            *m.entry(c).or_default() += 1;
        }
        m
    };
    let need = bag(&want);
    let total: usize = need.values().sum();
    // The quote's words (three letters or more, no function name): a
    // formula is never placed by letters its words only share.
    const FUNCTIONS: &[&str] = &[
        "exp", "det", "log", "sin", "cos", "tan", "max", "min", "arg", "sup", "inf", "lim", "var",
        "cov", "diag", "sqrt", "frac",
    ];
    let words: Vec<String> = want
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && w.chars().all(char::is_alphabetic))
        .map(|w| loose(w).0)
        .filter(|w| !FUNCTIONS.contains(&w.as_str()))
        .collect();
    let mut best: Option<(usize, usize, usize)> = None; // (covered, -span, start)
    for start in 0..lines.len() {
        for span in 1..=MAX_SPAN.min(lines.len() - start) {
            let window = &lines[start..start + span];
            if window
                .iter()
                .all(|l| l.trim().is_empty() || crate::pdftext::plain_prose(l))
            {
                continue;
            }
            let text = window.join("\n");
            let have = bag(&text);
            let covered: usize = need
                .iter()
                .map(|(c, n)| (*n).min(have.get(c).copied().unwrap_or(0)))
                .sum();
            if covered * 5 < total * 4 {
                continue;
            }
            // Most of what the window holds is the quote's too: letters a
            // long stretch of other text happens to have don't place it.
            let held: usize = have.values().sum();
            let flat = loose(&normalise(&text)).0;
            if covered * 5 < held * 3 || !words.iter().all(|w| flat.contains(w.as_str())) {
                continue;
            }
            let better = match best {
                None => true,
                Some((bc, bs, _)) => covered > bc || (covered == bc && span < bs),
            };
            if better {
                best = Some((covered, span, start));
            }
            break;
        }
    }
    best.map(|(_, _, start)| start)
}

/// A math quote's letters and digits as the text layout has them: LaTeX
/// commands as their Greek letter or function name (others dropped), and
/// Unicode sub- and superscript digits as digits.
fn math_letters(quote: &str) -> String {
    const GREEK: &[(&str, &str)] = &[
        ("alpha", "α"),
        ("beta", "β"),
        ("gamma", "γ"),
        ("delta", "δ"),
        ("epsilon", "ε"),
        ("varepsilon", "ε"),
        ("zeta", "ζ"),
        ("eta", "η"),
        ("theta", "θ"),
        ("kappa", "κ"),
        ("lambda", "λ"),
        ("mu", "μ"),
        ("nu", "ν"),
        ("xi", "ξ"),
        ("pi", "π"),
        ("rho", "ρ"),
        ("sigma", "σ"),
        ("tau", "τ"),
        ("phi", "φ"),
        ("varphi", "φ"),
        ("chi", "χ"),
        ("psi", "ψ"),
        ("omega", "ω"),
        ("Gamma", "Γ"),
        ("Delta", "Δ"),
        ("Theta", "Θ"),
        ("Lambda", "Λ"),
        ("Xi", "Ξ"),
        ("Pi", "Π"),
        ("Sigma", "Σ"),
        ("Phi", "Φ"),
        ("Psi", "Ψ"),
        ("Omega", "Ω"),
        ("det", "det"),
        ("exp", "exp"),
        ("log", "log"),
        ("ln", "ln"),
        ("sin", "sin"),
        ("cos", "cos"),
        ("tan", "tan"),
        ("max", "max"),
        ("min", "min"),
        ("tr", "tr"),
    ];
    static CMD: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\\([A-Za-z]+)").unwrap());
    let text = CMD.replace_all(quote, |c: &regex::Captures| {
        GREEK
            .iter()
            .find(|(k, _)| *k == &c[1])
            .map_or(" ", |(_, v)| v)
            .to_string()
    });
    text.chars()
        .map(|c| match c {
            '₀'..='₉' => char::from_u32(u32::from(c) - 0x2080 + 0x30).unwrap_or(c),
            '⁰' => '0',
            '¹' => '1',
            '²' => '2',
            '³' => '3',
            '⁴'..='⁹' => char::from_u32(u32::from(c) - 0x2070 + 0x30).unwrap_or(c),
            _ => c,
        })
        .collect()
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
    // A linked file counts only if it really is inside the folder, and
    // Pathfinder's private files (a database or staging copy kept in the
    // workspace) never do.
    let visible = crate::agent::confine::Visible::new(&root);
    for entry in visible.walk(cwd).build().flatten() {
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

/// Most files one search reads.
const MAX_FIND_FILES: usize = 5000;

/// Lines in the workspace's sources that contain the terms: the ones
/// holding them as one phrase first, then the most terms, then the
/// earliest. Raw lines, nothing else.
pub fn find(cwd: &Path, terms: &[String], limit: usize) -> Vec<Hit> {
    search(cwd, terms, limit, false).0
}

/// `/find`: [`find`], plus how many lines matched in all (searched files
/// only). When some lines hold every term, only those count: a line with
/// one word of `Bayesian Fusion for GMMs` isn't a match for it.
pub fn find_counted(cwd: &Path, terms: &[String], limit: usize) -> (Vec<Hit>, usize) {
    search(cwd, terms, limit, true)
}

fn search(cwd: &Path, terms: &[String], limit: usize, strict: bool) -> (Vec<Hit>, usize) {
    let words: Vec<&String> = terms.iter().filter(|t| !term_form(t).is_empty()).collect();
    if words.is_empty() {
        return (Vec::new(), 0);
    }
    // Several words: also looked for as the phrase they make.
    let phrase = (words.len() >= 2).then(|| {
        term_form(
            &words
                .iter()
                .map(|w| w.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        )
    });
    // "for", "of", "the" count toward the phrase, not as terms of their
    // own: "for" is in "information".
    let mut counted: Vec<String> = words
        .iter()
        .filter(|w| !keywords(w).is_empty())
        .map(|w| term_form(w))
        .collect();
    if counted.is_empty() {
        counted = words.iter().map(|w| term_form(w)).collect();
    }
    let all = counted.len() + 1;
    let mut total = 0usize;
    let mut total_all = 0usize;
    // (rank, seq, hit): the phrase ranks above every term, every term
    // above some.
    let mut scored: Vec<(usize, usize, Hit)> = Vec::new();
    let mut seq = 0;
    // Best rank first, then the earliest; only the best `limit` are kept.
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
        let checks = PageChecks::of(&full, &rel);
        for (pi, page) in pages.iter().enumerate() {
            for (li, line) in page.lines().enumerate() {
                let low = loose(line).0;
                let n = counted.iter().filter(|t| low.contains(t.as_str())).count();
                if n == 0 || !checks.quotable(pi, page, line) {
                    continue;
                }
                let rank = if phrase.as_ref().is_some_and(|p| low.contains(p.as_str())) {
                    all
                } else {
                    n
                };
                total += 1;
                if rank >= counted.len() {
                    total_all += 1;
                }
                seq += 1;
                if scored.len() >= limit.max(64) * 4 {
                    best(&mut scored);
                }
                scored.push((
                    rank,
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
    best(&mut scored);
    if strict && total_all > 0 {
        scored.retain(|(rank, _, _)| *rank >= counted.len());
        total = total_all;
    }
    (scored.into_iter().map(|(_, _, h)| h).collect(), total)
}

/// An `/open` reply split for a frontend: the path, what part of it is
/// shown (`lines 20–24 of 391`, `page 3 of 12`, `12 items`) and the rest
/// (the numbered block and its footer, or the folder's list). None for a
/// notice (an error, an image, an empty folder).
pub fn split_source(markdown: &str) -> Option<(String, String, String)> {
    let (first, rest) = markdown.split_once('\n').unwrap_or((markdown, ""));
    let after = first.strip_prefix('`')?;
    let (title, range) = after.split_once('`')?;
    let range = range.trim();
    let range = if range.starts_with("lines ") || range.starts_with("page ") {
        range.to_string()
    } else {
        let n = range.strip_prefix('(')?.strip_suffix(" items):")?;
        n.parse::<usize>().ok()?;
        format!("{n} items")
    };
    Some((
        title.to_string(),
        range,
        rest.trim_matches('\n').to_string(),
    ))
}

/// How many lines a stored `/find` reply says matched: the `48` of its
/// leading `**12/48**`, else the hits it lists.
pub fn find_total(reply: &str) -> Option<usize> {
    let count = reply.strip_prefix("**")?.split("**").next()?;
    let (_, total) = count.split_once('/')?;
    total.parse().ok()
}

/// A `/find` reply's hit row (`` - `file:12`  text ``) read back.
pub fn parse_hit(row: &str) -> Option<Hit> {
    let (at, quote) = row.strip_prefix("- `")?.split_once("`  ")?;
    let local = |file: &str, page: Option<usize>, line: &str| {
        Some(Locator::Local {
            file: file.to_string(),
            page,
            line: line.parse().ok()?,
        })
    };
    let at = match at.rsplit_once(" l.") {
        Some((head, line)) => {
            let (file, page) = head.rsplit_once(" p.")?;
            local(file, Some(page.parse().ok()?), line)?
        }
        None => {
            let (file, line) = at.rsplit_once(':')?;
            local(file, None, line)?
        }
    };
    Some(Hit {
        at,
        quote: quote.to_string(),
    })
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
        // A page whose math came out garbled says so, above its text.
        let note = if unclean_pages(&resolved, &rel_display)
            .get(n - 1)
            .is_some_and(|t| t.marked())
        {
            format!("{}\n\n", crate::pdftext::unclean_note(n))
        } else {
            String::new()
        };
        let mut out = format!(
            "`{rel_display}` page {n} of {}\n\n{note}{fence}text numbered\n{body}{fence}",
            pages.len()
        );
        if n < pages.len() {
            out.push_str(&format!(
                "\npages {}–{} not shown · `/open {} {}`",
                n + 1,
                pages.len(),
                open_arg(&rel_display),
                n + 1
            ));
        }
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
    let mut out = format!(
        "`{rel_display}` lines {}–{end} of {}\n\n{fence}{lang} numbered\n{body}{fence}",
        start + 1,
        lines.len()
    );
    if end < lines.len() {
        out.push_str(&next_range_footer(&rel_display, end, lines.len(), WINDOW));
    }
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

/// A path as `/open` takes it back: quoted when it has spaces.
fn open_arg(rel: &str) -> String {
    if rel.contains(char::is_whitespace) {
        format!("'{rel}'")
    } else {
        rel.to_string()
    }
}

/// The line after a numbered block that was cut: what isn't shown and the
/// `/open` that shows the next `window` lines. The TUI draws it as the
/// block's `└─` row.
fn next_range_footer(rel: &str, end: usize, total: usize, window: usize) -> String {
    let next_end = (end + window).min(total);
    format!(
        "\n{}–{total} not shown · `/open {}:{}-{next_end}`",
        end + 1,
        open_arg(rel),
        end + 1
    )
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

/// How many lines a workspace source has (on `page` for a PDF), for
/// clamping a range of lines to it.
pub fn line_count(cwd: &Path, file: &str, page: Option<usize>) -> Option<usize> {
    let full = confine(cwd, file)?;
    let pages = pages(&full, file).ok()?;
    let text = pages.get(page.unwrap_or(1).checked_sub(1)?)?;
    Some(text.lines().count())
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

/// What makes two web addresses the same source: one arXiv paper's
/// `abs/`, `html/` and `pdf/` pages and its versions are one paper (and so is
/// its `10.48550/arXiv.` DOI), a DOI is one work whichever resolver serves
/// it, and other pages compare without scheme, `www.`, fragment or trailing
/// slash.
pub fn source_key(url: &str) -> String {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower
        .split_once("://")
        .map_or(lower.as_str(), |(_, r)| r)
        .split('#')
        .next()
        .unwrap_or_default();
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    // arXiv by its id, without a version or `.pdf`.
    let arxiv_id = |id: &str| -> Option<String> {
        let id = id.split(['?', '#']).next()?.trim_end_matches('/');
        let id = id.strip_suffix(".pdf").unwrap_or(id);
        let id = match id.rfind('v') {
            Some(i)
                if i + 1 < id.len()
                    && id[i + 1..].bytes().all(|b| b.is_ascii_digit())
                    && id[..i].ends_with(|c: char| c.is_ascii_digit()) =>
            {
                &id[..i]
            }
            _ => id,
        };
        (!id.is_empty() && id.contains(|c: char| c.is_ascii_digit())).then(|| format!("arxiv:{id}"))
    };
    for host in ["arxiv.org/", "export.arxiv.org/"] {
        if let Some(path) = rest.strip_prefix(host) {
            for kind in ["abs/", "html/", "pdf/"] {
                if let Some(key) = path.strip_prefix(kind).and_then(arxiv_id) {
                    return key;
                }
            }
        }
    }
    // A DOI wherever it is served from; an arXiv DOI is the arXiv paper.
    if let Some(at) = rest.find("10.").filter(|&i| {
        let tail = &rest[i + 3..];
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        digits >= 4
            && tail[digits..].starts_with('/')
            && (i == 0 || rest[..i].ends_with('/') || rest[..i].ends_with(':'))
    }) {
        let doi = rest[at..].split(['?', '#']).next().unwrap_or_default();
        let doi = doi.trim_end_matches('/');
        if let Some(key) = doi.strip_prefix("10.48550/arxiv.").and_then(arxiv_id) {
            return key;
        }
        return format!("doi:{doi}");
    }
    rest.trim_end_matches('/').to_string()
}

/// Web results that pass verification against their fetched text, one per
/// source: a paper's title and a line of its abstract are one entry, the
/// abstract's line (it says more), in the place the first of them had. One
/// paper's addresses are one source (`source_key`).
pub fn web_hits(results: &[WebResult]) -> Vec<Hit> {
    let url_of = |h: &Hit| match &h.at {
        Locator::Web { url, .. } => source_key(url),
        Locator::Local { file, .. } => file.clone(),
    };
    let is_title = |h: &Hit| matches!(&h.at, Locator::Web { section: Some(s), .. } if s == "Title");
    let mut hits: Vec<Hit> = Vec::new();
    for h in results
        .iter()
        .filter_map(|r| verify_web(&r.url, r.section.as_deref(), &r.page_text, &r.quote))
    {
        if let Some(o) = hits.iter_mut().find(|o| url_of(o) == url_of(&h)) {
            if is_title(o) && !is_title(&h) {
                *o = h;
            }
            continue;
        }
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

    #[test]
    fn find_counts_every_matching_line_beyond_the_limit() {
        let body: String = (0..30).map(|i| format!("gate line {i}\n")).collect();
        let d = ws(&[("a.rs", &body), ("b.md", "the gate\nnothing\n")]);
        let (hits, total) = find_counted(d.path(), &["gate".to_string()], 12);
        assert_eq!((hits.len(), total), (12, 31));
        assert_eq!(find(d.path(), &["gate".to_string()], 12), hits);
    }

    #[test]
    fn an_open_reply_splits_into_path_range_and_block() {
        let d = ws(&[
            ("src/x.rs", "fn a() {}\nfn b() {}\n"),
            ("notes/one.md", "hi"),
        ]);
        let opened = open_spec(d.path(), &parse_open_spec("src/x.rs")).unwrap();
        let (title, range, body) = split_source(&opened.markdown).unwrap();
        assert_eq!(
            (title.as_str(), range.as_str()),
            ("src/x.rs", "lines 1–2 of 2")
        );
        assert!(
            body.starts_with("```rust numbered\n    1  fn a() {}"),
            "{body}"
        );
        let dir = open_spec(d.path(), &parse_open_spec("notes")).unwrap();
        let (title, range, body) = split_source(&dir.markdown).unwrap();
        assert_eq!((title.as_str(), range.as_str()), ("notes/", "1 items"));
        assert_eq!(body, "- `notes/one.md`");
        // Notices aren't sources.
        assert_eq!(split_source("No file `x` in this workspace."), None);
        assert_eq!(
            split_source("`a.png` is an image. Attach it with `/attach a.png`."),
            None
        );
    }

    #[test]
    fn find_rows_read_back_into_hits() {
        for at in [
            Locator::Local {
                file: "app/src/a b.rs".into(),
                page: None,
                line: 42,
            },
            Locator::Local {
                file: "paper.pdf".into(),
                page: Some(3),
                line: 7,
            },
        ] {
            let hit = Hit {
                at,
                quote: "  let x = `y`;".into(),
            };
            let row = format!("- `{}`  {}", hit.at, hit.quote);
            assert_eq!(parse_hit(&row), Some(hit));
        }
        assert_eq!(parse_hit("**2** for `x`:"), None);
        assert_eq!(find_total("**12/48** for `x`:\n\n- `a:1`  x"), Some(48));
        assert_eq!(find_total("**2** for `x`:"), None);
    }

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

    fn have_poppler() -> bool {
        ["pdftotext", "pdftohtml"]
            .iter()
            .all(|t| std::process::Command::new(t).arg("-v").output().is_ok())
    }

    fn pdf_ws(pdf: &[u8]) -> tempdir::Dir {
        let d = tempdir::Dir::new();
        std::fs::write(d.path().join("slides.pdf"), pdf).unwrap();
        d
    }

    #[test]
    fn symbol_math_in_private_use_codes_reads_as_greek_and_math() {
        if !have_poppler() {
            return;
        }
        use crate::pdftext::fixture;
        // Symbol drawn through a ToUnicode that maps to U+F0xx, as
        // PowerPoint's SymbolMT does.
        let pdf = fixture::pdf(
            "<< /Type /Font /Subtype /TrueType /BaseFont /SymbolMT >>",
            Some(&[(0x72, 0xF072), (0xB6, 0xF0B6)]),
            fixture::MAXWELL,
        );
        let d = pdf_ws(&pdf);
        let opened = open_spec(d.path(), &parse_open_spec("slides.pdf 1")).unwrap();
        let md = &opened.markdown;
        assert!(md.contains("div D = ρ"), "{md}");
        assert!(md.contains("∂B"), "{md}");
        assert!(md.contains("FuB 1.19"), "{md}");
        assert!(
            !md.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c)),
            "{md}"
        );
        assert!(!md.contains("didn't extract cleanly"), "{md}");
        let h = verify_local(d.path(), &hit("slides.pdf", Some(1), "div D = ρ")).unwrap();
        assert_eq!(h.quote, "div D = ρ");
    }

    #[test]
    fn wrongly_mapped_symbol_math_is_flagged_and_never_quoted() {
        if !have_poppler() {
            return;
        }
        use crate::pdftext::fixture;
        // A Symbol font whose ToUnicode says `r` and `¶` for ρ and ∂.
        let pdf = fixture::pdf(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Symbol >>",
            Some(&[(0x72, 0x72), (0xB6, 0xB6)]),
            fixture::MAXWELL,
        );
        let d = pdf_ws(&pdf);
        let opened = open_spec(d.path(), &parse_open_spec("slides.pdf 1")).unwrap();
        let (_, _, body) = split_source(&opened.markdown).unwrap();
        assert!(
            body.starts_with(&format!(
                "{}\n\n```text numbered",
                crate::pdftext::unclean_note(1)
            )),
            "{body}"
        );
        // Garbled math isn't the source's words. Poppler says which runs
        // the wrong letters are in, so prose set apart from them is.
        assert_eq!(
            unclean_pages(&d.path().join("slides.pdf"), "slides.pdf"),
            vec![crate::pdftext::PageTrust::Lettered]
        );
        assert!(matches!(
            verify_local(d.path(), &hit("slides.pdf", Some(1), "div D = r")),
            Err(Rejected::NotInSource { .. })
        ));
        let h = verify_local(
            d.path(),
            &hit("slides.pdf", Some(1), "Maxwell equations describe"),
        )
        .unwrap();
        assert_eq!(h.quote, "Maxwell equations describe");
        assert!(find(d.path(), &["div".to_string()], 5).is_empty());
        assert_eq!(find(d.path(), &["Maxwell".to_string()], 5).len(), 1);
    }

    #[test]
    fn a_symbol_mu_read_as_m_never_verifies_as_millimetres() {
        if !have_poppler() {
            return;
        }
        use crate::pdftext::fixture;
        // "25 μm", with μ from a Symbol font whose ToUnicode says `m`.
        let pdf = fixture::pdf(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Symbol >>",
            Some(&[(0x6D, 0x6D)]),
            "BT /F2 20 Tf 50 300 Td (Metallschicht 25 ) Tj /F1 20 Tf (m) Tj /F2 20 Tf (m) Tj ET",
        );
        let d = pdf_ws(&pdf);
        let opened = open_spec(d.path(), &parse_open_spec("slides.pdf 1")).unwrap();
        assert!(opened.markdown.contains("25 mm"), "{}", opened.markdown);
        assert!(
            opened.markdown.contains(&crate::pdftext::unclean_note(1)),
            "{}",
            opened.markdown
        );
        assert!(matches!(
            verify_local(d.path(), &hit("slides.pdf", Some(1), "Metallschicht 25 mm")),
            Err(Rejected::NotInSource { .. })
        ));
    }

    #[test]
    fn a_symbol_font_without_to_unicode_stays_clean() {
        if !have_poppler() {
            return;
        }
        use crate::pdftext::fixture;
        let pdf = fixture::pdf(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Symbol >>",
            None,
            fixture::MAXWELL,
        );
        let d = pdf_ws(&pdf);
        let opened = open_spec(d.path(), &parse_open_spec("slides.pdf 1")).unwrap();
        assert!(opened.markdown.contains("div D = ρ"), "{}", opened.markdown);
        assert!(!opened.markdown.contains("didn't extract cleanly"));
    }

    /// The FUB lecture's Maxwell slide, from a real PDF outside the repo:
    /// `PATHFINDER_FUB_PDF=/path/to/FUB_Folien1.pdf cargo test -- --ignored fub_slide`.
    #[test]
    #[ignore]
    fn fub_slide_19_reads_its_maxwell_equations() {
        let Some(path) = std::env::var_os("PATHFINDER_FUB_PDF") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        let dir = path.parent().unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let opened = open_spec(dir, &parse_open_spec(&format!("{name} 19"))).unwrap();
        println!("{}", opened.markdown);
        let md = &opened.markdown;
        assert!(md.contains("div D = ρ"), "{md}");
        assert!(
            md.contains("∂B") && md.contains("∂t") && md.contains("∂D"),
            "{md}"
        );
        assert!(md.contains("FuB 1.19"), "{md}");
        assert!(
            !md.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c)),
            "{md}"
        );
        let unclean = unclean_pages(&path, &name);
        assert!(!unclean[18].marked(), "page 19 is marked unclean");
        let marked: Vec<usize> = (1..=unclean.len())
            .filter(|p| unclean[p - 1].marked())
            .collect();
        println!("pages marked unclean: {marked:?}");
    }

    const NOTES: &str = "# Notes\n\nThe Laplace transform turns\ndifferential equations into algebraic ones.\nIt is linear.\n";

    fn hit(file: &str, page: Option<usize>, quote: &str) -> ProposedHit {
        ProposedHit {
            file: file.into(),
            page,
            quote: quote.into(),
        }
    }

    /// The checks for one page of the given trust, with these trusted
    /// segments.
    fn checks(trust: crate::pdftext::PageTrust, trusted: &[&str]) -> PageChecks {
        PageChecks {
            trust: vec![trust],
            trusted: vec![trusted.iter().map(|t| loose(t).0).collect()],
            folded: vec![Default::default()],
        }
    }

    #[test]
    fn a_lettered_page_quotes_only_prose_clear_of_its_wrong_letters() {
        use crate::pdftext::PageTrust;
        let page =
            "Take sources forP one and the same value.\nThe sum is finite here.\nX = a + b\n";
        let c = checks(
            PageTrust::Lettered,
            &[
                "Take sources for",
                "one and the same value.\nThe sum is finite here.\n= a + b",
            ],
        );
        assert!(c.quotable(0, page, "The sum is finite here."));
        assert!(c.quotable(0, page, "one and the same value."));
        // The CMEX `P` (∑) inside prose, and math, are never quoted.
        assert!(!c.quotable(0, page, "sources forP one"));
        assert!(!c.quotable(0, page, "X = a + b"));
        // Words a wrong letter makes elsewhere on the page aren't vouched
        // for by the same words where they are clean.
        let page = "25 mm thick\nIt is 25 mm thick\n";
        let c = checks(
            PageTrust::Lettered,
            &["25 m", "m thick\nIt is 25 mm thick\n"],
        );
        assert!(!c.quotable(0, page, "25 mm thick"));
        assert!(c.quotable(0, page, "It is 25 mm thick"));
        // Shaky: prose only; unquotable: nothing.
        let c = checks(PageTrust::Shaky, &[]);
        assert!(c.quotable(0, page, "It is 25 mm thick"));
        assert!(!checks(PageTrust::Unquotable, &[]).quotable(0, page, "It is 25 mm thick"));
    }

    #[test]
    fn math_is_cited_by_its_prose_or_the_line_that_introduces_it() {
        let lines = [
            "Theorem 2.1 (Product rule). Let f and g be",
            "smooth, then:",
            "",
            "        d",
            "  h =     (f g) = f′ g + f g′          (7)",
            "       dx",
            "where the prime marks a derivative in x.",
        ];
        let prose = |l: &str| crate::pdftext::plain_prose(l);
        // The prose part of a quote that runs into the formula.
        assert_eq!(
            prose_part(
                &lines,
                "where the prime marks a derivative in x. h = f′ g",
                prose
            ),
            Some((7, "where the prime marks a derivative in x.".to_string()))
        );
        // The formula itself, as the text has it or as LaTeX: the title of
        // the statement it is part of.
        let at = locate(&lines, "h = (f g) = f′ g + f g′").unwrap().0 - 1;
        assert_eq!(at, 4);
        assert_eq!(
            lead_in(&lines, at, prose),
            Some((1, "Theorem 2.1 (Product rule).".to_string()))
        );
        assert_eq!(
            locate_math(&lines, r"h = \frac{d}{dx}(fg) = f'g + fg'"),
            Some(4)
        );
        // Without a title, the nearest lead-in of a few words; a line that
        // only finishes a broken word isn't one.
        let lines = [
            "The distance between the two densi-",
            "ties is:",
            "  d = ∥f − g∥2",
        ];
        assert_eq!(
            lead_in(&lines, 2, prose),
            Some((1, "The distance between the two densi-".to_string()))
        );
        // Prose that isn't in the source is never placed by its letters.
        let lines = [
            "The algorithm assigns every point to its nearest centre.",
            "x = 1",
        ];
        assert_eq!(
            locate_math(&lines, "Every point is assigned to the nearest centre"),
            None
        );
        // Nor is a made-up formula, by letters it shares with the math.
        let lines = [
            "Theorem 2.1 (Product rule). Let f and g be",
            "smooth, then:",
            "        d",
            "  h =     (f g) = f′ g + f g′          (7)",
            "       dx",
            "where the prime marks a derivative in x.",
            "  a = b + c + d + e + f + g + h          (8)",
        ];
        assert_eq!(locate_math(&lines, "price = cost + margin"), None);
        assert_eq!(
            locate_math(&lines, "a = b + c + d + e + f + g + h"),
            Some(6)
        );
        assert_eq!(
            locate_math(&lines[..6], "a = b + c + d + e + f + g + h"),
            None
        );
        assert_eq!(locate_math(&lines[..6], "x = d f / d h + g g"), None);
        // A real phrase doesn't vouch for made-up words around it.
        let prose = |l: &str| crate::pdftext::plain_prose(l);
        assert_eq!(
            prose_part(
                &lines,
                "we prove = the prime marks a derivative = is always zero for every curve",
                prose
            ),
            None
        );
    }

    #[test]
    fn a_garbled_formula_on_a_pdf_page_is_cited_by_its_theorem() {
        if !have_poppler() {
            return;
        }
        use crate::pdftext::fixture;
        // ∑ from a symbol font, alone on its line: the page's math is
        // garbled, so only its prose can be quoted.
        let pdf = fixture::pdf(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            Some(&[(0x53, 0x2211)]),
            "BT /F2 14 Tf 50 340 Td (Theorem 4 \\(Geometric series\\). For every q:) Tj ET \
BT /F1 20 Tf 60 300 Td (S) Tj ET \
BT /F2 14 Tf 50 270 Td (s = a + a q + a q q = a / \\(1 - q\\)) Tj ET \
BT /F2 14 Tf 50 200 Td (The proof is a telescoping sum over the terms.) Tj ET",
        );
        let d = pdf_ws(&pdf);
        assert_eq!(
            unclean_pages(&d.path().join("slides.pdf"), "slides.pdf"),
            vec![crate::pdftext::PageTrust::Shaky]
        );
        let h = verify_local(
            d.path(),
            &hit("slides.pdf", Some(1), "s = a + a q + a q q = a / (1 - q)"),
        )
        .unwrap();
        assert_eq!(h.quote, "Theorem 4 (Geometric series).");
        assert_eq!(h.at.to_string(), "slides.pdf p.1 l.1");
        let h = verify_local(
            d.path(),
            &hit("slides.pdf", Some(1), "The proof is a telescoping sum"),
        )
        .unwrap();
        assert_eq!(h.quote, "The proof is a telescoping sum");
        // Prose the page doesn't have is still dropped.
        assert!(matches!(
            verify_local(
                d.path(),
                &hit(
                    "slides.pdf",
                    Some(1),
                    "The series converges for every ratio"
                )
            ),
            Err(Rejected::NotInSource { .. })
        ));
    }

    #[test]
    fn find_puts_the_exact_phrase_first_and_counts_lines_with_every_term() {
        let d = ws(&[(
            "paper.md",
            "Using Bayesian fusion of GMMs is easy.\nBayesian ideas.\nMore information on fusion.\nProposition 3 (Bayesian Fusion for GMMs).\nGMMs again.\n",
        )]);
        let terms: Vec<String> = ["Bayesian", "Fusion", "for", "GMMs"]
            .iter()
            .map(|t| t.to_string())
            .collect();
        let (hits, total) = find_counted(d.path(), &terms, 12);
        assert_eq!(
            hits.iter().map(|h| h.at.to_string()).collect::<Vec<_>>(),
            vec!["paper.md:4", "paper.md:1"]
        );
        assert_eq!(total, 2);
        // `find`, for code's own searches, still ranks the rest after them.
        assert_eq!(find(d.path(), &terms, 12).len(), 5);
    }

    #[test]
    fn a_paper_s_title_and_abstract_are_one_web_hit() {
        let text = "\u{1}Title\nFusing Gaussian mixtures in closed form\n\u{1}Abstract\nWe fuse Gaussian mixtures exactly.\n";
        let at = |section: &str, quote: &str| WebResult {
            url: "https://arxiv.org/abs/1".into(),
            section: Some(section.into()),
            page_text: text.into(),
            quote: quote.into(),
        };
        let hits = web_hits(&[
            at("Title", "Fusing Gaussian mixtures in closed form"),
            WebResult {
                url: "https://example.org/b".into(),
                section: None,
                page_text: "Mixtures are fused here too.".into(),
                quote: "Mixtures are fused here too.".into(),
            },
            at("Abstract", "We fuse Gaussian mixtures exactly."),
        ]);
        assert_eq!(
            hits.iter().map(|h| h.at.to_string()).collect::<Vec<_>>(),
            vec![
                "https://arxiv.org/abs/1 § Abstract",
                "https://example.org/b"
            ]
        );
        assert_eq!(hits[0].quote, "We fuse Gaussian mixtures exactly.");
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
    fn one_paper_s_addresses_are_one_source() {
        let key = source_key("https://arxiv.org/abs/2306.07309v1");
        assert_eq!(key, "arxiv:2306.07309");
        for same in [
            "https://arxiv.org/html/2306.07309",
            "https://arxiv.org/html/2306.07309v2/",
            "http://arxiv.org/pdf/2306.07309v1.pdf",
            "https://www.arxiv.org/abs/2306.07309#S2",
            "https://doi.org/10.48550/arXiv.2306.07309",
        ] {
            assert_eq!(source_key(same), key, "{same}");
        }
        assert_ne!(source_key("https://arxiv.org/abs/2306.07310"), key);
        assert_eq!(
            source_key("http://arxiv.org/abs/hep-th/9901001v3"),
            "arxiv:hep-th/9901001"
        );
        assert_eq!(
            source_key("https://doi.org/10.1109/TSP.2006.1"),
            source_key("https://dx.doi.org/10.1109/tsp.2006.1/")
        );
        assert_eq!(
            source_key("https://example.org/a/"),
            source_key("http://www.example.org/a")
        );
        assert_ne!(
            source_key("https://example.org/a"),
            source_key("https://example.org/b")
        );
    }

    #[test]
    fn an_arxiv_paper_s_abs_and_html_pages_are_one_web_hit() {
        let hits = web_hits(&[
            WebResult {
                url: "http://arxiv.org/abs/2306.07309v1".into(),
                section: Some("Abstract".into()),
                page_text: "We fuse Gaussian mixtures exactly.".into(),
                quote: "We fuse Gaussian mixtures exactly.".into(),
            },
            WebResult {
                url: "https://arxiv.org/html/2306.07309".into(),
                section: Some("Method".into()),
                page_text: "Each pair of components is multiplied.".into(),
                quote: "Each pair of components is multiplied.".into(),
            },
        ]);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(
            hits[0].at.to_string(),
            "http://arxiv.org/abs/2306.07309v1 § Abstract"
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
        assert!(
            r.markdown.contains("```markdown numbered\n"),
            "{}",
            r.markdown
        );
        assert!(
            r.markdown
                .ends_with("```\n11–40 not shown · `/open sub/notes.md:11-40`"),
            "{}",
            r.markdown
        );
        let whole = open_spec(d.path(), &parse_open_spec("sub/notes.md")).unwrap();
        assert!(whole.markdown.ends_with("```"), "{}", whole.markdown);
        let cut = open_spec(d.path(), &parse_open_spec("sub/notes.md:1-3")).unwrap();
        assert!(
            cut.markdown
                .ends_with("4–40 not shown · `/open sub/notes.md:4-40`"),
            "{}",
            cut.markdown
        );
        assert_eq!(r.target.lines, Some((5, 10)));
        assert_eq!(r.target.kind, SourceKind::Markdown);

        // Safe fence escaping when markdown contains ```.
        let fenced = open_spec(d.path(), &parse_open_spec("README.md")).unwrap();
        assert!(
            fenced.markdown.contains("````markdown numbered\n"),
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

    /// A LaTeX preprint whose big operators come out as letters (CMEX's
    /// `X` for ∑), from outside the repo:
    /// `PATHFINDER_GMM_PDF=/path/to/2603.20365v1.pdf cargo test -- --ignored gmm_preprint`.
    #[test]
    #[ignore]
    fn gmm_preprint_cites_survive_on_its_math_pages() {
        let Some(path) = std::env::var_os("PATHFINDER_GMM_PDF") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        let dir = path.parent().unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let unclean = unclean_pages(&path, &name);
        for p in [18, 27, 29] {
            assert_eq!(unclean[p - 1], crate::pdftext::PageTrust::Lettered, "p.{p}");
        }
        let cite = |page: usize, quote: &str| {
            let h = verify_local(dir, &hit(&name, Some(page), quote))
                .unwrap_or_else(|e| panic!("p.{page} {quote:?}: {e:?}"));
            let Locator::Local { page: at, .. } = &h.at else {
                unreachable!()
            };
            assert_eq!(*at, Some(page), "{quote:?} -> {h:?}");
            assert!(crate::pdftext::plain_prose(&h.quote), "{h:?}");
            println!("p.{page} {quote:?} -> {} {:?}", h.at, h.quote);
            h.quote
        };
        // Proposition B.3, p.29: its title, and its equation (44).
        let b3 = "Proposition B.3 (Bayesian Fusion for GMMs).";
        assert_eq!(cite(29, b3), b3);
        assert_eq!(
            cite(
                29,
                "Proposition B.3 (Bayesian Fusion for GMMs). G_X(x|g_a) = Σ π_ai N_X(x|m_ai, Σ_ai)"
            ),
            b3
        );
        assert_eq!(
            cite(
                29,
                "G_X(x|g_a) · G_X(x|g_b) = Σ_{i=1}^{K_a} Σ_{j=1}^{K_b} c_{a_i b_j} π_{a_i} π_{b_j} N_X(x|m_{a_i b_j}, Σ_{a_i b_j})",
            ),
            b3
        );
        // Section 6.8, p.18: its heading, a sentence with a reference, and
        // the L2 distance formula (21).
        let s68 = "6.8 Reduction of Gaussian Mixture Models";
        assert_eq!(cite(18, s68), s68);
        assert!(
            cite(
                18,
                "the L2 distance between mixtures admits a closed-form expression [18]"
            )
            .contains("[18]")
        );
        assert!(cite(18, "∥GX (x|ga ) − GX (x|gb )∥L2 (Rd ) =").contains("L2 distance"));
        // Equation (39), p.27, as the text has it, in LaTeX and in Unicode:
        // shown by the proposition it belongs to.
        let a6 = "Proposition A.6 (Bayesian Fusion of Gaussians).";
        for q in [
            "mab = (Σ−1",
            r"c_{ab} = \frac{\exp\left(-\frac{1}{2}(m_a - m_b)^\top (\Sigma_a + \Sigma_b)^{-1} (m_a - m_b)\right)}{\sqrt{\det(2\pi(\Sigma_a+\Sigma_b))}}",
            "c_ab = exp(−½(m_a − m_b)ᵀ(Σ_a + Σ_b)⁻¹(m_a − m_b)) / √det(2π(Σ_a + Σ_b))",
            "Σ_ab = (Σ_a⁻¹ + Σ_b⁻¹)⁻¹",
        ] {
            assert_eq!(cite(27, q), a6);
        }
        // A made-up formula is never placed on a page by the letters it
        // shares with the math there.
        for (page, q) in [
            (1, "price = cost + margin"),
            (9, "price = cost + margin"),
            (18, "price = cost + margin"),
            (18, "total = mass + energy"),
            (27, "kinetic_energy = half m v^2"),
            (29, "a = b + c + d + e + f + g + h"),
            (18, "D = w^T A w - 2 w^T B v + v^T C v"),
            (
                18,
                "we prove = the L2 distance between mixtures = is always zero",
            ),
        ] {
            let got = verify_local(dir, &hit(&name, Some(page), q));
            assert!(
                matches!(got, Err(Rejected::NotInSource { .. })),
                "p.{page} {q:?} -> {got:?}"
            );
        }
        // A wrong letter (CMEX's `P` for ∑ inline) is never quoted, and
        // words the paper doesn't have are still not found.
        assert!(
            !cite(
                29,
                "random generators (sources) forP one and the same quantity"
            )
            .contains("forP")
        );
        assert!(matches!(
            verify_local(
                dir,
                &hit(
                    &name,
                    Some(29),
                    "Mixtures make fusion cheap and exact for every sensor"
                )
            ),
            Err(Rejected::NotInSource { .. })
        ));
        // `/find` puts the exact heading first and counts lines with every word.
        let terms: Vec<String> = "Bayesian Fusion for GMMs"
            .split(' ')
            .map(String::from)
            .collect();
        let (hits, total) = find_counted(dir, &terms, 12);
        println!(
            "{total} lines: {:?}",
            hits.iter().map(|h| h.at.to_string()).collect::<Vec<_>>()
        );
        assert!(hits[0].quote.contains(b3), "{:?}", hits[0]);
        assert!(total < 20, "{total}");
    }
}
