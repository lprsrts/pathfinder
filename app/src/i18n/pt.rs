//! Portuguese.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "Candidato para **{title}**:",
        IsItRight => "Está certo? Diga por quê.",
        PassedOneMore => "Aprovado. Mais um:",
        PartHint => "Pense em voz alta aqui. `/summary` · `/known` · `/delegate`",
        RestsOn => "Depende de",
        ToldNotShown => "Registrado como dito: {labels}.",
        GapNeeds => "Primeiro precisa de **{concept}**: {why}.",
        GapClaimed => "você disse que sabia, ainda sem evidência",
        GapDelegated => "você delegou sem entender",
        GapShaky => "sua última verificação falhou",
        GapUncovered => "nada no mapa cobre isso ainda",
        NeededBy => "Necessário para {part}.",
        AddGapParts => "Adicionar uma parte para {names}?",
        And => " e ",
        ProposedChange => "Mudança proposta no mapa:",
        AcceptReject => "`/accept` ou `/reject`.",
        Needs => "precisa de {deps}",
        FixedReply => "É isso que você está julgando. Primeiro o seu veredito.",
        GateCapped => {
            "(Verificações compartilhadas esgotadas por hoje; respostas sobre o candidato esperam o seu veredito. Chave própria: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => {
            "(Verificação indisponível, resposta retida. Tente de novo em instantes.)"
        }
        Understood => "**{title}** entendido. `/next` · `/handoff`",
        KeepWorking => "Continue trabalhando e depois explique de novo.",
        AllResolved => "Todas as partes estão resolvidas.",
        NoOtherPart => "Nenhuma outra parte pendente.",
        MapUpdated => "Mapa atualizado.",
        ChangeRejected => "Mudança no mapa rejeitada.",
        NoChangeWaiting => "Nenhuma mudança no mapa pendente.",
        RecordedKnown => "**{title}** registrado como já conhecido.",
        Delegated => "**{title}** delegado sem entender; não revisado.",
        QuickCheckLead => "Verificação rápida:",
        ExamLead => "Treino de prova, uma pergunta por vez.",
        ExamQuestionFor => "Pergunta sobre **{title}**:",
        ExamAnswer => "Sua resposta?",
        RecordTitle => "**Seu histórico nesta pasta**",
        ToldOnly => "só dito",
        ReadOnly => "lido",
        ReadAt => "Ler:",
        Logged => "Anotado.",
        NoRecord => "Ainda sem histórico.",
        RecordSummary => "Demonstrado: {shown} · só dito: {told} · instável: {shaky}",
        Checks => "{passed} aprovadas, {failed} reprovadas",
        EvidenceNote => "Só contam verificações aprovadas.",
        OpenMisconception => "Equívoco em aberto sobre {concept}: \"{statement}\"",
        WhereToLook => "**Onde procurar:**",
        NotFoundOnline => "**Não encontrado** aqui nem online.",
        NotFoundHere => "**Não encontrado** aqui.",
        TryTerms => "Tente: {terms}",
        NotSearched => "Não pesquisado: {services}.",
        Told => "**Dito**",
        FirstWrongStep => "**Primeiro passo que não se sustenta** ({at}):",
        NoWrongStep => "**Nenhum passo errado encontrado.**",
        CheckUnclear => {
            "Não consegui apontar um passo do seu trabalho. Escreva um passo por linha e use `/check` de novo."
        }
        StepAt => "linha {n}",
        PhotoUnreadable => "Não foi possível ler a foto.",
        PhotoCopy => "Lido de {files}. Confira se é isso que você escreveu:",
        FindHits => "**{n}** para {terms}:",
        FindNone => "Nenhuma linha desta pasta contém {terms}.",
        WebNone => "Nada online corresponde a {terms}.",
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "aberta",
        PartStatus::Understood => "entendida",
        PartStatus::InReview => "em revisão",
        PartStatus::Done => "entendida, concluída",
        PartStatus::Known => "declarada como conhecida",
        PartStatus::Delegated => "delegada sem entender",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "desconhecido",
        ConceptState::Claimed => "declarado",
        ConceptState::Delegated => "delegado",
        ConceptState::Shaky => "instável",
        ConceptState::Shown => "demonstrado",
        ConceptState::Solid => "sólido",
    }
}
