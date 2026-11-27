//! The agent's tools. Names match `stream::describe_tool`, so thread entries
//! get titles and kinds for free.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use globset::GlobBuilder;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde_json::{Value, json};

use super::confine::confine;
use crate::llm::ToolSpec;

const MAX_READ_LINES: usize = 2000;
/// Under the agent's 50 KB cap on tool output, so the "continue at" note
/// at the end of a long read is never cut off.
const MAX_READ_BYTES: usize = 46 * 1024;
const MAX_LINE_CHARS: usize = 2000;
const MAX_LIST: usize = 500;
const MAX_MATCHES: usize = 200;
const MAX_GREP_FILE: u64 = 1024 * 1024;

pub struct ToolEnv<'a> {
    pub root: &'a Path,
    pub attached: &'a [PathBuf],
}

pub fn read_only_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read".into(),
            description:
                "Read a text file in the workspace (numbered lines), or a PDF's text page by page."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path, relative to the workspace."},
                    "offset": {"type": "integer", "description": "First line to read (1-based)."},
                    "limit": {"type": "integer", "description": "Number of lines to read."},
                    "pages": {"type": "string", "description": "PDF only: pages to read, e.g. `12` or `3-7` (1-based). Default: the first 20."}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "view".into(),
            description: "See pages of a PDF, or an image file, as images: for circuits, diagrams, plots, tables and handwriting that `read` can't show. At most 4 PDF pages per call; use `read` first to find the pages.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "PDF or image (PNG, JPEG, GIF, WebP) path, relative to the workspace."},
                    "pages": {"type": "string", "description": "PDF only: pages to see, e.g. `3` or `3-5` (1-based, at most 4). Default: 1."}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "ls".into(),
            description: "List files and folders in the workspace (respects .gitignore).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Folder to list; defaults to the workspace root."},
                    "depth": {"type": "integer", "description": "How many levels deep (1-3, default 2)."}
                },
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "glob".into(),
            description: "Find files whose path matches a glob pattern, e.g. `**/*.rs`.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string", "description": "Folder to search in; defaults to the workspace root."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "grep".into(),
            description:
                "Search file contents with a regular expression, PDFs included. Returns `path:line: text` matches, or `file.pdf p.N: text` for a PDF's page N (read it with `pages`)."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regular expression."},
                    "path": {"type": "string", "description": "Folder or file to search; defaults to the workspace root."},
                    "glob": {"type": "string", "description": "Only search files matching this glob, e.g. `*.rs`."},
                    "ignore_case": {"type": "boolean"}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
    ]
}

/// Read-only tools plus `edit`, `write` and `bash`, for agent work.
pub fn full_specs() -> Vec<ToolSpec> {
    let mut specs = read_only_specs();
    specs.extend([
        ToolSpec {
            name: "edit".into(),
            description: "Replace text in a workspace file. Read the file first. `old_string` must appear exactly once unless `replace_all` is true; include enough surrounding lines to make it unique. Copy it from the file without the line-number prefix `read` shows. Typographic quotes, dashes and trailing spaces are matched loosely when there is exactly one such match.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path, relative to the workspace."},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean"}
                },
                "required": ["path", "old_string", "new_string"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "write".into(),
            description: "Create a workspace file, or replace all of its content. Creates missing folders. Prefer `edit` for changes to existing files.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path, relative to the workspace."},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        },
        bash_spec(super::sandbox::available().is_some()),
    ]);
    specs
}

/// The shell `bash` commands run in, named where it isn't `sh`.
fn shell_note() -> String {
    match super::shell::current() {
        super::shell::Shell::Posix => String::new(),
        super::shell::Shell::GitBash(_) => " with Git Bash".into(),
        super::shell::Shell::PowerShell(_) => {
            " with PowerShell (write PowerShell, not POSIX sh)".into()
        }
    }
}

/// The `bash` tool. With a sandbox the model is told what it confines and
/// how to ask past it; without one (Windows, Linux without a working
/// bubblewrap, or `PATHFINDER_NO_SANDBOX`) it is told the truth: every
/// command runs with the user's full access, and the user approves each.
fn bash_spec(sandboxed: bool) -> ToolSpec {
    let shell = shell_note();
    let lead = format!(
        "Run a shell command in the workspace{shell}; the user watches it run. Returns its output and exit code. Use it for building, testing and running code; use the file tools to read and change files."
    );
    if !sandboxed {
        return ToolSpec {
            name: "bash".into(),
            description: format!(
                "{lead} There is no sandbox on this system: every command runs with the user's full access and the user approves each one. Prefer the file tools; avoid commands that change things outside the workspace."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_secs": {"type": "integer", "description": "Default 120, at most 600."}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        };
    }
    ToolSpec {
        name: "bash".into(),
        description: format!(
            "{lead} Commands run in a sandbox: they can read files, write only inside the workspace (not .git) and the temp folders, and have no network. When a command truly needs the internet (installing packages, fetching), set `network`; when it must write elsewhere or into .git (e.g. `git commit`), set `outside_sandbox`. Both need a `justification`, and the user is asked first."
        ),
        parameters: json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout_secs": {"type": "integer", "description": "Default 120, at most 600."},
                "network": {"type": "boolean", "description": "Allow network access (the user is asked)."},
                "outside_sandbox": {"type": "boolean", "description": "Run without the sandbox (the user is asked)."},
                "justification": {"type": "string", "description": "Why `network` or `outside_sandbox` is needed, in one sentence for the user."}
            },
            "required": ["command"],
            "additionalProperties": false
        }),
    }
}

