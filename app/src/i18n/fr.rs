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
        ShowKnown => {
            "Noté : tu connais déjà **{title}**. Une affirmation, pas une preuve. `/undo` (Esc) l'annule."
        }
        ShowTell => {
            "Je te le dis, comme demandé. Noté comme dit, jamais comme compris. Esc avant l'arrivée annule."
        }
        ShowHandoff => {
            "Confier **{title}** à l'agent ? Il travaille dans une copie de staging et tu relis le résultat. Enter lance · Esc annule."
        }
        ShowHandoffInPlace => {
            "Confier **{title}** à l'agent ? Ce n'est pas un dépôt git : il modifie tes fichiers directement, sans staging ni relecture vérifiée. Enter lance · Esc annule."
        }
        ShowDelegate => {
            "Déléguer **{title}** sans le comprendre ? Ce sera noté comme délégué : tu ne te portes pas garant du résultat. Enter lance · Esc annule."
        }
        ShowVerdict => {
            "Juger ceci comme ton verdict sur le candidat ? « {words} » Enter juge · Esc le garde comme note."
        }
        NotRunVerdict => "Pas jugé comme verdict : gardé comme note.",
        NotRunHandoff => "Non lancé : **{title}** n'a pas été confié à l'agent.",
        NotRunDelegate => "Non lancé : **{title}** n'a pas été délégué.",
        NotRunStale => "Non lancé : ce qui attendait ton accord ne s'applique plus.",
        AskWhich => {
            "Tu voulais dire {choices} ? Rien n'est noté tant que tu ne le dis pas. Sinon, continue."
        }
        AskKnown => "que tu connais déjà **{title}** (`/known`)",
        AskTell => "qu'on te le dise (`/tell <question>`)",
        AskAgent => "confier la partie à l'agent (`/handoff` ou `/delegate`)",
        Or => " ou ",
        Undone => {
            "Annulé : ton affirmation sur **{title}** est retirée. Le dossier la garde comme retirée."
        }
        NothingToUndo => {
            "Rien à annuler. Seule une affirmation de ta dernière étape peut être retirée. Un verdict jugé, une relecture, une remise, une délégation et une réponse qu'on t'a donnée restent."
        }
        NothingWaiting => "Rien n'attend ton accord.",
        NextStep => "Ensuite : {at} · {question}",
        NextStepNoPlace => {
            "Ensuite : aucun endroit de tes sources ne nomme encore **{concept}** ; `/find <terms>` les parcourt. · {question}"
        }
        StepWideExample => "Trouve un endroit où {concept} est utilisé. Qu'y fait-il ?",
        StepWidePurpose => "À quoi sert {concept} dans cette partie, avec tes mots ?",
        StepWideMechanism => "Comment fonctionne {concept}, étape par étape ?",
        StepNarrowExample => {
            "Choisis une ligne où {concept} compte. Que changerait-il s'il était différent ?"
        }
        StepNarrowPurpose => {
            "Pourquoi {concept} est-il nécessaire exactement là, et qu'est-ce qui casse sans lui ?"
        }
        StepNarrowMechanism => {
            "Suis {concept} étape par étape : que se passe-t-il d'abord, et ensuite ?"
        }
        Horizon => "Horizon : {what}.",
        HorizonNeeds => "dépend de {parts}",
        HorizonLeads => "mène à {parts}",
        EvidenceRatio => "Réussites : {passes} · affirmations : {claims}.",
        ClaimWithdrawn => "Affirmation sur **{concept}** retirée ({date}).",
        StepHistory => "Comment tu es arrivé ici : {kinds}.",
        StepKindRate => "{kind} {passes} sur {judged} ont mené à une réussite",
        SteeredTimes => "orienté par toi {n} fois",
        StepHistoryNote => {
            "C'est tout ce que Pathfinder garde sur ta façon d'apprendre, dans le dossier local de ce répertoire."
        }
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
