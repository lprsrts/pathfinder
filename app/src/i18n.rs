//! The fixed lines Pathfinder writes itself, in the conversation's
//! language. Models write everything else in the user's language already;
//! these lines are the ones that would otherwise stay English.

use crate::understanding::ConceptState;

/// A line Pathfinder writes itself (English and German).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// After the fixed reply: the shared gate's daily checks are used up.
    GateCapped,
    /// After the fixed reply: the gate's service couldn't be reached.
    GateUnavailable,
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
    /// Heading of the folder-wide record.
    RecordTitle,
    /// A concept you were only told about.
    ToldOnly,
    ReadOnly,
    /// Before a part's pointer into the workspace.
    ReadAt,
    /// A log entry, taken without comment.
    Logged,
    /// The folder has no record yet.
    NoRecord,
    /// Under the record: "{shown}", "{told}", "{shaky}".
    RecordSummary,
    /// Checks on a concept: "{passed}", "{failed}".
    Checks,
    /// Under a part's `/why`.
    EvidenceNote,
    /// An open misconception: "{concept}", "{statement}".
    OpenMisconception,
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
            PassedOneMore => "Bestanden. Noch einer:",
            PartHint => "Denk hier laut. `/summary` · `/known` · `/delegate`",
            RestsOn => "Beruht auf",
            ToldNotShown => "Als gesagt vermerkt: {labels}.",
            GapNeeds => "Zuerst braucht es **{concept}**: {why}.",
            GapClaimed => "du hast angegeben, es zu können, aber noch nicht gezeigt",
            GapDelegated => "du hast es abgegeben, ohne es zu verstehen",
            GapShaky => "deine letzte Prüfung dazu ist danebengegangen",
            GapUncovered => "nichts auf der Karte deckt es bisher ab",
            NeededBy => "Gebraucht für {part}.",
            AddGapParts => "Einen Teil für {names} hinzufügen?",
            And => " und ",
            ProposedChange => "Vorgeschlagene Kartenänderung:",
            AcceptReject => "`/accept` oder `/reject`.",
            Needs => "braucht {deps}",
            FixedReply => "Das beurteilst du gerade. Erst dein Urteil.",
            GateCapped => {
                "(Geteilte Prüfungen für heute aufgebraucht; Antworten zum Kandidaten erst nach deinem Urteil. Eigener Schlüssel: `pathfinder auth set typesafe`.)"
            }
            GateUnavailable => {
                "(Prüfung nicht erreichbar, Antwort zurückgehalten. Versuch es gleich noch einmal.)"
            }
            Understood => "**{title}** verstanden. `/next` · `/handoff`",
            KeepWorking => "Arbeite weiter und erklär es dann noch einmal.",
            AllResolved => "Alle Teile sind erledigt.",
            NoOtherPart => "Kein weiterer offener Teil.",
            MapUpdated => "Karte aktualisiert.",
            ChangeRejected => "Kartenänderung abgelehnt.",
            NoChangeWaiting => "Es wartet keine Kartenänderung.",
            RecordedKnown => "**{title}** als schon bekannt vermerkt.",
            Delegated => "**{title}** ohne Verständnis abgegeben; nicht geprüft.",
            QuickCheckLead => "Kurze Prüfung:",
            ExamLead => "Prüfungssimulation, eine Frage nach der anderen.",
            ExamQuestionFor => "Prüfungsfrage zu",
            ExamAnswer => "Deine Antwort?",
            RecordTitle => "**Dein Verlauf in diesem Ordner**",
            Checks => "{passed} bestanden, {failed} nicht",
            EvidenceNote => "Nur bestandene Prüfungen zählen.",
            OpenMisconception => "Offenes Missverständnis bei {concept}: „{statement}“",
            ToldOnly => "nur gesagt",
            ReadOnly => "gelesen",
            ReadAt => "Lesen:",
            Logged => "Notiert.",
            NoRecord => "Noch kein Verlauf.",
            RecordSummary => "Gezeigt: {shown} · nur gesagt: {told} · wackelig: {shaky}",
        }
    } else {
        match line {
            CandidateFor => "Candidate for",
            IsItRight => "Is it right? Say why.",
            PassedOneMore => "Passed. One more:",
            PartHint => "Think out loud here. `/summary` · `/known` · `/delegate`",
            RestsOn => "Rests on",
            ToldNotShown => "Recorded as told: {labels}.",
            GapNeeds => "It needs **{concept}** first: {why}.",
            GapClaimed => "you claimed to know it, with no evidence yet",
            GapDelegated => "you handed it off without understanding it",
            GapShaky => "your last check on it failed",
            GapUncovered => "nothing on the map covers it yet",
            NeededBy => "Needed by {part}.",
            AddGapParts => "Add a part for {names}?",
            And => " and ",
            ProposedChange => "Proposed map change:",
            AcceptReject => "`/accept` or `/reject`.",
            Needs => "needs {deps}",
            FixedReply => crate::secrets::FIXED_REPLY,
            GateCapped => {
                "(Shared checks used up for today; replies about the candidate wait for your verdict. Own key: `pathfinder auth set typesafe`.)"
            }
            GateUnavailable => "(Check unreachable, reply held back. Try again in a moment.)",
            Understood => "**{title}** understood. `/next` · `/handoff`",
            KeepWorking => "Keep working, then explain it again.",
            AllResolved => "Every part is resolved.",
            NoOtherPart => "No other unresolved part.",
            MapUpdated => "Map updated.",
            ChangeRejected => "Map change rejected.",
            NoChangeWaiting => "No map change is waiting.",
            RecordedKnown => "**{title}** recorded as already known.",
            Delegated => "**{title}** handed off without understanding; not reviewed.",
            QuickCheckLead => "Quick check:",
            ExamLead => "Exam practice, one question at a time.",
            ExamQuestionFor => "Question on",
            ExamAnswer => "Your answer?",
            RecordTitle => "**Your record in this folder**",
            Checks => "{passed} passed, {failed} failed",
            EvidenceNote => "Only passed checks count.",
            OpenMisconception => "Open misconception on {concept}: \"{statement}\"",
            ToldOnly => "told only",
            ReadOnly => "read",
            ReadAt => "Read:",
            Logged => "Logged.",
            NoRecord => "No record yet.",
            RecordSummary => "Shown: {shown} · told only: {told} · shaky: {shaky}",
        }
    }
}

/// A part status's name in `lang`.
pub fn part_status(lang: Option<&str>, s: crate::domain::PartStatus) -> &'static str {
    use crate::domain::PartStatus::*;
    if !german(lang) {
        return s.label();
    }
    match s {
        Open => "offen",
        Understood => "verstanden",
        InReview => "Ergebnis zur Prüfung",
        Done => "erledigt",
        Known => "als bekannt angegeben",
        Delegated => "abgegeben",
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
