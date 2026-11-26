//! French.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "Candidat pour **{title}** :",
        IsItRight => "C'est juste ? Dis pourquoi.",
        PassedOneMore => "Réussi. Encore un :",
        PartHint => "Réfléchis à voix haute ici. `/summary` · `/known` · `/delegate`",
        RestsOn => "Repose sur",
        ToldNotShown => "Noté comme dit : {labels}.",
        GapNeeds => "Il faut d'abord **{concept}** : {why}.",
        GapClaimed => "tu as dit le savoir, sans preuve pour l'instant",
        GapDelegated => "tu l'as délégué sans le comprendre",
        GapShaky => "ta dernière vérification a échoué",
        GapUncovered => "rien sur la carte ne le couvre encore",
        NeededBy => "Nécessaire pour {part}.",
        AddGapParts => "Ajouter une partie pour {names} ?",
        And => " et ",
        ProposedChange => "Modification de carte proposée :",
        AcceptReject => "`/accept` ou `/reject`.",
        Needs => "nécessite {deps}",
        FixedReply => "C'est ce que tu es en train de juger. D'abord ton verdict.",
        GateCapped => {
            "(Vérifications partagées épuisées pour aujourd'hui ; les réponses sur le candidat attendent ton verdict. Clé personnelle : `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => "(Vérification injoignable, réponse retenue. Réessaie dans un instant.)",
        Understood => "**{title}** compris. `/next` · `/handoff`",
        KeepWorking => "Continue, puis explique-le à nouveau.",
        AllResolved => "Toutes les parties sont résolues.",
        NoOtherPart => "Aucune autre partie non résolue.",
        MapUpdated => "Carte mise à jour.",
        ChangeRejected => "Modification de carte refusée.",
        NoChangeWaiting => "Aucune modification de carte en attente.",
        RecordedKnown => "**{title}** noté comme déjà connu.",
        Delegated => "**{title}** délégué sans être compris ; non relu.",
        QuickCheckLead => "Vérification rapide :",
        ExamLead => "Entraînement à l'examen, une question à la fois.",
        ExamQuestionFor => "Question sur **{title}** :",
        ExamAnswer => "Ta réponse ?",
        RecordTitle => "**Ton historique dans ce dossier**",
        ToldOnly => "seulement dit",
        ReadOnly => "lu",
        ReadAt => "À lire :",
        Logged => "Noté.",
        NoRecord => "Pas encore d'historique.",
        RecordSummary => "Montré : {shown} · seulement dit : {told} · fragile : {shaky}",
        Checks => "{passed} réussies, {failed} échouées",
        EvidenceNote => "Seules les vérifications réussies comptent.",
        OpenMisconception => "Idée fausse en cours sur {concept} : « {statement} »",
        WhereToLook => "**Où regarder :**",
        NotFoundOnline => "**Introuvable** ici comme en ligne.",
        NotFoundHere => "**Introuvable** ici.",
        TryTerms => "Essaie : {terms}",
        NotSearched => "Non consulté : {services}.",
        Told => "**Dit**",
        FirstWrongStep => "**Première étape qui ne tient pas** ({at}) :",
        NoWrongStep => "**Aucune étape fausse trouvée.**",
        CheckUnclear => {
            "Impossible de désigner une étape de ton travail. Écris une étape par ligne, puis relance `/check`."
        }
        StepAt => "ligne {n}",
        PhotoUnreadable => "Impossible de lire la photo.",
        PhotoCopy => "Lu depuis {files}. Vérifie que c'est bien ce que tu as écrit :",
        FindHits => "**{n}** pour {terms} :",
        FindNone => "Aucune ligne de ce dossier ne contient {terms}.",
        WebNone => "Rien en ligne ne correspond à {terms}.",
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "ouverte",
        PartStatus::Understood => "comprise",
        PartStatus::InReview => "en relecture",
        PartStatus::Done => "comprise, terminée",
        PartStatus::Known => "déclarée connue",
        PartStatus::Delegated => "déléguée sans être comprise",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "inconnu",
        ConceptState::Claimed => "déclaré",
        ConceptState::Delegated => "délégué",
        ConceptState::Shaky => "fragile",
        ConceptState::Shown => "montré",
        ConceptState::Solid => "solide",
    }
}
