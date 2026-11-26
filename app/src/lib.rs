//! Pathfinder core library: session loop, epistemic gate, understanding model,
//! provenance ledger, source verification, model routing, and command metadata.

pub mod agent;
pub mod auth;
pub mod bugreport;
pub mod catalog;
pub mod commands;
pub mod config;
pub mod crash;
pub mod decide;
pub mod domain;
pub mod engine;
pub mod envvar;
pub mod i18n;
pub mod ledger;
pub mod llm;
pub mod operator;
pub mod options;
pub mod paths;
pub mod providers;
pub mod runtime;
pub mod secrets;
pub mod sources;
pub mod store;
pub mod stream;
pub mod telemetry;
pub mod understanding;
pub mod validate;
pub mod web;
pub mod workspace;

/// Opens a URL in the default browser, unless `PATHFINDER_NO_BROWSER` is set
/// (e.g. over SSH), in which case the printed URL is used instead.
pub fn open_browser(url: &str) {
    if envvar::get_os("PATHFINDER_NO_BROWSER").is_some() {
        return;
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
