//! What happens when Pathfinder panics: the terminal is put back, the
//! details go to `crash.log` on this computer, and only the fact that it
//! crashed (no message, no backtrace) is counted, if you share usage events.

use std::io::Write;
use std::path::PathBuf;

use pathfinder_schema::Surface;

pub fn log_path() -> PathBuf {
    crate::paths::state_dir().join("crash.log")
}

/// Installs the panic hook. `restore_terminal` runs first when the panic is
/// on the main thread (the TUI's loop), so the shell is usable again.
pub fn install(surface: Surface, restore_terminal: Option<fn()>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let main = std::thread::current().name() == Some("main");
        if main && let Some(restore) = restore_terminal {
            restore();
        }
        let backtrace = std::backtrace::Backtrace::force_capture();
        let path = log_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(
                f,
                "=== Pathfinder {} at {}\n{info}\n{backtrace}\n",
                env!("CARGO_PKG_VERSION"),
                unix_now()
            );
        }
        tracing::error!("panic: {info}");
        crate::telemetry::try_emit(pathfinder_schema::Event::Crash { surface });
        if main {
            eprintln!(
                "\nPathfinder crashed. The details are in {}.\n\
                 Please report it: `pathfinder bug \"what you were doing\"` (you see it all before it's sent).",
                path.display()
            );
            previous(info);
        }
    }));
}

/// The last crash's details, if one happened in the last week.
pub fn recent() -> Option<String> {
    let path = log_path();
    let age = std::fs::metadata(&path)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()?;
    if age.as_secs() > 7 * 86_400 {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let last = text.rsplit("=== Pathfinder ").next()?;
    Some(format!("=== Pathfinder {}", last.trim_end()))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
