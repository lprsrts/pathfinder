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
        ShowKnown => {
            "Annotato: conosci già **{title}**. Un'affermazione, non una prova. `/undo` (Esc) la annulla."
        }
        ShowTell => {
            "Te lo dico, come chiesto. Annotato come detto, mai come capito. Esc prima che arrivi annulla."
        }
        ShowHandoff => {
            "Affidare **{title}** all'agente? Lavora in una copia di staging e tu rivedi il risultato. Enter avvia · Esc annulla."
        }
        ShowHandoffInPlace => {
            "Affidare **{title}** all'agente? Questo non è un repository git: modifica i tuoi file direttamente, senza staging né revisione verificata. Enter avvia · Esc annulla."
        }
        ShowDelegate => {
            "Delegare **{title}** senza capirlo? Resta annotato come delegato: non garantisci il risultato. Enter avvia · Esc annulla."
        }
        ShowVerdict => {
            "Giudicare questo come tuo verdetto sul candidato? «{words}» Enter giudica · Esc lo tiene come nota."
        }
        NotRunVerdict => "Non giudicato come verdetto: tenuto come nota.",
        NotRunHandoff => "Non avviato: **{title}** non è stato affidato all'agente.",
        NotRunDelegate => "Non avviato: **{title}** non è stato delegato.",
        NotRunStale => "Non avviato: ciò che aspettava il tuo OK non vale più.",
        AskWhich => {
            "Intendevi {choices}? Nulla viene annotato finché non lo dici. Altrimenti continua pure."
        }
        AskKnown => "che conosci già **{title}** (`/known`)",
        AskTell => "che te lo dica (`/tell <question>`)",
        AskAgent => "affidare la parte all'agente (`/handoff` o `/delegate`)",
        Or => " o ",
        Undone => {
            "Annullato: la tua affermazione su **{title}** è ritirata. Il registro la tiene come ritirata."
        }
        NothingToUndo => {
            "Niente da annullare. Si può ritirare solo un'affermazione del tuo ultimo passo. Un verdetto giudicato, una revisione, un affidamento, una delega e una risposta che ti è stata data restano."
        }
        NothingWaiting => "Niente aspetta il tuo OK.",
        NextStep => "Prossimo: {at} · {question}",
        NextStepNoPlace => {
            "Prossimo: nessun punto delle tue fonti nomina ancora **{concept}**; `/find <terms>` le cerca. · {question}"
        }
        StepWideExample => "Trova un punto in cui si usa {concept}. Che cosa fa lì?",
        StepWidePurpose => "A che cosa serve {concept} in questa parte, con parole tue?",
        StepWideMechanism => "Come funziona {concept}, passo per passo?",
        StepNarrowExample => {
            "Scegli una riga in cui {concept} conta. Che cosa cambierebbe se fosse diverso?"
        }
        StepNarrowPurpose => "Perché serve {concept} proprio lì, e che cosa si rompe senza?",
        StepNarrowMechanism => "Segui {concept} passo per passo: che cosa succede prima, e poi?",
        Horizon => "Orizzonte: {what}.",
        HorizonNeeds => "richiede {parts}",
        HorizonLeads => "porta a {parts}",
        EvidenceRatio => "Superati: {passes} · affermazioni: {claims}.",
        ClaimWithdrawn => "Affermazione su **{concept}** ritirata ({date}).",
        StepHistory => "Come sei arrivato qui: {kinds}.",
        StepKindRate => "{kind} {passes} su {judged} hanno portato a un superamento",
        SteeredTimes => "guidato da te {n} volte",
        StepHistoryNote => {
            "È tutto ciò che Pathfinder tiene su come impari, nel registro locale di questa cartella."
        }
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
