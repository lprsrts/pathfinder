mod acp;
mod config;
mod critic;
mod domain;
mod engine;
mod runtime;
mod store;
mod workspace;

use std::env;
use std::sync::Arc;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::engine::PiEngine;
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
    match args.first().map(String::as_str) {
        Some("acp") => {
            let cfg = Config::load();
            let store = Arc::new(Store::open(&cfg.db_path)?);
            let engine = Arc::new(PiEngine::new(cfg));
            let runtime = Arc::new(Runtime::new(store, engine));
            acp::run_stdio(runtime).await
        }
        Some("--version") | Some("-V") => {
            println!("harness {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            eprintln!(
                "harness {} — human-in-the-loop pathfinder over ACP\n\nUsage:\n  harness acp        Run ACP server over stdio (for Zed)\n  harness --version  Print version",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}
