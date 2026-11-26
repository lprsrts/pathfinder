//! The slash commands the terminal app knows: its own, and the task
//! commands the runtime runs (the same ones Zed offers).

/// A command the terminal app handles itself.
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
    Join,
    Data,
    Update,
    Attach,
    Share,
    Bug,
}

/// (name, other names, what it does), in the order help shows them.
pub const LOCAL: [(Local, &str, &[&str], &str); 14] = [
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
        "attach a PDF or image: /attach <file>",
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
pub const TASK: [(&str, &str); 15] = [
    ("/why", "your record, or what the current part rests on"),
    ("/map", "the plan as a diagram"),
    ("/plan", "build the plan now"),
    ("/summary", "explain the current part; a candidate follows"),
    ("/next", "move to the next open part"),
    ("/focus", "switch to a part: /focus <number or id>"),
    (
        "/known",
        "skip a part you already know (recorded as your claim)",
    ),
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
fn names() -> impl Iterator<Item = &'static str> {
    LOCAL
        .iter()
        .map(|(_, name, ..)| *name)
        .chain(TASK.iter().map(|(name, _)| *name))
}

/// The commands that start with what's typed (only while the first word is
/// still being typed).
pub fn matches(input: &str) -> Vec<&'static str> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let typed = input.to_ascii_lowercase();
    names().filter(|n| n.starts_with(&typed)).collect()
}

/// What Tab completes the input to: the one match, or the matches' common
/// beginning.
pub fn complete(input: &str) -> Option<String> {
    let found = matches(input);
    let first = *found.first()?;
    if found.len() == 1 {
        return Some(format!("{first} "));
    }
    let common: String = first
        .chars()
        .enumerate()
        .take_while(|(i, c)| found.iter().all(|n| n.chars().nth(*i) == Some(*c)))
        .map(|(_, c)| c)
        .collect();
    (common.len() > input.len()).then_some(common)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_aliases_are_recognised() {
        assert_eq!(parse("/quit"), Parsed::Local(Local::Quit, ""));
        assert_eq!(parse("/exit"), Parsed::Local(Local::Quit, ""));
        assert_eq!(parse("/Q"), Parsed::Local(Local::Quit, ""));
        assert_eq!(parse("/clear"), Parsed::Local(Local::New, ""));
        assert_eq!(
            parse("/bug it broke"),
            Parsed::Local(Local::Bug, "it broke")
        );
        assert_eq!(parse("/why"), Parsed::Task);
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
        assert!(matches("/quit now").is_empty());
        assert_eq!(matches("/s"), ["/status", "/share", "/summary"]);
    }
}