/// How to install poppler (`pdftotext`, `pdftoppm`) on this system.
const POPPLER_HINT: &str = if cfg!(target_os = "macos") {
    "`brew install poppler`"
} else if cfg!(windows) {
    "`winget install oschwartz10612.Poppler` or `scoop install poppler`, then add its `bin` folder to PATH"
} else {
    "`sudo apt install poppler-utils`, or the `poppler` package on Fedora and Arch"
};

/// A file a tool changed: `old` is None for a new file.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub path: PathBuf,
    pub old: Option<String>,
    pub new: String,
}

/// The file an `edit` or `write` call targets: its path as given, and where
/// it resolves inside the workspace.
pub fn change_target(args: &Value, env: &ToolEnv<'_>) -> Result<(String, PathBuf), String> {
    let path = str_arg(args, "path").ok_or("`path` is required")?;
    Ok((path.to_string(), confine(env.root, path, true, &[])?))
}

/// The new content for `edit` or `write`, given the current one (None when
/// the file doesn't exist), and a message for the model.
pub fn apply_change(
    name: &str,
    args: &Value,
    path: &str,
    old: Option<&str>,
) -> Result<(String, String), String> {
    let mut note = None;
    let new = match name {
        "write" => args
            .get("content")
            .and_then(Value::as_str)
            .ok_or("`content` is required")?
            .to_string(),
        "edit" => {
            let text =
                old.ok_or_else(|| format!("{path} doesn't exist; use `write` to create it"))?;
            let from = args
                .get("old_string")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or(
                    "`old_string` is required; to create or replace a whole file, use `write`",
                )?;
            let to = args.get("new_string").and_then(Value::as_str).unwrap_or("");
            let all = args
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if from == to {
                return Err("`old_string` and `new_string` are the same; nothing to change".into());
            }
            let edited = edit_text(text, from, to, all, path)?;
            note = edited.1;
            edited.0
        }
        other => return Err(format!("`{other}` doesn't change files")),
    };
    let lines = new.lines().count();
    let message = match (old, name) {
        (None, _) => format!("Created {path} ({lines} lines)."),
        (Some(_), "write") => format!("Replaced {path} ({lines} lines)."),
        _ => match note {
            Some(how) => format!("Edited {path} ({how}; check the result)."),
            None => format!("Edited {path}."),
        },
    };
    Ok((message, new))
}

/// Typographic characters PDFs and word processors put in text, mapped to
/// what a model types.
fn plain_char(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' | '\u{00AB}'
        | '\u{00BB}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        c => c,
    }
}

fn plain(s: &str) -> String {
    s.chars().map(plain_char).collect()
}

/// Byte ranges in `text` where `from` matches: exactly, else with
/// typographic characters made plain, else line by line ignoring trailing
/// whitespace. Returns the ranges and how they matched (None: exactly).
fn find_matches(text: &str, from: &str) -> (Vec<std::ops::Range<usize>>, Option<&'static str>) {
    let exact: Vec<_> = text
        .match_indices(from)
        .map(|(i, m)| i..i + m.len())
        .collect();
    if !exact.is_empty() {
        return (exact, None);
    }
    // Same characters one to one, so char positions carry over.
    let starts: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect();
    let flat: Vec<char> = text.chars().map(plain_char).collect();
    let want: Vec<char> = from.chars().map(plain_char).collect();
    if !want.is_empty() && want.len() <= flat.len() {
        let hits: Vec<_> = (0..=flat.len() - want.len())
            .filter(|&i| flat[i..i + want.len()] == want[..])
            .map(|i| starts[i]..starts[i + want.len()])
            .collect();
        if !hits.is_empty() {
            return (
                hits,
                Some("matched after making quotes, dashes and spaces plain"),
            );
        }
    }
    // Whole lines, ignoring trailing whitespace.
    let want: Vec<String> = from
        .trim_end_matches('\n')
        .lines()
        .map(|l| plain(l.trim_end()))
        .collect();
    let mut lines = Vec::new();
    let mut at = 0;
    for l in text.split_inclusive('\n') {
        lines.push((at, l));
        at += l.len();
    }
    let mut hits = Vec::new();
    if !want.is_empty() {
        for i in 0..lines.len().saturating_sub(want.len() - 1) {
            let window = &lines[i..i + want.len()];
            if window
                .iter()
                .zip(&want)
                .all(|((_, l), w)| plain(l.trim_end()) == *w)
            {
                let (start, _) = window[0];
                let (last_at, last) = window[want.len() - 1];
                // Keep the last line's newline unless `old_string` had one.
                let end = last_at + last.trim_end_matches(['\n', '\r']).len();
                let end = if from.ends_with('\n') {
                    last_at + last.len()
                } else {
                    end
                };
                hits.push(start..end);
            }
        }
    }
    (hits, Some("matched ignoring trailing whitespace"))
}

