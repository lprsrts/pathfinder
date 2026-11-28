//! Live output: tool calls as thread entries, and the user-facing field of a
//! JSON answer decoded as it streams in.

use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Search,
    Execute,
    Fetch,
    Other,
}

/// Content shown inside a tool call entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolContent {
    /// A file change: `old` is None for a new file.
    Diff {
        path: PathBuf,
        old: Option<String>,
        new: String,
    },
    Text(String),
}

impl ToolContent {
    /// What a tool row shows of it: a diff as its size, text as its last
    /// six lines.
    pub fn summary(&self) -> String {
        match self {
            ToolContent::Diff { path, old, new } => {
                let before = old.as_deref().map_or(0, |o| o.lines().count());
                format!(
                    "{}: {} lines, was {before}",
                    path.display(),
                    new.lines().count()
                )
            }
            ToolContent::Text(t) => {
                let tail: Vec<&str> = t.lines().rev().take(6).collect();
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            }
        }
    }
}

/// `path` as a tool row shows it: relative to the workspace `root` the
/// agent works in (a staging copy's own root included), as the row's title
/// names it; absolute only when it lies outside.
pub fn shown_path(path: &Path, root: &Path) -> PathBuf {
    if let Ok(rel) = path.strip_prefix(root) {
        return rel.to_path_buf();
    }
    // `/tmp` and `/private/tmp` on macOS: compare the real paths too.
    match (path.canonicalize(), root.canonicalize()) {
        (Ok(p), Ok(r)) => p
            .strip_prefix(&r)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStart {
    pub id: String,
    pub title: String,
    pub kind: ToolKind,
    pub path: Option<PathBuf>,
}

/// Turns a CLI's tool name and input into a thread entry.
pub fn describe_tool(id: String, name: &str, input: &Value, cwd: &Path) -> ToolStart {
    let str_field = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| input.get(*k).and_then(Value::as_str))
            .map(str::to_string)
    };
    let path = str_field(&["file_path", "path"]).map(|p| {
        let p = PathBuf::from(p);
        if p.is_absolute() { p } else { cwd.join(p) }
    });
    let shown = path.as_ref().map(|p| match p.strip_prefix(cwd) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.display().to_string(),
        Err(_) => p.display().to_string(),
    });

    let lower = name.to_ascii_lowercase();
    let (kind, title) = match lower.as_str() {
        "read" => (ToolKind::Read, {
            let file = shown.clone().unwrap_or_default();
            let num = |k: &str| input.get(k).and_then(Value::as_u64);
            match (
                input
                    .get("pages")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|p| !p.is_empty()),
                num("offset"),
                num("limit"),
            ) {
                (Some(pages), _, _) if pages.contains(['-', ',']) => {
                    format!("Read {file} (pages {pages})")
                }
                (Some(page), _, _) => format!("Read {file} (page {page})"),
                (None, Some(o), Some(l)) => {
                    format!("Read {file} (lines {o}–{})", o + l.saturating_sub(1))
                }
                (None, Some(o), None) => format!("Read {file} (from line {o})"),
                _ => format!("Read {file}"),
            }
        }),
        "view" => (
            ToolKind::Read,
            match input.get("pages").and_then(|v| v.as_str()) {
                Some(pages) => {
                    format!("View {} (pages {pages})", shown.clone().unwrap_or_default())
                }
                None => format!("View {}", shown.clone().unwrap_or_default()),
            },
        ),
        "edit" | "multiedit" => (
            ToolKind::Edit,
            format!("Edit {}", shown.clone().unwrap_or_default()),
        ),
        "write" => (
            ToolKind::Edit,
            format!("Write {}", shown.clone().unwrap_or_default()),
        ),
        "glob" | "grep" | "find" | "ls" => {
            let what = str_field(&["pattern", "query"]).or_else(|| shown.clone());
            let label = if lower == "ls" {
                "List".to_string()
            } else {
                capitalize(&lower)
            };
            match what {
                Some(w) => (ToolKind::Search, format!("{label} `{w}`")),
                None => (ToolKind::Search, label),
            }
        }
        "bash" => (ToolKind::Execute, {
            let cmd = str_field(&["command"]).unwrap_or_default();
            let flag = |k: &str| input.get(k).and_then(Value::as_bool).unwrap_or(false);
            let why = str_field(&["justification"])
                .map(|w| format!(": {w}"))
                .unwrap_or_default();
            if flag("outside_sandbox") {
                format!("Run `{cmd}` outside the sandbox{why}")
            } else if flag("network") {
                format!("Run `{cmd}` with network access{why}")
            } else {
                format!("Run `{cmd}`")
            }
        }),
        "webfetch" | "websearch" | "web_search" => (ToolKind::Fetch, capitalize(name)),
        _ => (ToolKind::Other, name.to_string()),
    };
    ToolStart {
        id,
        title: title.trim().to_string(),
        kind,
        path,
    }
}

