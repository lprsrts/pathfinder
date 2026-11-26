//! Slash commands, contextual actions, and path argument parsing/completion
//! shared by the TUI and other frontends.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::domain::{PartStatus, Session, SessionMode};

/// A command the frontend handles locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Local {
    Help,
    Quit,
    New,
    Resume,
    Model,
    Status,
    Copy,
    Thinking,
    Tools,
    Join,
    Data,
    Update,
    Attach,
    Share,
    Bug,
}

/// Which subsystem owns a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    Local,
    Task,
}

impl CommandCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Task => "task",
        }
    }
}

/// Rich metadata for a slash command, used by command palettes and help.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub args: &'static str,
    pub description: &'static str,
    pub category: CommandCategory,
}

impl CommandInfo {
    /// Display usage string, e.g. `"/find <terms>"` or `"/map"`.
    pub fn usage(self) -> String {
        if self.args.is_empty() {
            self.name.to_string()
        } else {
            format!("{} {}", self.name, self.args)
        }
    }
}

/// A contextual next-step action for the current `Session` state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuickAction {
    /// Command prefix to insert/execute (e.g. `"/accept"`, `"/object "`), or `""` for prose input.
    pub command: &'static str,
    /// Short label for the tray or side panel (e.g. `"/accept"`, `"right or flawed + why"`).
    pub label: &'static str,
    /// Brief description of what this action does right now.
    pub description: &'static str,
    /// Whether the user must supply arguments or prose before submitting.
    pub needs_arg: bool,
}

/// A candidate file or directory for `/attach`, `/open`, or `/check` completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachCandidate {
    /// Formatted argument to insert after the command (quoted if it contains spaces, trailing `/` for dirs).
    pub arg: String,
    /// Short label for display in a completion popup (e.g. `notes.pdf` or `slides/`).
    pub label: String,
    pub is_dir: bool,
}

/// (name, other names, what it does), in the order help shows them.
pub const LOCAL: [(Local, &str, &[&str], &str); 15] = [
    (Local::Help, "/help", &["/?"], "these commands and the keys"),
    (
        Local::Quit,
        "/quit",
        &["/exit", "/q"],
        "quit; the conversation stays in your terminal",
    ),
    (Local::New, "/new", &["/clear"], "start a new thread"),
    (
        Local::Resume,
        "/resume",
        &["/threads"],
        "open an earlier thread in this folder",
    ),
    (
        Local::Model,
        "/model",
        &["/models", "/settings"],
        "models, decisions, checks",
    ),
    (
        Local::Status,
        "/status",
        &["/version"],
        "version, models, sign-in, gate, sharing",
    ),
    (Local::Copy, "/copy", &[], "copy the last reply"),
    (Local::Thinking, "/thinking", &[], "show or hide thinking"),
    (Local::Tools, "/tools", &[], "show or hide every tool call"),
    (
        Local::Join,
        "/join",
        &[],
        "join the beta, or change what you share",
    ),
    (
        Local::Data,
        "/data",
        &[],
        "what you share, and how to change it",
    ),
    (Local::Update, "/update", &[], "check for a new version"),
    (
        Local::Attach,
        "/attach",
        &[],
        "attach a PDF or image, or /attach clear",
    ),
    (
        Local::Share,
        "/share",
        &[],
        "send this thread (shown in full first)",
    ),
    (
        Local::Bug,
        "/bug",
        &[],
        "send a bug report (shown in full first)",
    ),
];

/// The task commands, run by the runtime: (name, what it does).
pub const TASK: [(&str, &str); 20] = [
    ("/find", "search files: /find <terms>"),
    ("/open", "show a file: /open <file> [page or line]"),
    ("/web", "search online: /web <terms>"),
    ("/tell", "get an answer, recorded as told: /tell <question>"),
    ("/check", "first wrong step: /check <work, file or photo>"),
    ("/why", "your record, or what the current part rests on"),
    ("/map", "the plan as a diagram"),
    ("/plan", "build the plan now"),
    ("/summary", "explain the current part; a candidate follows"),
    ("/next", "move to the next open part"),
    ("/focus", "switch to a part: /focus <number or id>"),
    ("/known", "skip a part you already know"),
    ("/handoff", "give an understood part to the agent"),
    ("/delegate", "hand a part off without understanding it"),
    ("/approve", "accept the agent's result"),
    ("/object", "send the result back: /object <reason>"),
    ("/accept", "accept a proposed change to the plan"),
    ("/reject", "reject a proposed change to the plan"),
    ("/replan", "ask for a change to the plan"),
    (
        "/coverage",
        "how much of the agent's work you can vouch for",
    ),
];

