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
        let reply = match session.advance() {
            Some(_) => format!("{lead}\n\n{}", self.enter_part(session, false)),
            None if session.mode == SessionMode::Done => format!(
                "{lead}\n\n{}",
                i18n::line(session.language.as_deref(), i18n::Line::AllResolved)
            ),
            None => lead,
        };
        session.push_turn("assistant", &reply);
        TurnOutput::changed(reply)
    }
}
