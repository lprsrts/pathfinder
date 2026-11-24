//! Picks the engine for each request from the model's route.

use anyhow::Result;

use super::cli::CliEngine;
use super::{BoxFuture, Engine, EngineRequest, StreamSink};

pub struct RouterEngine {
    cli: CliEngine,
}

impl RouterEngine {
    pub fn new(cli: CliEngine) -> Self {
        Self { cli }
    }
}

impl Engine for RouterEngine {
    fn run_text<'a>(
        &'a self,
        req: &'a EngineRequest,
        sink: StreamSink,
    ) -> BoxFuture<'a, Result<String>> {
        self.cli.run_text(req, sink)
    }
}