const LOCAL_ARGS: [(&str, &str, &str); 3] = [
    ("/attach", "<file|clear>", "attach a PDF or image, or clear attachments"),
    ("/share", "[note]", "send this thread (shown in full first)"),
    ("/bug", "[what happened]", "send a bug report (shown in full first)"),
];

const TASK_META: [(&str, &str, &str); 20] = [
    ("/find", "<terms>", "search files"),
    ("/open", "<file> [page or line]", "show a file"),
    ("/web", "<terms>", "search online"),
    ("/tell", "<question>", "get an answer, recorded as told"),
    ("/check", "<work, file or photo>", "first wrong step"),
    ("/why", "", "your record, or what the current part rests on"),
    ("/map", "", "the plan as a diagram"),
    ("/plan", "[goal]", "build the plan now"),
    ("/summary", "[text]", "explain the current part; a candidate follows"),
    ("/next", "", "move to the next open part"),
    ("/focus", "<number or id>", "switch to a part"),
    ("/known", "[id]", "skip a part you already know"),
    ("/handoff", "[notes]", "give an understood part to the agent"),
    ("/delegate", "[notes]", "hand a part off without understanding it"),
    ("/approve", "", "accept the agent's result"),
    ("/object", "<reason>", "send the result back"),
    ("/accept", "", "accept a proposed change to the plan"),
    ("/reject", "", "reject a proposed change to the plan"),
    ("/replan", "[what changed]", "ask for a change to the plan"),
    ("/coverage", "", "how much of the agent's work you can vouch for"),
];

/// Metadata for every known slash command, local first then task.
pub fn all() -> Vec<CommandInfo> {
    let mut out = Vec::with_capacity(LOCAL.len() + TASK_META.len());
    for (_, name, aliases, desc) in LOCAL {
        let (args, description) = LOCAL_ARGS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map_or(("", desc), |(_, a, d)| (*a, *d));
        out.push(CommandInfo {
            name,
            aliases,
            args,
            description,
            category: CommandCategory::Local,
        });
    }
    for (name, args, description) in TASK_META {
        out.push(CommandInfo {
            name,
            aliases: &[],
            args,
            description,
            category: CommandCategory::Task,
        });
    }
    out
}

/// What a line starting with `/` is.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed<'a> {
    Local(Local, &'a str),
    Task,
    Unknown(&'a str),
}

pub fn parse(text: &str) -> Parsed<'_> {
    let (word, arg) = match text.split_once(char::is_whitespace) {
        Some((w, a)) => (w, a.trim()),
        None => (text, ""),
    };
    let word_lower = word.to_ascii_lowercase();
    for (cmd, name, aliases, _) in LOCAL {
        if word_lower == name || aliases.contains(&word_lower.as_str()) {
            return Parsed::Local(cmd, arg);
        }
    }
    if TASK.iter().any(|(name, _)| *name == word_lower) {
        return Parsed::Task;
    }
    Parsed::Unknown(word)
}

/// Every command name, for completion.
#[allow(dead_code)]
fn names() -> impl Iterator<Item = &'static str> {
    LOCAL
        .iter()
        .map(|(_, name, ..)| *name)
        .chain(TASK.iter().map(|(name, _)| *name))
}

/// The commands whose canonical name starts with what's typed (only while the
/// first word is still being typed).
#[allow(dead_code)]
pub fn matches(input: &str) -> Vec<&'static str> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let typed = input.to_ascii_lowercase();
    names().filter(|n| n.starts_with(&typed)).collect()
}

/// Searches commands matching prefix, alias, or substring while typing the first word.
#[allow(dead_code)]
pub fn search(input: &str) -> Vec<CommandInfo> {
    search_for(input, None)
}

