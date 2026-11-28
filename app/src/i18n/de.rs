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
        PartHint => {
            "Denk hier laut: deine Überlegungen beim Arbeiten, dann den Teil in deinen eigenen Worten, wenn du so weit bist. Wenn du ihn schon kennst oder der Agent ihn machen soll, sag es einfach."
        }
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
        AcceptReject => "Sag ja, um es zu übernehmen, oder nein.",
        Needs => "braucht {deps}",
        FixedReply => "Das beurteilst du gerade. Erst dein Urteil.",
        GateCapped => {
            "(Geteilte Prüfungen für heute aufgebraucht; Antworten zum Kandidaten erst nach deinem Urteil. Eigener Schlüssel: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => {
            "(Prüfung nicht erreichbar, Antwort zurückgehalten. Versuch es gleich noch einmal.)"
        }
        Understood => {
            "**{title}** verstanden. Sag, wenn du weitermachen oder ihn an den Agenten geben willst."
        }
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
        ShowKnown => {
            "Vermerkt: Du kennst **{title}** schon. Eine Behauptung, kein Nachweis. Sag „rückgängig“ oder drück Esc, um es zurückzunehmen."
        }
        ShowTell => {
            "Ich sag's dir, wie gewünscht. Vermerkt als gesagt, nie als verstanden. Esc, bevor es da ist, bricht ab."
        }
        ShowHandoff => {
            "**{title}** an den Agenten geben? Er arbeitet in einer Staging-Kopie, und du prüfst das Ergebnis. Enter startet · Esc bricht ab."
        }
        ShowHandoffInPlace => {
            "**{title}** an den Agenten geben? Das ist kein Git-Repository: Er ändert deine Dateien direkt, ohne Staging und ohne geprüftes Review. Enter startet · Esc bricht ab."
        }
        ShowDelegate => {
            "**{title}** abgeben, ohne es zu verstehen? Es wird als abgegeben vermerkt: Für das Ergebnis bürgst du nicht. Enter startet · Esc bricht ab."
        }
        DelegateWhy => "Keine Übergabe mit deiner Prüfung: Du hast {concepts} noch nicht gezeigt.",
        ShowVerdict => {
            "Das als dein Urteil über den Kandidaten werten? „{words}“ Enter wertet · Esc behält es als Notiz."
        }
        ShowApprove => {
            "Das Ergebnis des Agenten für **{title}** annehmen? Enter nimmt an · Esc bricht ab."
        }
        ShowObject => {
            "**{title}** mit deinem Einwand an den Agenten zurückgeben? „{words}“ Enter sendet · Esc bricht ab."
        }
        NotRunVerdict => "Nicht als Urteil gewertet: als Notiz behalten.",
        NotRunHandoff => "Nicht ausgeführt: **{title}** ging nicht an den Agenten.",
        NotRunDelegate => "Nicht ausgeführt: **{title}** wurde nicht abgegeben.",
        NotRunApprove => "Nicht ausgeführt: **{title}** wurde nicht angenommen.",
        NotRunObject => "Nicht ausgeführt: Dein Einwand zu **{title}** wurde nicht gesendet.",
        NotRunStale => "Nicht ausgeführt: Was auf dein OK wartete, gilt nicht mehr.",
        AskWhich => {
            "Was meintest du: {choices}? Nichts wird vermerkt, bis du es sagst; eine Zahl oder ein paar Worte reichen."
        }
        AskAgentPart => "den Agenten **{title}** machen lassen",
        AskKnown => "vermerken, dass ich **{title}** schon kenne (ohne Prüfung)",
        AskTell => "sag es mir einfach",
        AskAgent => "lass den Agenten diesen Teil machen",
        AskCheck => "prüf mich zu **{title}**",
        AskNote => "nur eine Notiz",
        AskWhichPart => "Welchen Teil meinst du: {choices}? Nichts wird vermerkt, bis du es sagst.",
        Or => " oder ",
        Undone => {
            "Rückgängig: Deine Behauptung zu **{title}** ist zurückgezogen. Der Verlauf hält sie als zurückgezogen fest."
        }
        NothingToUndo => {
            "Nichts rückgängig zu machen. Nur eine Behauptung aus deinem letzten Schritt lässt sich zurücknehmen. Ein gewertetes Urteil, ein Review, eine Übergabe, eine Abgabe und eine Antwort, die du dir sagen ließt, bleiben."
        }
        NothingWaiting => "Nichts wartet auf dein OK.",
        NextStep => "Als Nächstes: {at} · {question}",
        NextStepNoPlace => {
            "Als Nächstes: Noch nichts in deinen Quellen nennt **{concept}**; frag mich, wo du suchen sollst. · {question}"
        }
        StepPart => "Wozu ist dieser Teil da, in deinen Worten?",
        StepWideExample => "Finde eine Stelle, an der {concept} vorkommt. Was bewirkt es dort?",
        StepWidePurpose => "Wozu dient {concept} in diesem Teil, in deinen Worten?",
        StepWideMechanism => "Wie funktioniert {concept}, Schritt für Schritt?",
        StepNarrowExample => {
            "Nimm eine Zeile, in der {concept} zählt. Was änderte sich, wäre es anders?"
        }
        StepNarrowPurpose => "Warum braucht es {concept} genau dort, und was bricht ohne?",
        StepNarrowMechanism => {
            "Verfolge {concept} Schritt für Schritt: Was passiert zuerst, was danach?"
        }
        Horizon => "Horizont: {what}.",
        HorizonNeeds => "braucht {parts}",
        HorizonLeads => "führt zu {parts}",
        EvidenceRatio => {
            "Von den Ideen oben: {passes} in einer Prüfung bestanden, {claims} stützen sich nur auf dein Wort."
        }
        ClaimWithdrawn => "Behauptung zu **{concept}** zurückgezogen ({date}).",
        StepHistory => "Was bisher zum Bestehen führte: {kinds}.",
        StepKindRate => "{kind}, {passes} von {judged} Mal",
        SteeredTimes => "{n}-mal von dir gelenkt",
        AngleExample => "mit einem Beispiel anfangen",
        AnglePurpose => "fragen, wofür es da ist",
        AngleMechanism => "nachvollziehen, wie es funktioniert",
        AgentWork => "**An den Agenten übergeben**",
        AgentInReview => {
            "**{title}**: prüfbar, übergeben, nachdem du gezeigt hast, worauf es beruht; wartet auf deine Prüfung."
        }
        AgentApproved => {
            "**{title}**: prüfbar, übergeben, nachdem du gezeigt hast, worauf es beruht; du hast es angenommen."
        }
        AgentMissedCheck => {
            "**{title}**: angenommen, ohne die Prüfung zu bemerken; zählt nicht als Prüfung, du bürgst nicht dafür."
        }
        AgentDelegated => {
            "**{title}**: delegiert, ohne dein Verständnis erledigt; du bürgst nicht dafür."
        }
        AgentCaughtCheck => {
            "**{title}**: prüfbar; in der Prüfung hast du den eingebauten Fehler bemerkt und widersprochen, also wurde das echte Ergebnis des Agenten geschrieben."
        }
        ShowAgentParts => {
            "Soll der Agent diese Teile nacheinander machen? {parts}. Enter startet alle · Esc bricht ab."
        }
        AgentPartHandoff => "**{title}** (Übergabe: du prüfst das Ergebnis)",
        AgentPartDelegate => "**{title}** (abgegeben: du bürgst nicht dafür)",
        AgentPartDelegateWhy => "**{title}** (abgegeben: du hast {concepts} noch nicht gezeigt)",
        ConfirmHandoff => "ja, übergib es",
        ConfirmDelegate => "ja, delegiere es",
        ConfirmVerdict => "ja, werte es",
        ConfirmApprove => "ja, nimm es an",
        ConfirmObject => "ja, gib es zurück",
        CancelShown => "nein",
        ShowKnownShown => {
            "Vermerkt: Du kennst **{title}** schon. Jede Idee, auf der es beruht, hast du schon gezeigt: Es stützt sich auf deine Prüfungen, nicht nur auf dein Wort. Sag „rückgängig“ oder drück Esc, um es zurückzunehmen."
        }
        ShowKnownSome => {
            "Vermerkt: Du kennst **{title}** schon. Nur dein Wort für {concepts}; den Rest hast du schon gezeigt. Sag „rückgängig“ oder drück Esc, um es zurückzunehmen."
        }
        StepHistoryNote => {
            "Mehr hält Pathfinder nicht darüber fest, wie du lernst; es bleibt im lokalen Verlauf dieses Ordners."
        }
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
