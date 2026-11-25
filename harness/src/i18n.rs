//! The fixed lines Pathfinder writes itself, in the conversation's
//! language. Models write everything else in the user's language already;
//! these lines are the ones that would otherwise stay English.

use crate::understanding::ConceptState;

/// A fixed line.
#[derive(Debug, Clone, Copy)]
pub enum Line {
    /// Before a candidate: "Candidate for **{title}**:".
    CandidateFor,
    /// After a candidate.
    IsItRight,
    /// A pass that doesn't settle the part yet.
    PassedOneMore,
    /// Under the first part's card.
    PartHint,
    /// Before the concepts a part rests on.
    RestsOn,
    /// After an answer that names concepts on the record: "{labels}".
    ToldNotShown,
    /// A gap under the part: "{concept}", "{why}".
    GapNeeds,
    GapClaimed,
    GapDelegated,
    GapShaky,
    GapUncovered,
    /// A proposed gap part's note: "{part}".
    NeededBy,
    /// A gap proposal's brief: "{names}".
    AddGapParts,
    /// Joins names in a list.
    And,
    /// Before a proposed map change.
    ProposedChange,
    /// After a proposed map change.
    AcceptReject,
    /// A proposed part's dependencies: "{deps}".
    Needs,
    /// The gate's reply when a draft can't be shown.
    FixedReply,
    /// A part judged understood: "{title}".
    Understood,
    /// A failed check.
    KeepWorking,
    AllResolved,
    NoOtherPart,
    MapUpdated,
    ChangeRejected,
    NoChangeWaiting,
    /// A part skipped as known: "{title}".
    RecordedKnown,
    /// A part delegated: "{title}".
    Delegated,
    /// Before a quick check's candidate.
    QuickCheckLead,
    /// Before an exam's first question.
    ExamLead,
    /// Before an exam question: "Question on **{title}**:".
    ExamQuestionFor,
    /// After an exam question.
    ExamAnswer,
}

fn german(lang: Option<&str>) -> bool {
    lang.is_some_and(|l| l.eq_ignore_ascii_case("de"))
}