/// Searches commands while typing the first word, ordering contextual matches
/// for `session` first within each match tier.
pub fn search_for(input: &str, session: Option<&Session>) -> Vec<CommandInfo> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let typed = input.to_ascii_lowercase();
    let needle = typed.strip_prefix('/').unwrap_or(&typed);

    let context_rank: HashMap<&'static str, usize> = session
        .map(|s| {
            contextual_actions(s)
                .into_iter()
                .filter(|a| !a.command.is_empty())
                .enumerate()
                .map(|(idx, a)| (a.command.trim(), idx))
                .collect()
        })
        .unwrap_or_default();

    let mut scored: Vec<((u8, usize, usize), CommandInfo)> = Vec::new();
    for (order, info) in all().into_iter().enumerate() {
        let match_tier = if info.name == typed {
            Some(0)
        } else if info.name.starts_with(&typed) {
            Some(1)
        } else if info.aliases.iter().any(|a| a.starts_with(&typed)) {
            Some(2)
        } else if !needle.is_empty() && info.name[1..].contains(needle) {
            Some(3)
        } else if !needle.is_empty() && info.aliases.iter().any(|a| a[1..].contains(needle)) {
            Some(4)
        } else {
            None
        };
        if let Some(tier) = match_tier {
            let ctx = context_rank.get(info.name).copied().unwrap_or(usize::MAX);
            scored.push(((tier, ctx, order), info));
        }
    }
    scored.sort_by_key(|(key, _)| *key);
    scored.into_iter().map(|(_, info)| info).collect()
}

/// What Tab completes the input to: the one match, or the prefix matches'
/// common beginning.
#[allow(dead_code)]
pub fn complete(input: &str) -> Option<String> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return None;
    }
    let prefix = matches(input);
    if prefix.len() == 1 {
        return Some(format!("{} ", prefix[0]));
    }
    if prefix.is_empty() {
        let found = search(input);
        if found.len() == 1 {
            return Some(format!("{} ", found[0].name));
        }
        return None;
    }
    let first = prefix[0];
    let common: String = first
        .chars()
        .enumerate()
        .take_while(|(i, c)| prefix.iter().all(|n| n.chars().nth(*i) == Some(*c)))
        .map(|(_, c)| c)
        .collect();
    (common.len() > input.len()).then_some(common)
}

/// Completes the `index`-th match from [`search_for`] with a trailing space.
pub fn complete_at(input: &str, index: usize, session: Option<&Session>) -> Option<String> {
    let found = search_for(input, session);
    found.get(index).map(|cmd| format!("{} ", cmd.name))
}

/// Steps a palette selection index by `delta` (`+1` / `-1`), wrapping around `0..len`.
pub fn step_selection(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let clamped = current.min(len - 1) as isize;
    (clamped + delta).rem_euclid(len as isize) as usize
}

