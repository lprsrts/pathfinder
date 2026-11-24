mod acp;
mod agent;
mod auth;
mod catalog;
mod cli;
mod config;
mod decide;
mod domain;
mod engine;
mod llm;
mod runtime;
mod secrets;
mod store;
mod stream;
mod understanding;
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
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let args: Vec<String> = env::args().skip(1).collect();
    // Zed's sign-in buttons re-run the agent command with extra arguments,
    // e.g. `harness acp auth set`.
    let args: Vec<String> = match args.split_first() {
        Some((first, rest)) if first == "acp" && !rest.is_empty() => rest.to_vec(),
        _ => args,
    };
    match args.first().map(String::as_str) {
        Some("acp") => {
            let cfg = Config::load();
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
            acp::run_stdio(runtime, creds).await
        }
        Some("auth") => cli::auth(&args[1..]).await,
        Some("models") => cli::models(&args[1..]).await,
        Some("--version") | Some("-V") => {
            println!("harness {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            eprintln!(
                "harness {} — human-in-the-loop pathfinder over ACP\n\nUsage:\n  harness acp                Run the ACP server over stdio (for Zed)\n  harness auth [status|set|remove|import]   Manage API keys\n  harness models [--refresh] List discovered models\n  harness --version          Print version",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}
