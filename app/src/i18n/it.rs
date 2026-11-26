//! Italian.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "Candidato per **{title}**:",
        IsItRight => "È giusto? Di' perché.",
        PassedOneMore => "Superato. Un altro:",
        PartHint => "Pensa ad alta voce qui. `/summary` · `/known` · `/delegate`",
        RestsOn => "Si basa su",
        ToldNotShown => "Registrato come detto: {labels}.",
        GapNeeds => "Prima serve **{concept}**: {why}.",
        GapClaimed => "hai detto di saperlo, ancora senza prove",
        GapDelegated => "l'hai delegato senza capirlo",
        GapShaky => "la tua ultima verifica non è andata",
        GapUncovered => "niente sulla mappa lo copre ancora",
        NeededBy => "Serve a {part}.",
        AddGapParts => "Aggiungere una parte per {names}?",
        And => " e ",
        ProposedChange => "Modifica proposta alla mappa:",
        AcceptReject => "`/accept` o `/reject`.",
        Needs => "richiede {deps}",
        FixedReply => "È ciò che stai giudicando. Prima il tuo verdetto.",
        GateCapped => {
            "(Verifiche condivise esaurite per oggi; le risposte sul candidato aspettano il tuo verdetto. Chiave personale: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => {
            "(Verifica non raggiungibile, risposta trattenuta. Riprova tra un momento.)"
        }
        Understood => "**{title}** capito. `/next` · `/handoff`",
        KeepWorking => "Continua a lavorare, poi spiegalo di nuovo.",
        AllResolved => "Tutte le parti sono risolte.",
        NoOtherPart => "Nessun'altra parte da risolvere.",
        MapUpdated => "Mappa aggiornata.",
        ChangeRejected => "Modifica alla mappa rifiutata.",
        NoChangeWaiting => "Nessuna modifica alla mappa in attesa.",
        RecordedKnown => "**{title}** registrato come già noto.",
        Delegated => "**{title}** delegato senza capirlo; non rivisto.",
        QuickCheckLead => "Verifica rapida:",
        ExamLead => "Simulazione d'esame, una domanda alla volta.",
        ExamQuestionFor => "Domanda su **{title}**:",
        ExamAnswer => "La tua risposta?",
        RecordTitle => "**Il tuo registro in questa cartella**",
        ToldOnly => "solo detto",
        ReadOnly => "letto",
        ReadAt => "Leggi:",
        Logged => "Annotato.",
        NoRecord => "Ancora nessun registro.",
        RecordSummary => "Dimostrato: {shown} · solo detto: {told} · incerto: {shaky}",
        Checks => "{passed} superate, {failed} non superate",
        EvidenceNote => "Contano solo le verifiche superate.",
        OpenMisconception => "Idea sbagliata aperta su {concept}: «{statement}»",
        WhereToLook => "**Dove guardare:**",
        NotFoundOnline => "**Non trovato** né qui né online.",
        NotFoundHere => "**Non trovato** qui.",
        TryTerms => "Prova: {terms}",
        NotSearched => "Non cercato: {services}.",
        Told => "**Detto**",
        FirstWrongStep => "**Primo passaggio che non regge** ({at}):",
        NoWrongStep => "**Nessun passaggio sbagliato trovato.**",
        CheckUnclear => {
            "Non riesco a indicare un passaggio del tuo lavoro. Scrivi un passaggio per riga e usa di nuovo `/check`."
        }
        StepAt => "riga {n}",
        PhotoUnreadable => "Impossibile leggere la foto.",
        PhotoCopy => "Letto da {files}. Controlla che sia ciò che hai scritto:",
        FindHits => "**{n}** per {terms}:",
        FindNone => "Nessuna riga in questa cartella contiene {terms}.",
        WebNone => "Niente online corrisponde a {terms}.",
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "aperta",
        PartStatus::Understood => "capita",
        PartStatus::InReview => "in revisione",
        PartStatus::Done => "capita, conclusa",
        PartStatus::Known => "dichiarata nota",
        PartStatus::Delegated => "delegata senza capirla",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "sconosciuto",
        ConceptState::Claimed => "dichiarato",
        ConceptState::Delegated => "delegato",
        ConceptState::Shaky => "incerto",
        ConceptState::Shown => "dimostrato",
        ConceptState::Solid => "solido",
    }
}