/// Contextual next-step actions for the current `Session` state.
pub fn contextual_actions(session: &Session) -> Vec<QuickAction> {
    let mut actions = Vec::new();
    if session.pending_change.is_some() {
        actions.push(QuickAction {
            command: "/accept",
            label: "/accept",
            description: "accept the proposed plan change",
            needs_arg: false,
        });
        actions.push(QuickAction {
            command: "/reject",
            label: "/reject",
            description: "reject the proposed plan change",
            needs_arg: false,
        });
    }
    if session.pending_candidate.is_some() {
        if session.exam {
            actions.push(QuickAction {
                command: "",
                label: "answer in your own words",
                description: "explain your answer to the exam question",
                needs_arg: true,
            });
        } else {
            actions.push(QuickAction {
                command: "",
                label: "right or flawed + why",
                description: "judge the waiting candidate and explain why",
                needs_arg: true,
            });
        }
    }
    if session.pending_candidate.is_none()
        && let Some(part) = session.active_part()
    {
        match part.status {
            PartStatus::InReview => {
                actions.push(QuickAction {
                    command: "/approve",
                    label: "/approve",
                    description: "accept the agent's result",
                    needs_arg: false,
                });
                actions.push(QuickAction {
                    command: "/object ",
                    label: "/object <reason>",
                    description: "send the result back",
                    needs_arg: true,
                });
            }
            PartStatus::Open if session.mode == SessionMode::Working => {
                actions.push(QuickAction {
                    command: "/summary ",
                    label: "/summary <text>",
                    description: "explain the current part",
                    needs_arg: true,
                });
                actions.push(QuickAction {
                    command: "/known",
                    label: "/known",
                    description: "skip a part you already know",
                    needs_arg: false,
                });
                actions.push(QuickAction {
                    command: "/delegate ",
                    label: "/delegate",
                    description: "hand off without understanding",
                    needs_arg: true,
                });
            }
            PartStatus::Understood if session.mode == SessionMode::Working => {
                actions.push(QuickAction {
                    command: "/handoff ",
                    label: "/handoff",
                    description: "give an understood part to the agent",
                    needs_arg: true,
                });
            }
            _ => {}
        }
    }
    if session.mode == SessionMode::Working {
        actions.push(QuickAction {
            command: "/next",
            label: "/next",
            description: "move to the next open part",
            needs_arg: false,
        });
        actions.push(QuickAction {
            command: "/map",
            label: "/map",
            description: "the plan as a diagram",
            needs_arg: false,
        });
        actions.push(QuickAction {
            command: "/why",
            label: "/why",
            description: "your understanding record",
            needs_arg: false,
        });
    } else if session.mode == SessionMode::Onboarding && session.map.parts.is_empty() {
        actions.push(QuickAction {
            command: "/find ",
            label: "/find <terms>",
            description: "search workspace files",
            needs_arg: true,
        });
        actions.push(QuickAction {
            command: "/plan",
            label: "/plan",
            description: "build the plan now",
            needs_arg: false,
        });
    } else if !session.history.is_empty() {
        actions.push(QuickAction {
            command: "/why",
            label: "/why",
            description: "your understanding record",
            needs_arg: false,
        });
    }
    actions
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub(crate) fn expand_tilde(path: &str) -> String {
    if path == "~" {
        return home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|| "~".to_string());
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home_dir()
    {
        return format!("{}/{rest}", home.to_string_lossy().trim_end_matches('/'));
    }
    path.to_string()
}

pub(crate) fn unescape_shell_path(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek()
            && matches!(
                next,
                ' ' | '\'' | '"' | '\\' | '(' | ')' | '[' | ']' | '&' | ';' | '!' | '$'
            )
        {
            out.push(next);
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// Parses a file path argument: strips surrounding quotes/backticks, unescapes
/// backslash-escaped spaces/metacharacters, and expands a leading `~`.
pub fn parse_path_arg(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let trimmed = if trimmed.len() >= 2 && trimmed.starts_with('`') && trimmed.ends_with('`') {
        trimmed[1..trimmed.len() - 1].trim()
    } else {
        trimmed
    };
    let unquoted = if trimmed.len() >= 2 && trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        unescape_shell_path(&trimmed[1..trimmed.len() - 1])
    } else if let Some(rest) = trimmed.strip_prefix('\'') {
        rest.to_string()
    } else if let Some(rest) = trimmed.strip_prefix('"') {
        unescape_shell_path(rest)
    } else {
        unescape_shell_path(trimmed)
    };
    expand_tilde(&unquoted)
}

/// Resolves an `/attach` argument against `cwd`.
pub fn resolve_attach_path(raw: &str, cwd: &Path) -> Option<PathBuf> {
    let parsed = parse_path_arg(raw);
    if parsed.is_empty() {
        return None;
    }
    let p = Path::new(&parsed);
    Some(if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    })
}

/// If `input` is currently typing the path argument of `/attach`, `/open`, or
/// `/check`, returns `(canonical_command, trimmed_start_arg_slice)`.
pub fn path_arg_slice(input: &str) -> Option<(&'static str, &str)> {
    let mut split_at = None;
    for (idx, ch) in input.char_indices() {
        if ch.is_whitespace() {
            split_at = Some(idx);
            break;
        }
    }
    let idx = split_at?;
    let head = &input[..idx];
    let cmd = if head.eq_ignore_ascii_case("/attach") {
        "/attach"
    } else if head.eq_ignore_ascii_case("/open") {
        "/open"
    } else if head.eq_ignore_ascii_case("/check") {
        "/check"
    } else {
        return None;
    };
    Some((cmd, input[idx..].trim_start()))
}

fn attach_arg_slice(input: &str) -> Option<&str> {
    path_arg_slice(input).and_then(|(cmd, rest)| (cmd == "/attach").then_some(rest))
}

fn format_path_arg(path: &str, escape_only: bool) -> String {
    if escape_only {
        let mut out = String::with_capacity(path.len());
        for c in path.chars() {
            if matches!(
                c,
                ' ' | '\'' | '"' | '\\' | '(' | ')' | '[' | ']' | '&' | ';' | '!' | '$'
            ) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    } else if path.contains(char::is_whitespace) {
        let escaped = path.replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        path.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathFilter {
    Attachable,
    Openable,
    Checkable,
}

fn matches_for_slice(raw_arg: &str, cwd: &Path, filter: PathFilter) -> Vec<AttachCandidate> {
    let trimmed = raw_arg.trim_start();
    let unquoted = if let Some(rest) = trimmed.strip_prefix('\'') {
        rest.strip_suffix('\'').unwrap_or(rest).to_string()
    } else if let Some(rest) = trimmed.strip_prefix('"') {
        unescape_shell_path(rest.strip_suffix('"').unwrap_or(rest))
    } else {
        unescape_shell_path(trimmed)
    };

    if unquoted == "~" && filter == PathFilter::Attachable {
        return vec![AttachCandidate {
            arg: "~/".to_string(),
            label: "~/".to_string(),
            is_dir: true,
        }];
    }

    let (dir_display, file_prefix) = match unquoted.rfind('/') {
        Some(pos) => (&unquoted[..=pos], &unquoted[pos + 1..]),
        None => ("", unquoted.as_str()),
    };

    let search_dir = if dir_display.is_empty() {
        cwd.to_path_buf()
    } else {
        let expanded_dir = expand_tilde(dir_display);
        let p = Path::new(&expanded_dir);
        if p.is_absolute() {
            if filter == PathFilter::Openable {
                // `/open` is confined to the workspace.
                let Ok(root) = cwd.canonicalize() else {
                    return Vec::new();
                };
                if !p.canonicalize().is_ok_and(|c| c.starts_with(&root)) {
                    return Vec::new();
                }
            }
            p.to_path_buf()
        } else {
            cwd.join(p)
        }
    };

    let smart_case = file_prefix.chars().any(|c| c.is_uppercase());
    let prefix_lower = file_prefix.to_ascii_lowercase();
    let show_hidden = file_prefix.starts_with('.');

    let mut entries: Vec<(bool, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&search_dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let matches_prefix = if smart_case {
                name.starts_with(file_prefix)
            } else {
                name.to_ascii_lowercase().starts_with(&prefix_lower)
            };
            if !matches_prefix {
                continue;
            }
            let path = entry.path();
            let is_dir = path.is_dir();
            let include = match filter {
                PathFilter::Attachable => is_dir || crate::llm::attachable_type(&path).is_some(),
                PathFilter::Openable => {
                    is_dir || crate::sources::is_openable(&path)
                }
                PathFilter::Checkable => {
                    is_dir
                        || crate::llm::attachable_type(&path).is_some()
                        || crate::sources::is_openable(&path)
                }
            };
            if include {
                entries.push((is_dir, name));
            }
        }
    }
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_ascii_lowercase().cmp(&b.1.to_ascii_lowercase()))
            .then_with(|| a.1.cmp(&b.1))
    });

    let mut out: Vec<AttachCandidate> = entries
        .into_iter()
        .map(|(is_dir, name)| {
            let suffix = if is_dir { "/" } else { "" };
            let joined = format!("{dir_display}{name}{suffix}");
            AttachCandidate {
                arg: format_path_arg(&joined, is_dir),
                label: format!("{name}{suffix}"),
                is_dir,
            }
        })
        .collect();

    if filter == PathFilter::Attachable
        && !unquoted.is_empty()
        && !unquoted.contains('/')
        && "clear".starts_with(&unquoted.to_ascii_lowercase())
    {
        out.push(AttachCandidate {
            arg: "clear".to_string(),
            label: "clear (remove attachments)".to_string(),
            is_dir: false,
        });
    }
    out
}

