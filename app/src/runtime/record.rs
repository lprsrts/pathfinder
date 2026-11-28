//! §8 of docs/theory.md: the record.

use super::*;

impl Runtime {
    pub(super) fn set_status(
        &self,
        session: &mut Session,
        part_id: &str,
        status: PartStatus,
        notes: &str,
    ) -> Result<()> {
        let Some(part) = session.map.get_mut(part_id) else {
            return Ok(());
        };
        part.status = status;
        let title = part.title.clone();
        if let Some(record) = status.record() {
            self.store.record_outcome(
                &session.id,
                part_id,
                &title,
                record,
                status.label(),
                Some(notes),
            )?;
        }
        Ok(())
    }

    pub(super) fn after_resolution(&self, session: &mut Session, lead: String) -> TurnOutput {
        self.resolved_then(session, lead, None)
    }

    /// `after_resolution` once the agent did a part (`ran`: the concepts
    /// it rests on): the next part raises only the gaps that run opened.
    pub(super) fn after_agent_run(
        &self,
        session: &mut Session,
        lead: String,
        ran: &[String],
    ) -> TurnOutput {
        self.resolved_then(session, lead, Some(ran))
    }

    fn resolved_then(
        &self,
        session: &mut Session,
        lead: String,
        ran: Option<&[String]>,
    ) -> TurnOutput {
        let reply = match session.advance() {
            Some(_) => format!("{lead}\n\n{}", self.enter_part_after(session, false, ran)),
            None if session.mode == SessionMode::Done => format!(
                "{lead}\n\n{}",
                i18n::all_resolved(session.language.as_deref(), &session.map)
            ),
            None => lead,
        };
        session.push_turn("assistant", &reply);
        TurnOutput::text(reply)
    }
}