/// The lines of `text` closest to `from`, numbered like `read`, to show
/// the model where its edit missed.
fn closest_lines(text: &str, from: &str) -> Option<String> {
    let want: Vec<String> = from
        .lines()
        .map(|l| plain(l.trim()))
        .filter(|l| !l.is_empty())
        .collect();
    let lines: Vec<&str> = text.lines().collect();
    if want.is_empty() || lines.is_empty() {
        return None;
    }
    let score = |i: usize| {
        want.iter()
            .filter(|w| {
                lines[i..(i + want.len() + 2).min(lines.len())]
                    .iter()
                    .any(|l| plain(l.trim()) == **w)
            })
            .count()
    };
    let (best, hits) = (0..lines.len())
        .map(|i| (i, score(i)))
        .max_by_key(|&(i, s)| (s, usize::MAX - i))?;
    if hits == 0 {
        return None;
    }
    let first = best.saturating_sub(2);
    let mut out = String::new();
    for (n, l) in lines.iter().enumerate().skip(first).take(12) {
        let _ = writeln!(out, "{:>6}\t{l}", n + 1);
    }
    Some(out)
}

/// Applies one `edit` to `text`: the new text, and a note when the match
/// was loose. Line endings follow the file.
fn edit_text(
    text: &str,
    from: &str,
    to: &str,
    all: bool,
    path: &str,
) -> Result<(String, Option<String>), String> {
    let crlf = text.contains("\r\n");
    let view = if crlf {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    };
    let from = from.replace("\r\n", "\n");
    let to = to.replace("\r\n", "\n");
    let (hits, how) = find_matches(&view, &from);
    match hits.len() {
        0 => {
            if !to.trim().is_empty() && view.contains(to.as_str()) {
                return Err(format!(
                    "`old_string` was not found in {path}, but `new_string` is already there: the edit may already be applied. Read the file to check."
                ));
            }
            let mut msg = format!(
                "`old_string` was not found in {path}. Read the file and copy the text exactly, without the line-number prefix."
            );
            if let Some(near) = closest_lines(&view, &from) {
                let _ = write!(msg, " The closest lines:\n{near}");
            }
            return Err(msg);
        }
        n if n > 1 && !all => {
            return Err(format!(
                "`old_string` appears {n} times in {path}; add surrounding lines to make it unique, or set `replace_all`"
            ));
        }
        n if n > 1 && how.is_some() => {
            return Err(format!(
                "`old_string` only matches loosely, {n} times in {path}; copy it exactly from the file"
            ));
        }
        _ => {}
    }
    let mut out = String::with_capacity(view.len() + to.len());
    let mut at = 0;
    for r in &hits {
        out.push_str(&view[at..r.start]);
        out.push_str(&to);
        at = r.end;
    }
    out.push_str(&view[at..]);
    if crlf {
        out = out.replace('\n', "\r\n");
    }
    Ok((out, how.map(str::to_string)))
}

/// Reads a file to change; None when it doesn't exist yet.
pub fn read_existing(full: &Path, path: &str) -> Result<Option<String>, String> {
    match fs::read_to_string(full) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("can't read {path}: {e}")),
    }
}

/// Writes a changed file here, creating missing folders.
pub fn write_here(full: &Path, path: &str, content: &str) -> Result<(), String> {
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("can't create folders for {path}: {e}"))?;
    }
    fs::write(full, content).map_err(|e| format!("can't write {path}: {e}"))
}

/// Runs `edit` or `write` on the local file system.
#[cfg(test)]
pub fn change_file(
    name: &str,
    args: &Value,
    env: &ToolEnv<'_>,
) -> Result<(String, Change), String> {
    let (path, full) = change_target(args, env)?;
    let old = read_existing(&full, &path)?;
    let (message, new) = apply_change(name, args, &path, old.as_deref())?;
    write_here(&full, &path, &new)?;
    Ok((
        message,
        Change {
            path: full,
            old,
            new,
        },
    ))
}

