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
const MAX_READ_BYTES: usize = 256 * 1024;
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
                "Search file contents with a regular expression. Returns `path:line: text` matches."
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
            description: "Replace exact text in a workspace file. `old_string` must appear exactly once unless `replace_all` is true; include enough surrounding lines to make it unique. Read the file first.".into(),
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
        ToolSpec {
            name: "bash".into(),
            description: "Run a shell command in the workspace. The user is asked first and watches it run. Returns its output and exit code. Use it for building, testing, installing and git; use the file tools to read and change files.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_secs": {"type": "integer", "description": "Default 120, at most 600."}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        },
    ]);
    specs
}

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
                .ok_or("`old_string` is required")?;
            let to = args.get("new_string").and_then(Value::as_str).unwrap_or("");
            let all = args
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match text.matches(from).count() {
                0 => {
                    return Err(format!(
                        "`old_string` was not found in {path}; read the file and copy the text exactly"
                    ));
                }
                n if n > 1 && !all => {
                    return Err(format!(
                        "`old_string` appears {n} times in {path}; add surrounding lines to make it unique, or set `replace_all`"
                    ));
                }
                _ => {}
            }
            if all {
                text.replace(from, to)
            } else {
                text.replacen(from, to, 1)
            }
        }
        other => return Err(format!("`{other}` doesn't change files")),
    };
    let lines = new.lines().count();
    let message = match (old, name) {
        (None, _) => format!("Created {path} ({lines} lines)."),
        (Some(_), "write") => format!("Replaced {path} ({lines} lines)."),
        _ => format!("Edited {path}."),
    };
    Ok((message, new))
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
        other => Err(format!("unknown tool `{other}`")),
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn rel(root: &Path, path: &Path) -> String {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    path.strip_prefix(&root)
        .unwrap_or(path)
        .display()
        .to_string()
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
                "can't render `{path}`: `pdftoppm` isn't installed (`brew install poppler`). Ask the user to attach a screenshot instead."
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

/// Pages a PDF read returns when none are asked for.
const PDF_DEFAULT_PAGES: usize = 20;

/// A PDF's text by page, through `pdftotext` (poppler). Slides and scripts
/// are long, so the reader asks for pages.
fn read_pdf(file: &Path, path: &str, pages: Option<&str>) -> Result<String, String> {
    let out = std::process::Command::new("pdftotext")
        .args(["-layout", "-enc", "UTF-8"])
        .arg(file)
        .arg("-")
        .output()
        .map_err(|_| {
            format!(
                "can't read the PDF `{path}`: `pdftotext` isn't installed (`brew install poppler`). Ask the user to attach it instead."
            )
        })?;
    if !out.status.success() {
        return Err(format!(
            "can't read the PDF `{path}`: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut all: Vec<&str> = text.split('\u{c}').collect();
    if all.last().is_some_and(|p| p.trim().is_empty()) {
        all.pop();
    }
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
        let block = format!("\n--- page {} ---\n{}\n", i + 1, page.trim_end());
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

fn read(args: &Value, env: &ToolEnv<'_>) -> Result<String, String> {
    let path = str_arg(args, "path").ok_or("`path` is required")?;
    let file = confine(env.root, path, false, env.attached)?;
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
    let mut out = String::new();
    for (i, line) in text.lines().enumerate().skip(offset - 1).take(limit) {
        if out.len() + line.len() > MAX_READ_BYTES {
            let _ = writeln!(
                out,
                "... (stopped at {MAX_READ_BYTES} bytes; use offset to continue)"
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
    'files: for entry in WalkBuilder::new(&base)
        .sort_by_file_name(Ord::cmp)
        .build()
        .flatten()
    {
        let path = entry.path();
        if !path.is_file() || entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_GREP_FILE {
            continue;
        }
        if let Some(m) = &only {
            if !m.is_match(path.file_name().unwrap_or_default()) && !m.is_match(path) {
                continue;
            }
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
                    hits.push(format!("... (stopped at {MAX_MATCHES} matches)"));
                    break 'files;
                }
            }
        }
    }
    Ok(if hits.is_empty() {
        "No matches.".to_string()
    } else {
        hits.join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        dir.canonicalize().unwrap()
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
}