/// Codex runs commands as `/bin/zsh -lc "..."`; show the inner command.
pub fn strip_shell_wrapper(command: &str) -> String {
    for prefix in ["/bin/zsh -lc ", "/bin/bash -lc ", "bash -lc ", "zsh -lc "] {
        if let Some(rest) = command.strip_prefix(prefix) {
            let rest = rest.trim();
            let unquoted = rest
                .strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .or_else(|| rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')));
            return unquoted.unwrap_or(rest).to_string();
        }
    }
    command.to_string()
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldState {
    Seeking,
    InString,
    Done,
}

/// Decodes one top-level string field (e.g. `"reply"`) out of JSON text that
/// arrives in arbitrary chunks, returning the newly decoded text per chunk.
pub struct FieldStreamer {
    key: String,
    buf: String,
    pos: usize,
    state: FieldState,
}

impl FieldStreamer {
    pub fn new(field: &str) -> Self {
        Self {
            key: format!("\"{field}\""),
            buf: String::new(),
            pos: 0,
            state: FieldState::Seeking,
        }
    }

    pub fn push(&mut self, chunk: &str) -> String {
        if self.state == FieldState::Done {
            return String::new();
        }
        self.buf.push_str(chunk);
        if self.state == FieldState::Seeking && !self.seek() {
            return String::new();
        }
        self.decode()
    }

    /// Finds `"key" : "` and moves `pos` to the string's first character.
    fn seek(&mut self) -> bool {
        while let Some(rel) = self.buf[self.pos..].find(&self.key) {
            let after_key = self.pos + rel + self.key.len();
            let rest = &self.buf[after_key..];
            let trimmed = rest.trim_start();
            let Some(after_colon) = trimmed.strip_prefix(':') else {
                if trimmed.is_empty() {
                    return false; // wait for more input
                }
                self.pos = after_key; // the key text appeared as a value
                continue;
            };
            let value = after_colon.trim_start();
            if value.is_empty() {
                return false;
            }
            if !value.starts_with('"') {
                self.state = FieldState::Done; // not a string field
                return false;
            }
            self.pos = self.buf.len() - value.len() + 1;
            self.state = FieldState::InString;
            return true;
        }
        // Keep enough tail to match a key split across chunks.
        self.pos = self.buf.len().saturating_sub(self.key.len());
        while !self.buf.is_char_boundary(self.pos) {
            self.pos -= 1;
        }
        false
    }

    fn decode(&mut self) -> String {
        let mut out = String::new();
        let bytes = self.buf.as_bytes();
        let mut i = self.pos;
        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    self.state = FieldState::Done;
                    i += 1;
                    break;
                }
                b'\\' => {
                    let Some(&esc) = bytes.get(i + 1) else { break };
                    let rest = &self.buf[i + 1..];
                    // A LaTeX command could still be arriving: wait for the
                    // letters to end before deciding.
                    if matches!(esc, b'b' | b'f' | b'n' | b'r' | b't')
                        && rest.chars().all(|c| c.is_ascii_alphabetic())
                        && rest.len() < 12
                    {
                        break;
                    }
                    let literal = match esc {
                        b'b' | b'f' | b'n' | b'r' | b't' => latex_command(rest),
                        b'u' => rest.len() >= 5 && !unicode_escape(rest),
                        b'"' | b'\\' | b'/' => false,
                        _ => true,
                    };
                    if literal {
                        out.push('\\');
                        i += 1;
                        continue;
                    }
                    match esc {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let Some((ch, used)) = decode_unicode(&self.buf[i..]) else {
                                break; // incomplete escape; wait for more input
                            };
                            out.push(ch);
                            i += used;
                            continue;
                        }
                        other => out.push(other as char),
                    }
                    i += 2;
                }
                _ => {
                    let ch = self.buf[i..].chars().next().unwrap();
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
        self.pos = i;
        out
    }
}

/// LaTeX commands that begin with a JSON escape letter (`\\frac` reads as
/// form feed + "rac"). Models write them inside JSON strings with a single
/// backslash; both the stream and the repair step keep them as LaTeX.
const LATEX_ESCAPED: [&str; 44] = [
    "beta",
    "bar",
    "bf",
    "boldsymbol",
    "binom",
    "big",
    "bigl",
    "bigr",
    "bigg",
    "begin",
    "bullet",
    "bmatrix",
    "frac",
    "forall",
    "flat",
    "nabla",
    "neq",
    "ne",
    "nu",
    "not",
    "newline",
    "nleq",
    "ngeq",
    "rho",
    "right",
    "rangle",
    "rightarrow",
    "rceil",
    "rfloor",
    "theta",
    "tau",
    "times",
    "text",
    "textbf",
    "textit",
    "tan",
    "tanh",
    "to",
    "top",
    "tilde",
    "triangle",
    "tfrac",
    "neg",
    "nexists",
];

/// Whether `rest` (the text after a backslash) starts with a LaTeX command
/// from `LATEX_ESCAPED`, ending at a non-letter (or at the end, which may
/// still grow: then it counts once the command is complete).
pub(crate) fn latex_command(rest: &str) -> bool {
    let word: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    LATEX_ESCAPED.contains(&word.as_str())
}

/// `\\u` with four hex digits after it.
pub(crate) fn unicode_escape(rest: &str) -> bool {
    rest.len() >= 5 && rest[1..5].chars().all(|c| c.is_ascii_hexdigit())
}

/// Decodes `\uXXXX` (and a following low surrogate). None if incomplete.
fn decode_unicode(s: &str) -> Option<(char, usize)> {
    let hex = |from: usize| u32::from_str_radix(s.get(from..from + 4)?, 16).ok();
    let first = hex(2)?;
    if (0xD800..0xDC00).contains(&first) {
        if s.get(6..8)? != "\\u" {
            return Some((char::REPLACEMENT_CHARACTER, 6));
        }
        let second = hex(8)?;
        let code = 0x10000 + ((first - 0xD800) << 10) + (second.wrapping_sub(0xDC00) & 0x3FF);
        return Some((
            char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER),
            12,
        ));
    }
    Some((
        char::from_u32(first).unwrap_or(char::REPLACEMENT_CHARACTER),
        6,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_row_shows_paths_relative_to_the_workspace() {
        let root = Path::new("/private/tmp/ws");
        assert_eq!(
            shown_path(Path::new("/private/tmp/ws/out/letter.txt"), root),
            PathBuf::from("out/letter.txt")
        );
        // Outside the workspace it stays whole.
        assert_eq!(
            shown_path(Path::new("/elsewhere/x.txt"), root),
            PathBuf::from("/elsewhere/x.txt")
        );
        // A real path under a linked root (`/tmp` on macOS) still shortens.
        let dir = std::env::temp_dir().join(format!("pf-shown-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        let real = dir.canonicalize().unwrap().join("a.txt");
        assert_eq!(shown_path(&real, &dir), PathBuf::from("a.txt"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn latex_in_a_json_string_streams_as_latex() {
        // As models write it: single backslashes, some valid JSON escapes.
        let raw = r#"{"reply": "P = U \cdot I, \frac{1}{2} \theta\nNeu: \u00e4 \underline{x}"}"#;
        let mut f = FieldStreamer::new("reply");
        let mut out = String::new();
        for ch in raw.chars() {
            out.push_str(&f.push(&ch.to_string()));
        }
        assert_eq!(
            out,
            "P = U \\cdot I, \\frac{1}{2} \\theta\nNeu: ä \\underline{x}"
        );
    }

    fn stream_all(field: &str, chunks: &[&str]) -> String {
        let mut s = FieldStreamer::new(field);
        chunks.iter().map(|c| s.push(c)).collect()
    }

    #[test]
    fn decodes_field_across_arbitrary_chunk_splits() {
        let json = r#"```json
{"ready_to_map": false, "reply": "Line one\nwith \"quotes\", a tab\tand café 😀.", "goal": null}
```"#;
        let expected = "Line one\nwith \"quotes\", a tab\tand café 😀.";
        // Every split point, including inside the key and inside escapes.
        for cut in 0..json.len() {
            if !json.is_char_boundary(cut) {
                continue;
            }
            let (a, b) = json.split_at(cut);
            assert_eq!(stream_all("reply", &[a, b]), expected, "split at {cut}");
        }
        let one_by_one: Vec<String> = json.chars().map(String::from).collect();
        let refs: Vec<&str> = one_by_one.iter().map(String::as_str).collect();
        assert_eq!(stream_all("reply", &refs), expected);
    }

    #[test]
    fn ignores_key_text_inside_other_values_and_stops_at_end() {
        let json = r#"{"note": "the \"reply\" key", "reply": "hi", "more": "x"}"#;
        assert_eq!(stream_all("reply", &[json]), "hi");
        assert_eq!(stream_all("reply", &[r#"{"reply": null}"#]), "");
        assert_eq!(stream_all("reply", &["plain prose, no json"]), "");
    }

    #[test]
    fn describes_tools_from_each_cli() {
        let cwd = Path::new("/w");
        let t = describe_tool(
            "1".into(),
            "Read",
            &serde_json::json!({"file_path": "/w/src/a.rs"}),
            cwd,
        );
        assert_eq!(
            (t.kind, t.title.as_str()),
            (ToolKind::Read, "Read src/a.rs")
        );
        assert_eq!(t.path.as_deref(), Some(Path::new("/w/src/a.rs")));

        let t = describe_tool(
            "2".into(),
            "read",
            &serde_json::json!({"path": "src/b.rs"}),
            cwd,
        );
        assert_eq!(t.path.as_deref(), Some(Path::new("/w/src/b.rs")));

        let t = describe_tool(
            "3".into(),
            "Grep",
            &serde_json::json!({"pattern": "fn main"}),
            cwd,
        );
        assert_eq!(
            (t.kind, t.title.as_str()),
            (ToolKind::Search, "Grep `fn main`")
        );

        let t = describe_tool(
            "4".into(),
            "Bash",
            &serde_json::json!({"command": "cargo test"}),
            cwd,
        );
        assert_eq!(
            (t.kind, t.title.as_str()),
            (ToolKind::Execute, "Run `cargo test`")
        );

        assert_eq!(
            strip_shell_wrapper(r#"/bin/zsh -lc "rg -n name Cargo.toml""#),
            "rg -n name Cargo.toml"
        );
    }
}
