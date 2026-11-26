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
        PartHint => "Piensa en voz alta aquí. `/summary` · `/known` · `/delegate`",
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
        AcceptReject => "`/accept` o `/reject`.",
        Needs => "necesita {deps}",
        FixedReply => "Eso es lo que estás juzgando. Primero tu veredicto.",
        GateCapped => {
            "(Comprobaciones compartidas agotadas por hoy; las respuestas sobre el candidato esperan tu veredicto. Clave propia: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => {
            "(Comprobación no disponible, respuesta retenida. Inténtalo de nuevo en un momento.)"
        }
        Understood => "**{title}** entendido. `/next` · `/handoff`",
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
