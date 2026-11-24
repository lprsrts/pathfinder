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
            description: "Read a text file in the workspace. Returns numbered lines.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path, relative to the workspace."},
                    "offset": {"type": "integer", "description": "First line to read (1-based)."},
                    "limit": {"type": "integer", "description": "Number of lines to read."}
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

fn read(args: &Value, env: &ToolEnv<'_>) -> Result<String, String> {
    let path = str_arg(args, "path").ok_or("`path` is required")?;
    let file = confine(env.root, path, false, env.attached)?;
    if file.is_dir() {
        return Err(format!("`{path}` is a folder; use `ls` to list it"));
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
}
