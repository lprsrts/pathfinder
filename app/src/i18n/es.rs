//! Spanish.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "Candidato para **{title}**:",
        IsItRight => "¿Es correcto? Di por qué.",
        PassedOneMore => "Aprobado. Uno más:",
        PartHint => {
            "Piensa en voz alta aquí: tu razonamiento mientras trabajas y luego la parte con tus propias palabras cuando estés listo. Si ya la conoces o quieres que la haga el agente, dilo sin más."
        }
        RestsOn => "Se basa en",
        ToldNotShown => "Registrado como dicho: {labels}.",
        GapNeeds => "Primero necesita **{concept}**: {why}.",
        GapClaimed => "dijiste que lo sabías, aún sin pruebas",
        GapDelegated => "lo delegaste sin entenderlo",
        GapShaky => "tu última comprobación falló",
        GapUncovered => "nada en el mapa lo cubre aún",
        NeededBy => "Necesario para {part}.",
        AddGapParts => "¿Añadir una parte para {names}?",
        And => " y ",
        ProposedChange => "Cambio de mapa propuesto:",
        AcceptReject => "Di sí para añadirlo, o no.",
        Needs => "necesita {deps}",
        FixedReply => "Eso es lo que estás juzgando. Primero tu veredicto.",
        GateCapped => {
            "(Comprobaciones compartidas agotadas por hoy; las respuestas sobre el candidato esperan tu veredicto. Clave propia: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => {
            "(Comprobación no disponible, respuesta retenida. Inténtalo de nuevo en un momento.)"
        }
        Understood => "**{title}** entendido. Dime cuándo quieres seguir o pasárselo al agente.",
        KeepWorking => "Sigue trabajando y luego explícalo otra vez.",
        AllResolved => "Todas las partes están resueltas.",
        NoOtherPart => "No queda otra parte sin resolver.",
        MapUpdated => "Mapa actualizado.",
        ChangeRejected => "Cambio de mapa rechazado.",
        NoChangeWaiting => "No hay ningún cambio de mapa pendiente.",
        RecordedKnown => "**{title}** registrado como ya sabido.",
        Delegated => "**{title}** delegado sin entenderlo; sin revisar.",
        QuickCheckLead => "Comprobación rápida:",
        ExamLead => "Práctica de examen, una pregunta cada vez.",
        ExamQuestionFor => "Pregunta sobre **{title}**:",
        ExamAnswer => "¿Tu respuesta?",
        RecordTitle => "**Tu registro en esta carpeta**",
        ToldOnly => "solo dicho",
        ReadOnly => "leído",
        ReadAt => "Leer:",
        Logged => "Anotado.",
        NoRecord => "Aún no hay registro.",
        RecordSummary => "Demostrado: {shown} · solo dicho: {told} · inseguro: {shaky}",
        Checks => "{passed} aprobadas, {failed} falladas",
        EvidenceNote => "Solo cuentan las comprobaciones aprobadas.",
        OpenMisconception => "Error de concepto abierto en {concept}: «{statement}»",
        WhereToLook => "**Dónde mirar:**",
        NotFoundOnline => "**No encontrado** aquí ni en línea.",
        NotFoundHere => "**No encontrado** aquí.",
        TryTerms => "Prueba: {terms}",
        NotSearched => "Sin buscar: {services}.",
        Told => "**Dicho**",
        FirstWrongStep => "**Primer paso que no se sostiene** ({at}):",
        NoWrongStep => "**Ningún paso incorrecto.**",
        CheckUnclear => {
            "No pude señalar un paso de tu trabajo. Escríbelo con un paso por línea y vuelve a usar `/check`."
        }
        StepAt => "línea {n}",
        PhotoUnreadable => "No se pudo leer la foto.",
        PhotoCopy => "Leído de {files}. Comprueba que esto es lo que escribiste:",
        FindHits => "**{n}** para {terms}:",
        FindNone => "Ninguna línea de esta carpeta contiene {terms}.",
        WebNone => "Nada en línea coincide con {terms}.",
        ShowKnown => {
            "Anotado: ya conoces **{title}**. Una afirmación, no una prueba. Di «deshacer» o pulsa Esc para retirarlo."
        }
        ShowTell => {
            "Te lo digo, como pediste. Anotado como dicho, nunca como entendido. Esc antes de que llegue lo cancela."
        }
        ShowHandoff => {
            "¿Pasar **{title}** al agente? Trabaja en una copia de staging y tú revisas el resultado. Enter ejecuta · Esc cancela."
        }
        ShowHandoffInPlace => {
            "¿Pasar **{title}** al agente? Esto no es un repositorio git: edita tus archivos directamente, sin staging ni revisión comprobada. Enter ejecuta · Esc cancela."
        }
        ShowDelegate => {
            "¿Delegar **{title}** sin entenderlo? Queda anotado como delegado: no respondes por el resultado. Enter ejecuta · Esc cancela."
        }
        ShowVerdict => {
            "¿Juzgar esto como tu veredicto sobre el candidato? «{words}» Enter juzga · Esc lo guarda como nota."
        }
        NotRunVerdict => "No juzgado como veredicto: guardado como nota.",
        NotRunHandoff => "No ejecutado: **{title}** no se pasó al agente.",
        NotRunDelegate => "No ejecutado: **{title}** no se delegó.",
        NotRunStale => "No ejecutado: lo que esperaba tu OK ya no aplica.",
        AskWhich => {
            "¿Qué quisiste decir: {choices}? No se anota nada hasta que lo digas; basta un número o unas palabras."
        }
        AskKnown => "anotar que ya conozco **{title}** (sin comprobación)",
        AskTell => "dímelo sin más",
        AskAgent => "que el agente haga esta parte",
        AskCheck => "ponme a prueba en **{title}**",
        AskNote => "solo una nota",
        AskWhichPart => "¿Qué parte quieres decir: {choices}? No se anota nada hasta que lo digas.",
        Or => " o ",
        Undone => {
            "Deshecho: tu afirmación sobre **{title}** queda retirada. El registro la guarda como retirada."
        }
        NothingToUndo => {
            "Nada que deshacer. Solo se puede retirar una afirmación de tu último paso. Un veredicto juzgado, una revisión, una entrega, una delegación y una respuesta que te dieron se quedan."
        }
        NothingWaiting => "Nada espera tu OK.",
        NextStep => "Siguiente: {at} · {question}",
        NextStepNoPlace => {
            "Siguiente: nada en tus fuentes nombra aún **{concept}**; pregúntame dónde buscar. · {question}"
        }
        StepPart => "¿Para qué es esta parte, con tus palabras?",
        StepWideExample => "Encuentra un sitio donde se use {concept}. ¿Qué hace allí?",
        StepWidePurpose => "¿Para qué sirve {concept} en esta parte, con tus palabras?",
        StepWideMechanism => "¿Cómo funciona {concept}, paso a paso?",
        StepNarrowExample => {
            "Elige una línea donde {concept} importe. ¿Qué cambiaría si fuera distinto?"
        }
        StepNarrowPurpose => "¿Por qué hace falta {concept} justo ahí, y qué se rompe sin él?",
        StepNarrowMechanism => "Sigue {concept} paso a paso: ¿qué pasa primero y qué después?",
        Horizon => "Horizonte: {what}.",
        HorizonNeeds => "necesita {parts}",
        HorizonLeads => "lleva a {parts}",
        EvidenceRatio => "Aprobados: {passes} · afirmaciones: {claims}.",
        ClaimWithdrawn => "Afirmación sobre **{concept}** retirada ({date}).",
        StepHistory => "Cómo llegaste aquí: {kinds}.",
        StepKindRate => "{kind} {passes} de {judged} llevaron a un aprobado",
        SteeredTimes => "guiado por ti {n} veces",
        StepHistoryNote => {
            "Es todo lo que Pathfinder guarda sobre cómo aprendes, en el registro local de esta carpeta."
        }
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "abierta",
        PartStatus::Understood => "entendida",
        PartStatus::InReview => "en revisión",
        PartStatus::Done => "entendida, hecha",
        PartStatus::Known => "declarada como sabida",
        PartStatus::Delegated => "delegada sin entenderla",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "desconocido",
        ConceptState::Claimed => "declarado",
        ConceptState::Delegated => "delegado",
        ConceptState::Shaky => "inseguro",
        ConceptState::Shown => "demostrado",
        ConceptState::Solid => "sólido",
    }
}