/// Runs a tool. Errors are returned as text for the model to read.
pub fn run(name: &str, args: &Value, env: &ToolEnv<'_>) -> Result<String, String> {
    match name {
        "read" => read(args, env),
        "ls" => ls(args, env),
        "glob" => glob(args, env),
        "grep" => grep(args, env),
        "edit" | "write" | "bash" => Err(format!(
            "`{name}` isn't available in this step, which only reads; answer with what you found"
        )),
        other => {
            let known = [
                "read", "view", "ls", "glob", "grep", "edit", "write", "bash",
            ];
            let near = known
                .iter()
                .find(|k| other.to_lowercase().contains(*k) || k.contains(&other.to_lowercase()));
            Err(match near {
                Some(k) => format!("unknown tool `{other}`; did you mean `{k}`?"),
                None => format!("unknown tool `{other}`; the tools are {}", known.join(", ")),
            })
        }
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// `path` relative to `root`, as the model sees it: always with `/`, so
/// Windows paths read like everywhere else (and `confine` takes `/` back).
fn rel(root: &Path, path: &Path) -> String {
    let root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let shown = path.strip_prefix(&root).unwrap_or(path).to_string_lossy();
    if cfg!(windows) {
        shown.replace('\\', "/")
    } else {
        shown.into_owned()
    }
}

/// Pages `view` renders at most per call: images are costly.
const VIEW_MAX_PAGES: usize = 4;

/// `view`: a PDF's pages rendered as PNGs (`pdftoppm`), or an image file,
/// for the model to see. Returns a line describing them and the images.
pub fn view(
    args: &Value,
    env: &ToolEnv<'_>,
) -> Result<(String, Vec<crate::llm::Attachment>), String> {
    use base64::Engine as _;
    let path = str_arg(args, "path").ok_or("`path` is required")?;
    let file = confine(env.root, path, false, env.attached)?;
    if !file.exists() {
        return Err(not_found(path, env));
    }
    let encode = |bytes: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
    match crate::llm::attachable_type(&file) {
        Some("application/pdf") => {}
        Some(media_type) => {
            let bytes = fs::read(&file).map_err(|e| format!("cannot read `{path}`: {e}"))?;
            return Ok((
                format!("`{path}`, attached as an image."),
                vec![crate::llm::Attachment {
                    name: path.to_string(),
                    media_type: media_type.into(),
                    data: encode(bytes),
                }],
            ));
        }
        None => return Err(format!("`{path}` isn't a PDF or an image; use `read`")),
    }
    let spec = str_arg(args, "pages").unwrap_or("1");
    let (a, b) = spec.split_once('-').unwrap_or((spec, spec));
    let first: usize = a
        .trim()
        .parse()
        .map_err(|_| format!("bad `pages`: `{spec}`"))?;
    let last: usize = b
        .trim()
        .parse()
        .map_err(|_| format!("bad `pages`: `{spec}`"))?;
    let first = first.max(1);
    if last < first {
        return Err(format!("bad `pages`: `{spec}`"));
    }
    let last = last.min(first + VIEW_MAX_PAGES - 1);
    let dir =
        std::env::temp_dir().join(format!("pathfinder-view-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("pdftoppm")
        .args(["-png", "-r", "110", "-f", &first.to_string(), "-l", &last.to_string()])
        .arg(&file)
        .arg(dir.join("page"))
        .output()
        .map_err(|_| {
            format!(
                "can't render `{path}`: `pdftoppm` isn't installed ({POPPLER_HINT}). Ask the user to attach a screenshot instead."
            )
        })?;
    let mut pages: Vec<PathBuf> = fs::read_dir(&dir)
        .map(|it| it.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    pages.sort();
    let images: Vec<crate::llm::Attachment> = pages
        .iter()
        .filter_map(|p| fs::read(p).ok())
        .zip(first..)
        .map(|(bytes, n)| crate::llm::Attachment {
            name: format!("{path} page {n}"),
            media_type: "image/png".into(),
            data: encode(bytes),
        })
        .collect();
    fs::remove_dir_all(&dir).ok();
    if images.is_empty() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "`{path}` has no pages {first}–{last}{}",
            if why.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", why.trim())
            }
        ));
    }
    let shown = first + images.len() - 1;
    Ok((
        format!("`{path}`, pages {first}–{shown}, attached as images."),
        images,
    ))
}

/// A PDF's text, one entry per page, through `pdftotext` (poppler); cached
/// by path and modification time, since grep and read come back to the
/// same slides often.
pub(crate) fn pdf_pages(file: &Path, path: &str) -> Result<std::sync::Arc<Vec<String>>, String> {
    pdf_text(file, path).map(|t| t.pages.clone())
}

/// A PDF's text by page, with the pages whose math didn't extract cleanly
/// (see [`crate::pdftext`]).
pub(crate) struct PdfText {
    pub pages: std::sync::Arc<Vec<String>>,
    /// One per page: how far its text can be trusted.
    pub unclean: Vec<crate::pdftext::PageTrust>,
}

/// [`pdf_pages`], with each page's reliability.
pub(crate) fn pdf_text(file: &Path, path: &str) -> Result<std::sync::Arc<PdfText>, String> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    type Cache = Mutex<HashMap<PathBuf, (std::time::SystemTime, Arc<PdfText>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mtime = fs::metadata(file)
        .and_then(|m| m.modified())
        .map_err(|e| format!("cannot read `{path}`: {e}"))?;
    if let Some((t, text)) = cache.lock().unwrap().get(file)
        && *t == mtime
    {
        return Ok(text.clone());
    }
    // A size limit and a time limit: one odd PDF can't stall a turn.
    if fs::metadata(file).map(|m| m.len()).unwrap_or(0) > PDF_MAX_BYTES {
        return Err(format!("the PDF `{path}` is too large to read here"));
    }
    let out = run_poppler(
        "pdftotext",
        &["-layout", "-enc", "UTF-8"],
        file,
        &["-"],
        path,
    )?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pages: Vec<String> = text.split('\u{c}').map(str::to_string).collect();
    if pages.last().is_some_and(|p| p.trim().is_empty()) {
        pages.pop();
    }
    let unclean = repair_math(file, path, &mut pages);
    let text = Arc::new(PdfText {
        pages: Arc::new(pages),
        unclean,
    });
    let mut cache = cache.lock().unwrap();
    // Bounded: past the limit, start over rather than grow.
    let held: usize = cache
        .values()
        .map(|(_, t)| t.pages.iter().map(String::len).sum::<usize>())
        .sum();
    if cache.len() >= PDF_CACHE_FILES || held > PDF_CACHE_BYTES {
        cache.clear();
    }
    cache.insert(file.to_path_buf(), (mtime, text.clone()));
    Ok(text)
}

/// Symbol and math fonts: when the text has private-use symbol codes, or
/// `pdffonts` lists such a font, asks `pdftohtml -xml` which font drew what,
/// maps the codes back (see [`crate::pdftext`]) and says which pages are
/// still unreliable. Without poppler's help nothing is mapped, and pages
/// with leftover codes are marked.
fn repair_math(file: &Path, path: &str, pages: &mut [String]) -> Vec<crate::pdftext::PageTrust> {
    use crate::pdftext;
    let wanted = pdftext::has_private(pages)
        || run_poppler("pdffonts", &[], file, &[], path)
            .ok()
            .filter(|o| o.status.success())
            .is_some_and(|o| pdftext::lists_math_fonts(&String::from_utf8_lossy(&o.stdout)));
    let fonts = if wanted {
        run_poppler(
            "pdftohtml",
            &["-xml", "-i", "-q", "-stdout"],
            file,
            &[],
            path,
        )
        .ok()
        .filter(|o| o.status.success())
        .map(|o| pdftext::parse_xml(&String::from_utf8_lossy(&o.stdout)))
    } else {
        None
    };
    pdftext::repair(pages, fonts.as_ref())
}

/// Largest PDF read, how long `pdftotext` may take, and how much text the
/// PDF cache keeps.
const PDF_MAX_BYTES: u64 = 100 * 1024 * 1024;
const PDF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const PDF_CACHE_FILES: usize = 64;
const PDF_CACHE_BYTES: usize = 128 * 1024 * 1024;

/// A poppler tool (`pdftotext`, `pdffonts`, `pdftohtml`) on `file`, its
/// options before the file and `after` it, stopped after [`PDF_TIMEOUT`].
fn run_poppler(
    tool: &str,
    options: &[&str],
    file: &Path,
    after: &[&str],
    path: &str,
) -> Result<std::process::Output, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(tool)
        .args(options)
        .arg(file)
        .args(after)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| {
            format!(
                "can't read the PDF `{path}`: `{tool}` isn't installed ({POPPLER_HINT}). Ask the user to attach it instead."
            )
        })?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b);
        b
    });
    let err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b);
        b
    });
    let deadline = std::time::Instant::now() + PDF_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("reading the PDF `{path}` took too long"));
            }
            Err(e) => return Err(format!("can't read the PDF `{path}`: {e}")),
        }
    };
    Ok(std::process::Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// Pages a PDF read returns when none are asked for.
const PDF_DEFAULT_PAGES: usize = 20;

/// A PDF's text by page, through `pdftotext` (poppler). Slides and scripts
/// are long, so the reader asks for pages.
fn read_pdf(file: &Path, path: &str, pages: Option<&str>) -> Result<String, String> {
    let text = pdf_text(file, path)?;
    let all = &text.pages;
    let total = all.len();
    let (first, last) = match pages {
        Some(spec) => {
            let (a, b) = spec.split_once('-').unwrap_or((spec, spec));
            let a: usize = a
                .trim()
                .parse()
                .map_err(|_| format!("bad `pages`: `{spec}`"))?;
            let b: usize = b
                .trim()
                .parse()
                .map_err(|_| format!("bad `pages`: `{spec}`"))?;
            (a.max(1), b.min(total))
        }
        None => (1, total.min(PDF_DEFAULT_PAGES)),
    };
    if first > last {
        return Err(format!("`{path}` has {total} pages"));
    }
    let mut s = format!("`{path}`: {total} pages, showing {first}–{last}.\n");
    for (i, page) in all.iter().enumerate().take(last).skip(first - 1) {
        // The model can look at the page itself when its math is garbled.
        let note = if text.unclean.get(i).is_some_and(|t| t.marked()) {
            " (math didn't extract cleanly: see it with `view`)"
        } else {
            ""
        };
        let block = format!("\n--- page {}{note} ---\n{}\n", i + 1, page.trim_end());
        if s.len() + block.len() > MAX_READ_BYTES {
            let _ = writeln!(
                s,
                "\n... (stopped at page {i} to stay under {MAX_READ_BYTES} bytes; continue with pages {}-{last})",
                i + 1
            );
            return Ok(s);
        }
        s.push_str(&block);
    }
    if last < total {
        let _ = writeln!(
            s,
            "\n... ({} more pages; read them with `pages`)",
            total - last
        );
    }
    Ok(s)
}

/// "No such file", with the workspace's files of a similar name.
fn not_found(path: &str, env: &ToolEnv<'_>) -> String {
    let want = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let stem = want
        .rsplit_once('.')
        .map_or(want.as_str(), |(s, _)| s)
        .to_string();
    let mut near: Vec<(u8, String)> = Vec::new();
    for entry in WalkBuilder::new(env.root).build().flatten().take(20_000) {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let rank = if name == want {
            0
        } else if stem.len() >= 3 && name.contains(&stem) {
            1
        } else if name.len() >= 4
            && want.contains(name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s))
        {
            2
        } else {
            continue;
        };
        near.push((rank, rel(env.root, entry.path())));
    }
    near.sort();
    near.dedup();
    let mut msg = format!("`{path}` doesn't exist.");
    if near.is_empty() {
        msg.push_str(" Use `glob` or `ls` to find the file.");
    } else {
        let names: Vec<String> = near
            .into_iter()
            .take(4)
            .map(|(_, p)| format!("`{p}`"))
            .collect();
        let _ = write!(msg, " Did you mean {}?", names.join(", "));
    }
    msg
}

