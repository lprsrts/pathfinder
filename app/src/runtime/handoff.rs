//! §7 of docs/theory.md: handing a part to the agent and reviewing its work.

use super::*;

impl Runtime {
    pub(super) async fn handle_handoff(
        &self,
        session: &mut Session,
        instructions: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let Some(part) = session.active_part().cloned() else {
            return Ok(TurnOutput::text("No part is active."));
        };
        match part.status {
            PartStatus::Understood => {}
            PartStatus::Open => {
                return Ok(TurnOutput::text(
                    "Handoff comes after understanding: log, `/summary`, then judge the candidate. To hand it off without understanding, `/delegate`.",
                ));
            }
            other => {
                return Ok(TurnOutput::text(format!(
                    "**{}** is already {}.",
                    part.title,
                    other.label()
                )));
            }
        }
        // §13: in a git repository the agent works in a staging copy and
        // nothing reaches your files until your verdict.
        if let Some(root) = crate::ledger::repo_root(&session.cwd) {
            return self
                .handoff_staged(session, part, &root, instructions, files, sink)
                .await;
        }
        let before = self.snapshot_before(session);
        let result = self
            .run_agent(session, &part, true, instructions, files, sink, None)
            .await?;
        let changes = self
            .record_changes(session, &part, before, false, instructions)
            .await;
        if let Some(p) = session.map.get_mut(&part.id) {
            p.result = Some(result);
            p.status = PartStatus::InReview;
            p.unreviewed = changes;
        }
        Ok(TurnOutput::changed(REVIEW_PROMPT))
    }

    pub(super) async fn handle_object(
        &self,
        session: &mut Session,
        reason: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let Some(part) = session.active_part().cloned() else {
            return Ok(TurnOutput::text("No part is active."));
        };
        if part.status != PartStatus::InReview {
            return Ok(TurnOutput::text(
                "Nothing is waiting for your review on this part.",
            ));
        }
        if reason.is_empty() {
            return Ok(TurnOutput::text("Usage: `/object <what's wrong and why>`."));
        }
        if part.staging.is_some() {
            return self.object_staged(session, part, reason, files, sink).await;
        }
        self.record_review(
            session,
            &part.id,
            &part.unreviewed,
            "objected",
            Some(reason),
        );
        if let Some(p) = session.map.get_mut(&part.id) {
            p.objections.push(reason.to_string());
            p.unreviewed.clear();
        }
        let part = session.map.get(&part.id).cloned().unwrap();
        let before = self.snapshot_before(session);
        let result = self
            .run_agent(session, &part, true, "", files, sink, None)
            .await?;
        let changes = self.record_changes(session, &part, before, false, "").await;
        if let Some(p) = session.map.get_mut(&part.id) {
            p.result = Some(result);
            p.unreviewed = changes;
        }
        Ok(TurnOutput::text(REVIEW_PROMPT))
    }

    pub(super) async fn handle_approve(&self, session: &mut Session) -> Result<TurnOutput> {
        let Some(part) = session.active_part().cloned() else {
            return Ok(TurnOutput::text("No part is active."));
        };
        if part.status != PartStatus::InReview {
            return Ok(TurnOutput::text(
                "Nothing is waiting for your review on this part.",
            ));
        }
        if part.staging.is_some() {
            return self.approve_staged(session, part).await;
        }
        self.record_review(session, &part.id, &part.unreviewed, "approved", None);
        if let Some(p) = session.map.get_mut(&part.id) {
            p.unreviewed.clear();
        }
        self.set_status(session, &part.id, PartStatus::Done, "result approved")?;
        Ok(self.after_resolution(session, format!("**{}** done.", part.title)))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_agent(
        &self,
        session: &Session,
        part: &Part,
        understood: bool,
        instructions: &str,
        files: &[PathBuf],
        sink: StreamSink,
        staging: Option<&std::path::Path>,
    ) -> Result<String> {
        let mut prompt = format!(
            "Goal: {}\n\n{}\n\n{}",
            session.goal.as_deref().unwrap_or("(session goal)"),
            describe_part(part),
            if understood {
                "The user understands this part and will review your result."
            } else {
                "The user handed this part off without understanding it."
            }
        );
        if !part.objections.is_empty() {
            prompt.push_str(&format!(
                "\n\nYour previous result:\n{}\n\nThe user objected:\n- {}\n\nAddress every objection.",
                part.result.as_deref().unwrap_or("(none)"),
                part.objections.join("\n- ")
            ));
        }
        if !instructions.is_empty() {
            prompt.push_str(&format!("\n\nUser instructions: {instructions}"));
        }
        let mut ctx = crate::engine::turn_ctx();
        if staging.is_some() {
            prompt.push_str("\n\nYou are working in a copy of the user's workspace (your working directory). Edit and run things there as usual; the user reviews the result before it reaches their files.");
            // Edits stay in the copy: never through the client's files.
            ctx.host = ctx.host.map(review::without_files);
        }
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::Full,
            cwd: staging.map_or_else(|| session.cwd.clone(), std::path::Path::to_path_buf),
            system_prompt: AGENT_SYSTEM.to_string(),
            prompt,
            files: files.to_vec(),
            stream_text: true,
            models: self.models(session)?,
            ctx,
        };
        self.engine.run_text(&req, sink).await
    }
}