/// The line in `lang` (a two-letter code); English when it's unknown.
pub fn line(lang: Option<&str>, line: Line) -> &'static str {
    use Line::*;
    if german(lang) {
        match line {
            CandidateFor => "Kandidat für",
            IsItRight => "Stimmt das? Sag, warum.",
            PassedOneMore => {
                "Bestanden. Dieser Teil beruht auf mehr, als diese Prüfung abgedeckt hat, also noch einer:"
            }
            PartHint => {
                "Denk hier laut, während du arbeitest: deine Überlegungen und Gründe sind das Protokoll. Wenn du den ganzen Teil erklären kannst, sag es und erklär ihn (`/summary`); sag, wenn du ihn schon kannst (`/known`); `/delegate` gibt ihn ab, ohne ihn zu verstehen."
            }
            RestsOn => "Beruht auf",
            ToldNotShown => "Gesagt, nicht gezeigt: {labels} steht damit in deinem Verlauf.",
            GapNeeds => "Zuerst braucht es **{concept}**: {why}.",
            GapClaimed => "du hast angegeben, es zu können, aber noch nicht gezeigt",
            GapDelegated => "du hast es abgegeben, ohne es zu verstehen",
            GapShaky => "deine letzte Prüfung dazu ist danebengegangen",
            GapUncovered => "nichts auf der Karte deckt es bisher ab",
            NeededBy => "Gebraucht für {part}.",
            AddGapParts => {
                "Einen kleinen Teil für {names} hinzufügen, damit dieser Teil nicht ungeprüft darauf beruht."
            }
            And => " und ",
            ProposedChange => "Vorgeschlagene Kartenänderung:",
            AcceptReject => "`/accept` oder `/reject`.",
            Needs => "braucht {deps}",
            FixedReply => {
                "Genau das beurteilst du gerade, deshalb kann ich es vor deinem Urteil nicht klären. Gib dein Urteil mit Begründung ab; was richtig oder falsch war, siehst du danach."
            }
            Understood => {
                "**{title}** verstanden. Sag, wenn es weitergehen soll (`/next`), oder `/handoff`, wenn dieser Teil Arbeit für den Agenten hat."
            }
            KeepWorking => {
                "Arbeite es hier weiter durch und erklär den Teil dann noch einmal für einen neuen Kandidaten."
            }
            AllResolved => "Alle Teile sind erledigt.",
            NoOtherPart => "Kein weiterer offener Teil.",
            MapUpdated => "Karte aktualisiert.",
            ChangeRejected => "Kartenänderung abgelehnt.",
            NoChangeWaiting => "Es wartet keine Kartenänderung.",
            RecordedKnown => "**{title}** als schon bekannt vermerkt.",
            Delegated => "**{title}** ohne Verständnis abgegeben; nicht geprüft.",
            QuickCheckLead => "Dann kurz zum Festigen:",
            ExamLead => {
                "Prüfungssimulation: eine Frage nach der anderen. Antworte so, wie du es in der Prüfung erklären würdest; mit „genug“ hörst du auf."
            }
            ExamQuestionFor => "Prüfungsfrage zu",
            ExamAnswer => "Deine Antwort?",
        }
    } else {
        match line {
            CandidateFor => "Candidate for",
            IsItRight => "Is it right? Say why.",
            PassedOneMore => {
                "Passed. This part rests on more than that check covered, so one more:"
            }
            PartHint => {
                "Talk it through here as you work: your thinking and reasons are the log. When you can explain the whole part, say so and explain it (`/summary`); say if you already know it (`/known`); `/delegate` hands it off without understanding it."
            }
            RestsOn => "Rests on",
            ToldNotShown => "Told, not shown: {labels} is on your record as told.",
            GapNeeds => "It needs **{concept}** first: {why}.",
            GapClaimed => "you claimed to know it, with no evidence yet",
            GapDelegated => "you handed it off without understanding it",
            GapShaky => "your last check on it failed",
            GapUncovered => "nothing on the map covers it yet",
            NeededBy => "Needed by {part}.",
            AddGapParts => {
                "Add a small part for {names} so this part doesn't rest on it unchecked."
            }
            And => " and ",
            ProposedChange => "Proposed map change:",
            AcceptReject => "`/accept` or `/reject`.",
            Needs => "needs {deps}",
            FixedReply => crate::secrets::FIXED_REPLY,
            Understood => {
                "**{title}** understood. Say when to move on (`/next`), or `/handoff` if this part has work for the agent."
            }
            KeepWorking => {
                "Keep working through it here, then explain the part again for a new candidate."
            }
            AllResolved => "Every part is resolved.",
            NoOtherPart => "No other unresolved part.",
            MapUpdated => "Map updated.",
            ChangeRejected => "Map change rejected.",
            NoChangeWaiting => "No map change is waiting.",
            RecordedKnown => "**{title}** recorded as already known.",
            Delegated => "**{title}** handed off without understanding; not reviewed.",
            QuickCheckLead => "Then a quick one to make it stick:",
            ExamLead => {
                "Exam practice, one question at a time. Answer the way you would in the exam; say \"enough\" to stop."
            }
            ExamQuestionFor => "Question on",
            ExamAnswer => "Your answer?",
        }
    }
}

/// A concept state's name in `lang`.
pub fn state(lang: Option<&str>, s: ConceptState) -> &'static str {
    if !german(lang) {
        return s.label();
    }
    match s {
        ConceptState::Unknown => "unbekannt",
        ConceptState::Claimed => "behauptet",
        ConceptState::Delegated => "delegiert",
        ConceptState::Shaky => "wackelig",
        ConceptState::Shown => "gezeigt",
        ConceptState::Solid => "sicher",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn german_when_asked_english_otherwise() {
        assert_eq!(line(Some("de"), Line::IsItRight), "Stimmt das? Sag, warum.");
        assert_eq!(line(Some("fr"), Line::IsItRight), "Is it right? Say why.");
        assert_eq!(line(None, Line::RestsOn), "Rests on");
        assert_eq!(state(Some("DE"), ConceptState::Shaky), "wackelig");
    }
}