fn read(args: &Value, env: &ToolEnv<'_>) -> Result<String, String> {
    let path = str_arg(args, "path").ok_or("`path` is required")?;
    let file = confine(env.root, path, false, env.attached)?;
    if !file.exists() {
        return Err(not_found(path, env));
    }
    if file.is_dir() {
        return Err(format!("`{path}` is a folder; use `ls` to list it"));
    }
    if file
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        return read_pdf(&file, path, str_arg(args, "pages"));
    }
    let bytes = fs::read(&file).map_err(|e| format!("cannot read `{path}`: {e}"))?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err(format!("`{path}` looks like a binary file"));
    }
    let text = String::from_utf8_lossy(&bytes);
    let offset = args
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|l| l as usize)
        .unwrap_or(MAX_READ_LINES)
        .min(MAX_READ_LINES);
    let total = text.lines().count();
    if total > 0 && offset > total {
        return Err(format!(
            "offset {offset} is past the end: `{path}` has {total} lines"
        ));
    }
    let mut out = String::new();
    for (i, line) in text.lines().enumerate().skip(offset - 1).take(limit) {
        let line: std::borrow::Cow<str> = if line.chars().count() > MAX_LINE_CHARS {
            let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
            format!("{cut} … (line cut at {MAX_LINE_CHARS} characters)").into()
        } else {
            line.into()
        };
        if out.len() + line.len() > MAX_READ_BYTES {
            let _ = writeln!(
                out,
                "... (stopped to stay under {} KB; continue with offset {})",
                MAX_READ_BYTES / 1024,
                i + 1
            );
            return Ok(out);
        }
        let _ = writeln!(out, "{:>6}\t{line}", i + 1);
    }
    if offset - 1 + limit < total {
        let _ = writeln!(
            out,
            "... ({} more lines; continue with offset {})",
            total - (offset - 1 + limit),
            offset + limit
        );
    }
    if out.is_empty() {
        out = "(empty file)".to_string();
    }
    Ok(out)
}

