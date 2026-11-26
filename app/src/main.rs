mod acp;
mod agent;
mod auth;
mod bugreport;
mod catalog;
mod cli;
mod config;
mod crash;
mod decide;
mod domain;
mod engine;
mod envvar;
mod i18n;
mod ledger;
mod llm;
mod operator;
mod options;
mod paths;
mod providers;
mod runtime;
mod secrets;
mod sources;
mod store;
mod stream;
mod telemetry;
mod tui;
mod understanding;
mod update;
mod validate;
mod workspace;

use std::env;
use std::sync::Arc;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use crate::agent::ApiEngine;
use crate::auth::Credentials;
use crate::config::Config;
use crate::engine::cli::CliEngine;
use crate::engine::router::RouterEngine;
use crate::runtime::Runtime;
use crate::store::Store;

fn usage() -> String {
    format!(
        "Pathfinder {} makes sure you understand the work you do with AI.

Usage: pathfinder [command]

  pathfinder [tui] [folder]   Work in a folder (default: here)
  pathfinder auth login chatgpt
                              Sign in with your ChatGPT account (do this first)
  pathfinder auth [status|set|remove]
                              Your model accounts and API keys
  pathfinder data [status|show|join|on|off|delete]
                              Joining the beta and the data you share
  pathfinder bug [what happened]
                              Send a bug report (you see it all first)
  pathfinder update [--check] Install the newest release
  pathfinder paths            Where settings, data and logs are kept
  pathfinder help             This text

For developers:
  pathfinder acp              Run as an agent server over stdio (for Zed)
  pathfinder models [--refresh]
                              The models your keys can use
  pathfinder coverage [--export] [folder]
                              How much of the agent's changes you understood
  pathfinder eval [suite ...] [--out file]
                              Run the evals: {}
  pathfinder --version",
        env!("CARGO_PKG_VERSION"),
        runtime::eval::SUITES.join(", ")
    )
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    use std::process::ExitCode;
    let mut args: Vec<String> = env::args().skip(1).collect();
    // No command: work here, in the terminal.
    if args.is_empty() {
        args.push("tui".into());
    }
    match args[0].as_str() {
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            return ExitCode::SUCCESS;
        }
        "--version" | "-V" => {
            println!("pathfinder {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        "tui" => {
            use std::io::IsTerminal;
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                eprintln!("Pathfinder needs a terminal to run in. See `pathfinder help`.");
                return ExitCode::from(2);
            }
        }
        "acp" | "auth" | "data" | "bug" | "update" | "paths" | "models" | "coverage" | "eval" => {}
        other => {
            eprintln!("Unknown command `{other}`.\n\n{}", usage());
            return ExitCode::from(2);
        }
    }
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", explain(&e));
            ExitCode::FAILURE
        }
    }
}

/// An error as one plain sentence; the full chain with `PATHFINDER_DEBUG=1`.
fn explain(e: &anyhow::Error) -> String {
    if envvar::get_os("PATHFINDER_DEBUG").is_some() {
        return format!("Error: {e:?}");
    }
    let network = e.chain().find_map(|c| c.downcast_ref::<reqwest::Error>());
    if let Some(n) = network
        && (n.is_connect() || n.is_timeout())
    {
        let host = n
            .url()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "the server".into());
        return format!(
            "Couldn't reach {host}{}. Check your connection and try again.",
            if n.is_timeout() { " in time" } else { "" }
        );
    }
    let mut text = e.to_string();
    if let Some(cause) = e.chain().nth(1) {
        text.push_str(&format!(": {cause}"));
    }
    format!("{text}\n(More detail: PATHFINDER_DEBUG=1)")
}

