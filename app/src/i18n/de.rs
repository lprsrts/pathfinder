//! German.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "Kandidat für **{title}**:",
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
        ExamQuestionFor => "Prüfungsfrage zu **{title}**:",
        ExamAnswer => "Deine Antwort?",
        RecordTitle => "**Dein Verlauf in diesem Ordner**",
        ToldOnly => "nur gesagt",
        ReadOnly => "gelesen",
        ReadAt => "Lesen:",
        Logged => "Notiert.",
        NoRecord => "Noch kein Verlauf.",
        RecordSummary => "Gezeigt: {shown} · nur gesagt: {told} · wackelig: {shaky}",
        Checks => "{passed} bestanden, {failed} nicht",
        EvidenceNote => "Nur bestandene Prüfungen zählen.",
        OpenMisconception => "Offenes Missverständnis bei {concept}: „{statement}“",
        WhereToLook => "**Hier nachsehen:**",
        NotFoundOnline => "**Nicht gefunden**, weder hier noch online.",
        NotFoundHere => "**Nicht gefunden** in diesem Ordner.",
        TryTerms => "Such nach: {terms}",
        NotSearched => "Nicht durchsucht: {services}.",
        Told => "**Gesagt**",
        FirstWrongStep => "**Erster Schritt, der nicht stimmt** ({at}):",
        NoWrongStep => "**Kein Schritt gefunden, der nicht stimmt.**",
        CheckUnclear => {
            "Kein Schritt in deiner Arbeit zu finden. Schreib einen Schritt pro Zeile und dann noch einmal `/check`."
        }
        StepAt => "Zeile {n}",
        PhotoUnreadable => "Das Foto ließ sich nicht lesen.",
        PhotoCopy => "Gelesen aus {files}. Prüf, ob das dasteht, was du geschrieben hast:",
        FindHits => "**{n}** für {terms}:",
        FindNone => "Keine Zeile in diesem Ordner enthält {terms}.",
        WebNone => "Online passte nichts zu {terms}.",
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "offen",
        PartStatus::Understood => "verstanden",
        PartStatus::InReview => "Ergebnis zur Prüfung",
        PartStatus::Done => "erledigt",
        PartStatus::Known => "als bekannt angegeben",
        PartStatus::Delegated => "abgegeben",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "unbekannt",
        ConceptState::Claimed => "behauptet",
        ConceptState::Delegated => "delegiert",
        ConceptState::Shaky => "wackelig",
        ConceptState::Shown => "gezeigt",
        ConceptState::Solid => "sicher",
    }
}