fn ls(args: &Value, env: &ToolEnv<'_>) -> Result<String, String> {
    let dir = confine(env.root, str_arg(args, "path").unwrap_or("."), false, &[])?;
    if !dir.is_dir() {
        return Err("not a folder".to_string());
    }
    let depth = args
        .get("depth")
        .and_then(Value::as_u64)
        .unwrap_or(2)
        .clamp(1, 3) as usize;
    let mut out = Vec::new();
    let walk = WalkBuilder::new(&dir)
        .max_depth(Some(depth))
        .sort_by_file_name(Ord::cmp)
        .build();
    for entry in walk.flatten().skip(1) {
        if out.len() >= MAX_LIST {
            out.push(format!("... (stopped at {MAX_LIST} entries)"));
            break;
        }
        let suffix = if entry.path().is_dir() { "/" } else { "" };
        out.push(format!("{}{suffix}", rel(env.root, entry.path())));
    }
    Ok(if out.is_empty() {
        "(empty folder)".to_string()
    } else {
        out.join("\n")
    })
}

fn glob(args: &Value, env: &ToolEnv<'_>) -> Result<String, String> {
    let pattern = str_arg(args, "pattern").ok_or("`pattern` is required")?;
    let matcher = GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()
        .map_err(|e| format!("bad glob: {e}"))?
        .compile_matcher();
    let base = confine(env.root, str_arg(args, "path").unwrap_or("."), false, &[])?;
    let mut hits = Vec::new();
    for entry in WalkBuilder::new(&base)
        .sort_by_file_name(Ord::cmp)
        .build()
        .flatten()
    {
        if !entry.path().is_file() {
            continue;
        }
        let r = entry.path().strip_prefix(&base).unwrap_or(entry.path());
        if matcher.is_match(r) {
            hits.push(rel(env.root, entry.path()));
            if hits.len() >= MAX_LIST {
                hits.push(format!("... (stopped at {MAX_LIST} files)"));
                break;
            }
        }
    }
    Ok(if hits.is_empty() {
        "No files match.".to_string()
    } else {
        hits.join("\n")
    })
}

