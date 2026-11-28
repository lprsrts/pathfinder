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
        PartHint => {
            "Pensa ad alta voce qui: il tuo ragionamento mentre lavori, poi la parte con parole tue quando sei pronto. Se la conosci già o vuoi che la faccia l'agente, dillo e basta."
        }
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
        AcceptReject => "Di' sì per aggiungerlo, oppure no.",
        Needs => "richiede {deps}",
        FixedReply => "È ciò che stai giudicando. Prima il tuo verdetto.",
        GateCapped => {
            "(Verifiche condivise esaurite per oggi; le risposte sul candidato aspettano il tuo verdetto. Chiave personale: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => {
            "(Verifica non raggiungibile, risposta trattenuta. Riprova tra un momento.)"
        }
        Understood => "**{title}** capito. Dimmi quando vuoi andare avanti o affidarlo all'agente.",
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
            "Annotato: conosci già **{title}**. Un'affermazione, non una prova. Di' «annulla» o premi Esc per ritirarla."
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
        DelegateWhy => {
            "Non è una consegna con la tua revisione: non hai ancora mostrato {concepts}."
        }
        ShowVerdict => {
            "Giudicare questo come tuo verdetto sul candidato? «{words}» Enter giudica · Esc lo tiene come nota."
        }
        ShowApprove => {
            "Approvare il risultato dell'agente per **{title}**? Invio approva · Esc annulla."
        }
        ShowObject => {
            "Rimandare **{title}** all'agente con la tua obiezione? “{words}” Invio la invia · Esc annulla."
        }
        NotRunVerdict => "Non giudicato come verdetto: tenuto come nota.",
        NotRunHandoff => "Non avviato: **{title}** non è stato affidato all'agente.",
        NotRunDelegate => "Non avviato: **{title}** non è stato delegato.",
        NotRunApprove => "Non eseguito: **{title}** non è stato approvato.",
        NotRunObject => "Non eseguito: la tua obiezione su **{title}** non è stata inviata.",
        NotRunStale => "Non avviato: ciò che aspettava il tuo OK non vale più.",
        AskWhich => {
            "Cosa intendevi: {choices}? Nulla viene annotato finché non lo dici; basta un numero o qualche parola."
        }
        AskAgentPart => "lasciare che l'agente faccia **{title}**",
        AskKnown => "annotare che conosco già **{title}** (senza verifica)",
        AskTell => "dimmelo e basta",
        AskAgent => "lascia che l'agente faccia questa parte",
        AskCheck => "mettimi alla prova su **{title}**",
        AskNote => "solo una nota",
        AskWhichPart => "Quale parte intendi: {choices}? Nulla viene annotato finché non lo dici.",
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
            "Prossimo: niente nelle tue fonti nomina ancora **{concept}**; chiedimi dove cercare. · {question}"
        }
        StepPart => "A che cosa serve questa parte, con parole tue?",
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
        EvidenceRatio => {
            "Delle idee qui sopra, {passes} hanno superato una verifica e {claims} poggiano solo sulla tua parola."
        }
        ClaimWithdrawn => "Affermazione su **{concept}** ritirata ({date}).",
        StepHistory => "Cosa ha portato a un superamento finora: {kinds}.",
        StepKindRate => "{kind}, {passes} volte su {judged}",
        SteeredTimes => "guidato da te {n} volte",
        AngleExample => "partire da un esempio",
        AnglePurpose => "chiedere a cosa serve",
        AngleMechanism => "seguire come funziona",
        AgentWork => "**Affidato all'agente**",
        AgentInReview => {
            "**{title}**: rivedibile, affidato dopo che hai mostrato su cosa poggia; attende la tua revisione."
        }
        AgentApproved => {
            "**{title}**: rivedibile, affidato dopo che hai mostrato su cosa poggia; l'hai approvato."
        }
        AgentMissedCheck => {
            "**{title}**: approvato senza cogliere la verifica; non conta come revisione, non ne rispondi."
        }
        AgentDelegated => "**{title}**: delegato, fatto senza che tu lo capissi; non ne rispondi.",
        AgentCaughtCheck => {
            "**{title}**: rivedibile; nella revisione hai colto il controllo inserito e hai obiettato, quindi è stato scritto il vero risultato dell'agente."
        }
        ShowAgentParts => {
            "Lasciare all'agente queste parti, una dopo l'altra? {parts}. Enter le avvia tutte · Esc annulla."
        }
        AgentPartHandoff => "**{title}** (affidamento: rivedi tu il risultato)",
        AgentPartDelegate => "**{title}** (delegato: non lo garantisci)",
        AgentPartDelegateWhy => "**{title}** (delegato: non hai ancora mostrato {concepts})",
        ConfirmHandoff => "sì, affidalo",
        ConfirmDelegate => "sì, delegalo",
        ConfirmVerdict => "sì, giudicalo",
        ConfirmApprove => "sì, approvalo",
        ConfirmObject => "sì, rimandalo",
        CancelShown => "no",
        ShowKnownShown => {
            "Annotato: conosci già **{title}**. Hai già mostrato ogni idea su cui poggia, quindi si regge sulle tue verifiche, non solo sulla tua parola. Di' «annulla» o premi Esc per ritirarla."
        }
        ShowKnownSome => {
            "Annotato: conosci già **{title}**. Solo la tua parola per {concepts}; il resto l'hai già mostrato. Di' «annulla» o premi Esc per ritirarla."
        }
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
