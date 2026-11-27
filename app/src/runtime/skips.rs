//! §6 of docs/theory.md: the two kinds of skip, known and delegated.

use super::*;

impl Runtime {
    /// `then`: a part to go to afterwards ("I know this, take me to X"),
    /// instead of the next unresolved one.
    pub(super) fn handle_known(
        &self,
        session: &mut Session,
        then: Option<&str>,
    ) -> Result<TurnOutput> {
        Ok(self
            .record_known(session, then, i18n::Line::RecordedKnown)?
            .0)
    }

    /// `/known`, or a claim read from plain words (`lead` then shows it
    /// back, §15). Also returns the claim events, empty when nothing was
    /// recorded.
    pub(super) fn record_known(
        &self,
        session: &mut Session,
        then: Option<&str>,
        lead: i18n::Line,
    ) -> Result<(TurnOutput, Vec<(i64, String)>)> {
        let Some(part) = session.active_part().cloned() else {
            return Ok((TurnOutput::text("No part is active."), Vec::new()));
        };
        if part.status != PartStatus::Open {
            return Ok((
                TurnOutput::text(format!(
                    "**{}** is already {}.",
                    part.title,
                    part.status.label()
                )),
                Vec::new(),
            ));
        }
        session.pending_candidate = None;
        self.set_status(session, &part.id, PartStatus::Known, "claimed by the user")?;
        let events = self.record_skip(session, &part.id, EventKind::Claim);
        let lead = i18n::line(session.language.as_deref(), lead).replace("{title}", &part.title);
        let target = then
            .filter(|q| !q.is_empty() && *q != "next")
            .and_then(|q| session.map.resolve(q).ok())
            .filter(|id| *id != part.id);
        let Some(id) = target else {
            return Ok((self.after_resolution(session, lead), events));
        };
        session.active = Some(id);
        session.mode = SessionMode::Working;
        let reply = format!("{lead}\n\n{}", self.enter_part(session, false));
        session.push_turn("assistant", &reply);
        Ok((TurnOutput::text(reply), events))
    }

    pub(super) async fn handle_delegate(
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
            PartStatus::Open => {}
            PartStatus::Understood => {
                return Ok(TurnOutput::text(
                    "You understood this part, so `/handoff` it: you'll review the result.",
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
        session.pending_candidate = None;
        let before = self.snapshot_before(session);
        let result = self
            .run_agent(session, &part, false, instructions, files, sink, None)
            .await?;
        self.record_changes(session, &part, before, true, instructions)
            .await;
        if let Some(p) = session.map.get_mut(&part.id) {
            p.result = Some(result.clone());
        }
        self.set_status(session, &part.id, PartStatus::Delegated, &result)?;
        self.record_skip(session, &part.id, EventKind::Delegated);
        Ok(self.after_resolution(
            session,
            format!(
                "\n\n{}",
                i18n::line(session.language.as_deref(), i18n::Line::Delegated)
                    .replace("{title}", &part.title)
            ),
        ))
    }
}
