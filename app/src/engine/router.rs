//! Picks the engine for each request from the model's route.

use anyhow::Result;

use super::cli::CliEngine;
use super::{BoxFuture, Engine, EngineRequest, FinalCheck, StreamSink};
use crate::agent::ApiEngine;

pub struct RouterEngine {
    cli: CliEngine,
    api: ApiEngine,
}

impl RouterEngine {
    pub fn new(cli: CliEngine, api: ApiEngine) -> Self {
        Self { cli, api }
    }
}

impl Engine for RouterEngine {
    fn run_text<'a>(
        &'a self,
        req: &'a EngineRequest,
        sink: StreamSink,
    ) -> BoxFuture<'a, Result<String>> {
        if req.model().route.is_cli() {
            self.cli.run_text(req, sink)
        } else {
            self.api.run_text(req, sink)
        }
    }

    fn run_checked<'a>(
        &'a self,
        req: &'a EngineRequest,
        sink: StreamSink,
        check: FinalCheck<'a>,
    ) -> BoxFuture<'a, Result<String>> {
        if req.model().route.is_cli() {
            self.cli.run_text(req, sink)
        } else {
            self.api.run_checked(req, sink, check)
        }
    }
}
