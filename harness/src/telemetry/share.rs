//! A thread you chose to share: its messages, with your folder and home
//! paths shortened, shown to you in full before anything is sent.

use std::path::Path;

use pathfinder_schema::SharedThread;
use serde_json::json;

/// Replaces the study folder with `.` and the home folder with `~`.
pub fn scrub_paths(text: &str, cwd: &Path) -> String {
    let mut out = text.to_string();
    let cwd = cwd.display().to_string();
    if cwd.len() > 1 {
        out = out.replace(&format!("{cwd}/"), "./").replace(&cwd, ".");
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy().to_string();
        if home.len() > 1 {
            out = out.replace(&home, "~");
        }
    }
    out
}

/// The thread to send: (role, text) pairs, paths scrubbed.
pub fn thread(id: &str, turns: &[(String, String)], cwd: &Path, note: &str) -> SharedThread {
    let messages: Vec<_> = turns
        .iter()
        .map(|(role, text)| json!({ "role": role, "text": scrub_paths(text, cwd) }))
        .collect();
    SharedThread {
        id: id.to_string(),
        app: env!("CARGO_PKG_VERSION").to_string(),
        note: scrub_paths(note.trim(), cwd),
        transcript: json!(messages),
    }
}

/// What the preview shows: exactly the text that will be sent.
pub fn preview(thread: &SharedThread) -> Vec<String> {
    let mut lines = Vec::new();
    if !thread.note.is_empty() {
        lines.push(format!("Your note: {}", thread.note));
        lines.push(String::new());
    }
    for m in thread.transcript.as_array().into_iter().flatten() {
        let who = match m["role"].as_str() {
            Some("user") => "You",
            _ => "Pathfinder",
        };
        lines.push(format!("{who}:"));
        for l in m["text"].as_str().unwrap_or("").lines() {
            lines.push(format!("  {l}"));
        }
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_shortened() {
        let cwd = Path::new("/Users/someone/Uni/EET");
        assert_eq!(
            scrub_paths(
                "see /Users/someone/Uni/EET/notes/ch3.pdf and /Users/someone/Uni/EET",
                cwd
            ),
            "see ./notes/ch3.pdf and ."
        );
    }

    #[test]
    fn the_preview_shows_every_message() {
        let t = thread(
            "0b5d0f7e-6c2a-4b8e-9f00-1a2b3c4d5e6f",
            &[
                ("user".into(), "hi\nthere".into()),
                ("assistant".into(), "hello".into()),
            ],
            Path::new("/tmp/x"),
            "odd reply",
        );
        let p = preview(&t);
        assert!(p.contains(&"  there".to_string()));
        assert!(p.contains(&"Pathfinder:".to_string()));
        assert_eq!(p[0], "Your note: odd reply");
    }
}