/// Lists matching attachable files and subdirectories for `/attach <prefix>`.
pub fn attach_matches(input: &str, cwd: &Path) -> Vec<AttachCandidate> {
    let Some(raw_arg) = attach_arg_slice(input) else {
        return Vec::new();
    };
    matches_for_slice(raw_arg, cwd, PathFilter::Attachable)
}

/// Lists matching path candidates for `/attach`, `/open`, or `/check`.
pub fn path_matches(input: &str, cwd: &Path) -> Vec<AttachCandidate> {
    let Some((cmd, raw_arg)) = path_arg_slice(input) else {
        return Vec::new();
    };
    let filter = match cmd {
        "/attach" => PathFilter::Attachable,
        "/open" => PathFilter::Openable,
        "/check" => PathFilter::Checkable,
        _ => return Vec::new(),
    };
    matches_for_slice(raw_arg, cwd, filter)
}

/// Tab-completes `/attach`, `/open`, or `/check` to a matching file/directory or common prefix.
pub fn complete_path(input: &str, cwd: &Path) -> Option<String> {
    let (cmd, raw_arg) = path_arg_slice(input)?;
    let candidates = path_matches(input, cwd);
    let first = candidates.first()?;
    if candidates.len() == 1 {
        return Some(format!("{cmd} {}", first.arg));
    }
    // Compare unquoted raw paths for common prefix so quotes don't truncate mid-name.
    let unquoted: Vec<String> = candidates
        .iter()
        .map(|c| parse_path_arg(&c.arg))
        .collect();
    let first_raw = &unquoted[0];
    let common: String = first_raw
        .chars()
        .enumerate()
        .take_while(|(i, c)| unquoted.iter().all(|s| s.chars().nth(*i) == Some(*c)))
        .map(|(_, c)| c)
        .collect();
    let current = parse_path_arg(raw_arg);
    if common.len() > current.len() {
        Some(format!("{cmd} {}", format_path_arg(&common, true)))
    } else {
        None
    }
}