async fn run(args: Vec<String>) -> Result<()> {
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // The TUI owns the terminal: its logs go to a file.
    if args.first().map(String::as_str) == Some("tui") {
        let log = bugreport::tui_log();
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        // Keep one older log, up to LOG_MAX each.
        const LOG_MAX: u64 = 5 << 20;
        if std::fs::metadata(&log).is_ok_and(|m| m.len() > LOG_MAX) {
            std::fs::rename(&log, log.with_extension("log.1")).ok();
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)?;
        tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .init();
    }

    link_new_name();
    match args.first().map(String::as_str) {
        Some("tui") => crash::install(pathfinder_schema::Surface::Tui, Some(tui::restore_terminal)),
        Some("acp") => crash::install(pathfinder_schema::Surface::Zed, None),
        _ => {}
    }

    // Zed's sign-in buttons re-run the agent command with extra arguments,
    // e.g. `pathfinder acp auth set`.
    let args: Vec<String> = match args.split_first() {
        Some((first, rest)) if first == "acp" && !rest.is_empty() => rest.to_vec(),
        _ => args,
    };
    match args.first().map(String::as_str) {
        Some("acp") => {
            let cfg = Config::load();
            let cfg_db = cfg.db_path.clone();
            let store = Arc::new(Store::open(&cfg.db_path)?);
            let creds = Arc::new(Credentials::system());
            let engine = Arc::new(RouterEngine::new(
                CliEngine::new(cfg),
                ApiEngine::with_credentials(creds.clone()),
            ));
            let runtime = Arc::new(
                Runtime::new(store, engine)
                    .with_decider(Arc::new(decide::SystemDecider::new(creds.clone()))),
            );
            start_telemetry(&cfg_db);
            let result = acp::run_stdio(runtime, creds).await;
            stop_telemetry().await;
            result
        }
        Some("tui") => {
            let cfg = Config::load();
            let cfg_db = cfg.db_path.clone();
            let store = Arc::new(Store::open(&cfg.db_path)?);
            let creds = Arc::new(Credentials::system());
            let engine = Arc::new(RouterEngine::new(
                CliEngine::new(cfg),
                ApiEngine::with_credentials(creds.clone()),
            ));
            let runtime = Arc::new(
                Runtime::new(store, engine)
                    .with_decider(Arc::new(decide::SystemDecider::new(creds.clone()))),
            );
            let cwd = match args.get(1) {
                Some(dir) => std::path::PathBuf::from(dir),
                None => env::current_dir()?,
            };
            start_telemetry(&cfg_db);
            let result = tui::run(runtime, cwd).await;
            stop_telemetry().await;
            result
        }
        Some("auth") => cli::auth(&args[1..]).await,
        Some("data") => cli::data(&args[1..]).await,
        Some("update") => update::run(&args[1..]).await,
        Some("bug") => cli::bug(&args[1..]).await,
        Some("paths") => {
            paths::print();
            Ok(())
        }
        Some("models") => cli::models(&args[1..]).await,
        Some("coverage") => cli::coverage(&args[1..]),
        Some("eval") => {
            // Live suites run in a throwaway database; the record suites
            // read your real one.
            let cfg = Config::load();
            let record = Store::open(&cfg.db_path)?;
            let scratch =
                env::temp_dir().join(format!("pathfinder-eval-{}.sqlite", std::process::id()));
            let store = Arc::new(Store::open(&scratch)?);
            let creds = Arc::new(Credentials::system());
            let engine = Arc::new(RouterEngine::new(
                CliEngine::new(cfg),
                ApiEngine::with_credentials(creds.clone()),
            ));
            let runtime = Runtime::new(store, engine)
                .with_decider(Arc::new(decide::SystemDecider::new(creds.clone())));
            let out = args
                .iter()
                .position(|a| a == "--out")
                .and_then(|i| args.get(i + 1).cloned());
            let suites: Vec<String> = args[1..]
                .iter()
                .filter(|a| !a.starts_with("--") && Some(*a) != out.as_ref())
                .cloned()
                .collect();
            let (report, ok) = runtime.run_evals(&suites, &record).await?;
            println!("{report}");
            if let Some(path) = out {
                std::fs::write(&path, &report)?;
                eprintln!("wrote {path}");
            }
            for ext in ["", "-shm", "-wal"] {
                std::fs::remove_file(format!("{}{ext}", scratch.display())).ok();
            }
            if !ok {
                anyhow::bail!("an eval missed its target");
            }
            Ok(())
        }
        _ => unreachable!("main checked the command"),
    }
}

/// Installs from before 0.3 put the program at `harness`, and updated it in
/// place under that name. Moves it to `pathfinder`, once, and leaves
/// `harness` as a link to it, so settings that still run `harness` (Zed's)
/// keep working.
fn link_new_name() {
    #[cfg(unix)]
    if let Ok(exe) = env::current_exe()
        && exe.file_name().is_some_and(|n| n == "harness")
        && std::fs::symlink_metadata(&exe).is_ok_and(|m| m.file_type().is_file())
    {
        let new = exe.with_file_name("pathfinder");
        // 0.3.0 left `pathfinder` as a link to `harness`: replace it.
        let new_is_link = std::fs::symlink_metadata(&new).is_ok_and(|m| m.file_type().is_symlink());
        if (new_is_link || !new.exists())
            && (!new_is_link || std::fs::remove_file(&new).is_ok())
            && std::fs::rename(&exe, &new).is_ok()
        {
            std::os::unix::fs::symlink("pathfinder", &exe).ok();
        }
    }
}

/// Opens the usage-event outbox and starts uploading, if you agreed to it.
fn start_telemetry(db: &std::path::Path) {
    if let Err(e) = telemetry::init(db) {
        tracing::warn!("usage events are off: {e:#}");
        return;
    }
    telemetry::spawn_uploader();
}

/// A last upload on the way out, bounded so quitting stays quick.
async fn stop_telemetry() {
    let _ = tokio::time::timeout(telemetry::FLUSH_ON_EXIT, telemetry::flush()).await;
}
