mod acp;
mod agent;
mod auth;
mod catalog;
mod cli;
mod config;
mod decide;
mod domain;
mod engine;
mod i18n;
mod ledger;
mod llm;
mod runtime;
mod secrets;
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

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // The TUI owns the terminal: its logs go to a file.
    if args.first().map(String::as_str) == Some("tui") {
        let log = Config::load()
            .db_path
            .parent()
            .map(|d| d.join("tui.log"))
            .unwrap_or_else(|| env::temp_dir().join("harness-tui.log"));
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir).ok();
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

    // Zed's sign-in buttons re-run the agent command with extra arguments,
    // e.g. `harness acp auth set`.
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
        Some("models") => cli::models(&args[1..]).await,
        Some("coverage") => cli::coverage(&args[1..]),
        Some("eval") => {
            // Live suites run in a throwaway database; the record suites
            // read your real one.
            let cfg = Config::load();
            let record = Store::open(&cfg.db_path)?;
            let scratch =
                env::temp_dir().join(format!("harness-eval-{}.sqlite", std::process::id()));
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
        Some("--version") | Some("-V") => {
            println!("harness {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            eprintln!(
                "harness {} — human-in-the-loop pathfinder over ACP\n\nUsage:\n  harness tui [folder]       Pathfinder in the terminal, in a folder (default: here)\n  harness acp                Run the ACP server over stdio (for Zed)\n  harness auth [status|set|remove|import]   Manage API keys\n  harness models [--refresh] List discovered models\n  harness data [status|show|join|on|off|delete]   Usage data you share (off unless you join)\n  harness update [--check]   Install the newest release\n  harness coverage [--export] [path]   Understanding coverage of the agent's changes\n  harness eval [suite ...] [--out FILE]   Run the A10 evals (leak, discrimination, checks, tagging, evidence, coverage)\n  harness --version          Print version",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
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
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), telemetry::flush()).await;
}