/// Completes `/attach`, `/open`, or `/check` with the `index`-th candidate.
pub fn complete_path_at(input: &str, index: usize, cwd: &Path) -> Option<String> {
    let (cmd, _) = path_arg_slice(input)?;
    path_matches(input, cwd)
        .get(index)
        .map(|c| format!("{cmd} {}", c.arg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Map, MapChange, Part, PendingCandidate};

    #[test]
    fn names_and_aliases_are_recognised() {
        assert_eq!(parse("/quit"), Parsed::Local(Local::Quit, ""));
        assert_eq!(parse("/exit"), Parsed::Local(Local::Quit, ""));
        assert_eq!(parse("/Q"), Parsed::Local(Local::Quit, ""));
        assert_eq!(parse("/clear"), Parsed::Local(Local::New, ""));
        assert_eq!(parse("/tools"), Parsed::Local(Local::Tools, ""));
        assert_eq!(
            parse("/bug it broke"),
            Parsed::Local(Local::Bug, "it broke")
        );
        assert_eq!(parse("/why"), Parsed::Task);
        assert_eq!(parse("/WHY"), Parsed::Task);
        assert_eq!(parse("/focus 3"), Parsed::Task);
        assert_eq!(parse("/frobnicate"), Parsed::Unknown("/frobnicate"));
    }

    #[test]
    fn every_name_is_unique() {
        let mut all: Vec<&str> = names().collect();
        all.extend(
            LOCAL
                .iter()
                .flat_map(|(_, _, aliases, _)| aliases.iter().copied()),
        );
        let count = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), count);
    }

    #[test]
    fn tab_completes_commands() {
        assert_eq!(complete("/qu").as_deref(), Some("/quit "));
        // /resume, /replan, /reject: nothing to add beyond what's typed.
        assert_eq!(complete("/re"), None);
        assert_eq!(complete("/repl").as_deref(), Some("/replan "));
        assert_eq!(complete("/res").as_deref(), Some("/resume "));
        assert_eq!(complete("/zz"), None);
        assert_eq!(complete("/clear").as_deref(), Some("/new "));
        assert!(matches("/quit now").is_empty());
        assert_eq!(matches("/s"), ["/status", "/share", "/summary"]);
        assert_eq!(parse("/find laplace"), Parsed::Task);
        assert_eq!(parse("/tell what is x"), Parsed::Task);
    }

    #[test]
    fn search_matches_prefix_alias_and_substring_with_context_first() {
        let bare = search("/");
        assert_eq!(bare.len(), LOCAL.len() + TASK.len());

        // Alias match: /clear -> /new
        let clear = search("/cl");
        assert_eq!(clear[0].name, "/new");

        // Substring match: /cov -> /coverage, /ran -> /replan? no, /pro -> /approve
        let sub = search("/pprove");
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].name, "/approve");

        // Contextual prioritization: pending_change puts /accept and /reject first.
        let mut s = Session::new("s".into(), PathBuf::from("."));
        s.mode = SessionMode::Working;
        s.pending_change = Some(MapChange {
            brief: "add prereq".into(),
            ..Default::default()
        });
        let a_matches = search_for("/a", Some(&s));
        assert_eq!(a_matches[0].name, "/accept");
        assert_eq!(complete_at("/re", 0, Some(&s)).as_deref(), Some("/reject "));

        assert_eq!(step_selection(0, 3, -1), 2);
        assert_eq!(step_selection(2, 3, 1), 0);
        assert_eq!(step_selection(0, 0, 1), 0);
    }

    #[test]
    fn contextual_actions_reflect_session_state() {
        let mut s = Session::new("s".into(), PathBuf::from("."));
        assert!(
            contextual_actions(&s)
                .iter()
                .any(|a| a.command == "/plan")
        );

        s.mode = SessionMode::Working;
        s.map = Map {
            parts: vec![Part::new("p1".into(), "Part 1".into(), String::new(), vec![])],
        };
        s.active = Some("p1".into());
        let working_cmds: Vec<&str> = contextual_actions(&s)
            .into_iter()
            .map(|a| a.command)
            .collect();
        assert!(working_cmds.contains(&"/summary "));
        assert!(working_cmds.contains(&"/next"));
        assert!(working_cmds.contains(&"/map"));
        assert!(working_cmds.contains(&"/why"));

        s.map.parts[0].status = PartStatus::InReview;
        let review_cmds: Vec<&str> = contextual_actions(&s)
            .into_iter()
            .map(|a| a.command)
            .collect();
        assert_eq!(&review_cmds[..2], &["/approve", "/object "]);

        s.pending_candidate = Some(PendingCandidate {
            part_id: "p1".into(),
            candidate: "c".into(),
            key_correct: true,
            key_reason: String::new(),
            secret: None,
            targets: vec![],
            misconception: None,
            id: "1".into(),
        });
        assert!(
            contextual_actions(&s)
                .iter()
                .any(|a| a.label.contains("right or flawed"))
        );

        s.pending_change = Some(MapChange::default());
        let first_two: Vec<&str> = contextual_actions(&s)
            .into_iter()
            .take(2)
            .map(|a| a.command)
            .collect();
        assert_eq!(first_two, vec!["/accept", "/reject"]);
    }

    #[test]
    fn attach_open_and_check_arg_parsing_and_completion() {
        assert_eq!(parse_path_arg(r#""my file.pdf""#), "my file.pdf");
        assert_eq!(parse_path_arg(r#"'my file.pdf'"#), "my file.pdf");
        assert_eq!(parse_path_arg(r#"my\ file\ \(1\).pdf"#), "my file (1).pdf");
        if let Some(home) = std::env::var_os("HOME") {
            let expected = format!("{}/notes.pdf", home.to_string_lossy().trim_end_matches('/'));
            assert_eq!(parse_path_arg("~/notes.pdf"), expected);
        }

        let dir = std::env::temp_dir().join(format!("pf-cmd-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("slides")).unwrap();
        std::fs::write(dir.join("slides/lecture 1.pdf"), b"%PDF").unwrap();
        std::fs::write(dir.join("notes.txt"), b"ignored").unwrap();
        std::fs::write(dir.join("diagram.png"), b"png").unwrap();

        let top = attach_matches("/attach ", &dir);
        let labels: Vec<&str> = top.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["slides/", "diagram.png"]);

        let open_top = path_matches("/open ", &dir);
        let open_labels: Vec<&str> = open_top.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(open_labels, vec!["slides/", "diagram.png", "notes.txt"]);
        assert_eq!(
            complete_path("/open no", &dir).as_deref(),
            Some("/open notes.txt")
        );
        assert_eq!(
            complete_path("/check no", &dir).as_deref(),
            Some("/check notes.txt")
        );

        assert_eq!(
            complete_path("/attach dia", &dir).as_deref(),
            Some("/attach diagram.png")
        );
        assert_eq!(
            complete_path("/attach sl", &dir).as_deref(),
            Some("/attach slides/")
        );
        assert_eq!(
            complete_path("/attach slides/lec", &dir).as_deref(),
            Some("/attach \"slides/lecture 1.pdf\"")
        );
        assert_eq!(
            complete_path("/attach cl", &dir).as_deref(),
            Some("/attach clear")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
