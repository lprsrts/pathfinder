//! A bug report: your note plus what helps find the bug, with paths
//! shortened. You see all of it before anything is sent; without a link to
//! the collector it is saved to a file for you to send yourself.

use std::path::{Path, PathBuf};

use pathfinder_schema::{Arch, BugReport, Os};

use crate::telemetry::share::scrub_paths;

/// Log lines included from the end of `tui.log`.
const LOG_LINES: usize = 60;

/// Where the TUI writes its log.
pub fn tui_log() -> PathBuf {
    crate::paths::tui_log(&crate::config::Config::load().db_path)
}

pub fn build(note: &str, cwd: &Path) -> BugReport {
    let cfg = crate::config::Config::load();
    let consent = crate::telemetry::Consent::load();
    let yes = |b: bool| if b { "yes" } else { "no" };
    let mut d = String::new();
    d.push_str(&format!(
        "Pathfinder {} on {} {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    d.push_str(&format!(
        "Models: fast {}, main {}\n",
        cfg.system1.as_deref().unwrap_or("default"),
        cfg.system2.as_deref().unwrap_or("default")
    ));
    d.push_str(&format!(
        "Sandbox: {}\n",
        match crate::agent::sandbox::available() {
            Some(crate::agent::sandbox::Kind::Seatbelt) => "Seatbelt",
            Some(crate::agent::sandbox::Kind::Bwrap(_)) => "bubblewrap",
            None => "none",
        }
    ));
    d.push_str(&format!(
        "Joined: {} · usage events: {} · thread sharing: {}\n",
        yes(consent.enrolled),
        yes(consent.events),
        yes(consent.threads)
    ));
    if let Some(crash) = crate::crash::recent() {
        d.push_str("\nLast crash (this week):\n");
        d.push_str(&tail(&crash, 40));
        d.push('\n');
    }
    if let Ok(log) = std::fs::read_to_string(tui_log()) {
        d.push_str(&format!("\nLast {LOG_LINES} log lines:\n"));
        d.push_str(&tail(&log, LOG_LINES));
        d.push('\n');
    }
    let mut details = scrub_paths(&d, cwd);
    if details.len() > BugReport::MAX_DETAILS {
        let cut = details.len() - BugReport::MAX_DETAILS;
        let cut = (cut..details.len())
            .find(|&i| details.is_char_boundary(i))
            .unwrap_or(details.len());
        details = details[cut..].to_string();
    }
    let mut note = scrub_paths(note.trim(), cwd);
    note.truncate(
        (0..=BugReport::MAX_NOTE.min(note.len()))
            .rev()
            .find(|&i| note.is_char_boundary(i))
            .unwrap_or(0),
    );
    BugReport {
        id: uuid::Uuid::new_v4().to_string(),
        app: env!("CARGO_PKG_VERSION").to_string(),
        os: Os::current(),
        arch: Arch::current(),
        note,
        details,
    }
}

fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Exactly what will be sent, line by line.
pub fn preview(report: &BugReport) -> Vec<String> {
    let mut lines = vec![
        format!(
            "What happened: {}",
            if report.note.is_empty() {
                "(no note: add one after /bug)"
            } else {
                &report.note
            }
        ),
        String::new(),
    ];
    lines.extend(report.details.lines().map(str::to_string));
    lines
}

/// Saves the report for sending by hand; returns the file.
pub fn save(report: &BugReport) -> std::io::Result<PathBuf> {
    let path = crate::paths::state_dir().join(format!("bug-report-{}.txt", &report.id[..8]));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, format!("{}\n\n{}\n", report.note, report.details))?;
    Ok(path)
}