fn grep(args: &Value, env: &ToolEnv<'_>) -> Result<String, String> {
    let pattern = str_arg(args, "pattern").ok_or("`pattern` is required")?;
    let re = RegexBuilder::new(pattern)
        .case_insensitive(
            args.get("ignore_case")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )
        .build()
        .map_err(|e| format!("bad regex: {e}"))?;
    let only = match str_arg(args, "glob") {
        Some(g) => Some(
            GlobBuilder::new(g)
                .build()
                .map_err(|e| format!("bad glob: {e}"))?
                .compile_matcher(),
        ),
        None => None,
    };
    let base = confine(env.root, str_arg(args, "path").unwrap_or("."), false, &[])?;
    let mut hits = Vec::new();
    let mut skipped = 0;
    'files: for entry in WalkBuilder::new(&base)
        .sort_by_file_name(Ord::cmp)
        .build()
        .flatten()
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_pdf = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        if !is_pdf && entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_GREP_FILE {
            skipped += 1;
            continue;
        }
        if let Some(m) = &only {
            let relative = path.strip_prefix(&base).unwrap_or(path);
            if !m.is_match(path.file_name().unwrap_or_default()) && !m.is_match(relative) {
                continue;
            }
        }
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            let shown = rel(env.root, path);
            let Ok(pages) = pdf_pages(path, &shown) else {
                continue;
            };
            for (n, page) in pages.iter().enumerate() {
                for line in page.lines().filter(|l| re.is_match(l)) {
                    let line: String = line.trim().chars().take(300).collect();
                    hits.push(format!("{shown} p.{}: {line}", n + 1));
                    if hits.len() >= MAX_MATCHES {
                        hits.push(format!(
                            "... (stopped at {MAX_MATCHES} matches; narrow the pattern or `path`)"
                        ));
                        break 'files;
                    }
                }
            }
            continue;
        }
        let Ok(bytes) = fs::read(path) else { continue };
        if bytes.iter().take(8192).any(|b| *b == 0) {
            continue;
        }
        for (i, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
            if re.is_match(line) {
                let line: String = line.chars().take(300).collect();
                hits.push(format!(
                    "{}:{}: {}",
                    rel(env.root, path),
                    i + 1,
                    line.trim_end()
                ));
                if hits.len() >= MAX_MATCHES {
                    hits.push(format!(
                        "... (stopped at {MAX_MATCHES} matches; narrow the pattern or `path`)"
                    ));
                    break 'files;
                }
            }
        }
    }
    let mut out = if hits.is_empty() {
        "No matches.".to_string()
    } else {
        hits.join("\n")
    };
    if skipped > 0 {
        let _ = write!(out, "\n({skipped} files over 1 MB were not searched)");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_says_whether_commands_are_confined() {
        let jailed = bash_spec(true);
        assert!(jailed.description.contains("Commands run in a sandbox"));
        assert!(jailed.parameters["properties"]["outside_sandbox"].is_object());
        let open = bash_spec(false);
        assert!(
            open.description
                .contains("There is no sandbox on this system")
        );
        assert!(!open.description.contains("Commands run in a sandbox"));
        for k in ["network", "outside_sandbox", "justification"] {
            assert!(open.parameters["properties"].get(k).is_none(), "{k}");
        }
    }

    fn workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-tools-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        fs::write(
            dir.join("src/main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        fs::write(dir.join("blob.bin"), [0u8, 1, 2]).unwrap();
        dunce::canonicalize(dir).unwrap()
    }

    #[test]
    fn read_only_tools_work_and_stay_inside() {
        let ws = workspace();
        let env = ToolEnv {
            root: &ws,
            attached: &[],
        };
        let out = run(
            "read",
            &json!({"path": "src/main.rs", "offset": 2, "limit": 1}),
            &env,
        )
        .unwrap();
        assert!(out.starts_with("     2\t    println!"));
        assert!(out.contains("1 more lines"));
        assert!(
            run("read", &json!({"path": "blob.bin"}), &env)
                .unwrap_err()
                .contains("binary")
        );
        assert!(run("read", &json!({"path": "../x"}), &env).is_err());

        let listing = run("ls", &json!({}), &env).unwrap();
        assert!(listing.contains("src/\n") || listing.contains("src/"));
        assert!(listing.contains("src/main.rs"));

        assert_eq!(
            run("glob", &json!({"pattern": "**/*.rs"}), &env).unwrap(),
            "src/main.rs"
        );
        assert_eq!(
            run(
                "grep",
                &json!({"pattern": "NAME", "ignore_case": true}),
                &env
            )
            .unwrap(),
            "Cargo.toml:2: name = \"demo\""
        );
        assert_eq!(
            run("grep", &json!({"pattern": "fn", "glob": "*.rs"}), &env).unwrap(),
            "src/main.rs:1: fn main() {"
        );
        assert!(run("bash", &json!({}), &env).is_err());
        fs::remove_dir_all(ws).unwrap();
    }

    #[test]
    fn pdfs_are_read_page_by_page() {
        if std::process::Command::new("pdftotext")
            .arg("-v")
            .output()
            .is_err()
        {
            return; // poppler isn't installed here
        }
        let ws = workspace();
        fs::copy(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/two-pages.pdf"),
            ws.join("slides.pdf"),
        )
        .unwrap();
        let env = ToolEnv {
            root: &ws,
            attached: &[],
        };
        let all = run("read", &json!({"path": "slides.pdf"}), &env).unwrap();
        assert!(all.contains("2 pages, showing 1–2"));
        assert!(all.contains("--- page 1 ---\nSeite eins"));
        assert!(all.contains("--- page 2 ---\nSeite zwei: Blindleistung"));
        let second = run("read", &json!({"path": "slides.pdf", "pages": "2"}), &env).unwrap();
        assert!(!second.contains("Seite eins"));
        assert!(second.contains("Seite zwei"));
        assert!(run("read", &json!({"path": "slides.pdf", "pages": "5-6"}), &env).is_err());
    }

    #[test]
    fn view_renders_pdf_pages_as_images() {
        if std::process::Command::new("pdftoppm")
            .arg("-v")
            .output()
            .is_err()
        {
            return; // poppler isn't installed here
        }
        let ws = workspace();
        fs::copy(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/two-pages.pdf"),
            ws.join("slides.pdf"),
        )
        .unwrap();
        let env = ToolEnv {
            root: &ws,
            attached: &[],
        };
        let (text, images) = view(&json!({"path": "slides.pdf", "pages": "2-9"}), &env).unwrap();
        assert_eq!(text, "`slides.pdf`, pages 2–2, attached as images.");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].media_type, "image/png");
        assert!(images[0].data.starts_with("iVBOR"), "a PNG");
        assert!(view(&json!({"path": "slides.pdf", "pages": "7"}), &env).is_err());
        assert!(view(&json!({"path": "src/main.rs"}), &env).is_err());
    }

    #[test]
    fn edit_and_write_change_files_inside_only() {
        let root = workspace();
        let env = ToolEnv {
            root: &root,
            attached: &[],
        };
        let (msg, change) = change_file(
            "write",
            &json!({"path": "notes/a.txt", "content": "one\ntwo\n"}),
            &env,
        )
        .unwrap();
        assert!(msg.starts_with("Created notes/a.txt"));
        assert_eq!(change.old, None);
        let (_, change) = change_file(
            "edit",
            &json!({"path": "notes/a.txt", "old_string": "two", "new_string": "2"}),
            &env,
        )
        .unwrap();
        assert_eq!(change.old.as_deref(), Some("one\ntwo\n"));
        assert_eq!(
            fs::read_to_string(root.join("notes/a.txt")).unwrap(),
            "one\n2\n"
        );
        // Not found, ambiguous, outside the workspace, inside .git.
        assert!(
            change_file(
                "edit",
                &json!({"path": "notes/a.txt", "old_string": "zzz", "new_string": ""}),
                &env
            )
            .unwrap_err()
            .contains("not found")
        );
        fs::write(root.join("dup.txt"), "x x").unwrap();
        assert!(
            change_file(
                "edit",
                &json!({"path": "dup.txt", "old_string": "x", "new_string": "y"}),
                &env
            )
            .unwrap_err()
            .contains("2 times")
        );
        assert!(
            change_file(
                "write",
                &json!({"path": "../out.txt", "content": "x"}),
                &env
            )
            .is_err()
        );
        assert!(
            change_file(
                "write",
                &json!({"path": ".git/hooks/pre-commit", "content": "x"}),
                &env
            )
            .is_err()
        );
    }

    #[test]
    fn edits_match_typography_and_crlf_and_explain_misses() {
        let text = "Sei \u{201E}U\u{201C} die Spannung \u{2013} in V.\r\nZeile zwei  \r\nEnde\r\n";
        // Plain quotes and dash in old_string still find the typographic ones.
        let (out, note) = edit_text(
            text,
            "Sei \"U\" die Spannung - in V.",
            "Sei U die Spannung in V.",
            false,
            "f",
        )
        .unwrap();
        assert!(out.starts_with("Sei U die Spannung in V.\r\n"), "{out:?}");
        assert!(note.is_some());
        // Multi-line old_string with LF and no trailing spaces, on a CRLF file.
        let (out, _) = edit_text(text, "Zeile zwei\nEnde", "Zwei\nSchluss", false, "f").unwrap();
        assert!(out.ends_with("Zwei\r\nSchluss\r\n"), "{out:?}");
        // A miss shows the closest lines; an applied edit is recognised.
        let err = edit_text(
            "a\nb\nfn main() {\n}\n",
            "fn  main() {\nfoo",
            "x",
            false,
            "f",
        )
        .unwrap_err();
        assert!(
            err.contains("closest lines") || err.contains("not found"),
            "{err}"
        );
        let err = edit_text("let x = 2;\n", "let x = 1;", "let x = 2;", false, "f").unwrap_err();
        assert!(err.contains("already"), "{err}");
    }

    #[test]
    fn missing_files_suggest_near_names_and_reads_stop_with_an_offset() {
        let ws = workspace();
        let env = ToolEnv {
            root: &ws,
            attached: &[],
        };
        let err = read(&json!({"path": "main.rs"}), &env).unwrap_err();
        assert!(err.contains("src/main.rs"), "{err}");
        let err = read(&json!({"path": "Cargo.toml", "offset": 99}), &env).unwrap_err();
        assert!(err.contains("past the end"), "{err}");
        let err = run("edit", &json!({}), &env).unwrap_err();
        assert!(err.contains("only reads"), "{err}");
        // A relative glob filter on grep matches paths under the search root.
        let out = grep(&json!({"pattern": "println", "glob": "src/**/*.rs"}), &env).unwrap();
        assert!(out.contains("src/main.rs:2"), "{out}");
        fs::remove_dir_all(ws).unwrap();
    }
}
