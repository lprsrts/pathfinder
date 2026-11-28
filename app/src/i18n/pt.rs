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
        PartHint => {
            "Pense em voz alta aqui: seu raciocínio enquanto trabalha e, quando estiver pronto, a parte com suas próprias palavras. Se já a conhece ou quer que o agente a faça, é só dizer."
        }
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
        AcceptReject => "Diga sim para adicionar, ou não.",
        Needs => "precisa de {deps}",
        FixedReply => "É isso que você está julgando. Primeiro o seu veredito.",
        GateCapped => {
            "(Verificações compartilhadas esgotadas por hoje; respostas sobre o candidato esperam o seu veredito. Chave própria: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => {
            "(Verificação indisponível, resposta retida. Tente de novo em instantes.)"
        }
        Understood => {
            "**{title}** entendido. Diga quando quiser seguir em frente ou passá-lo ao agente."
        }
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
        ShowKnown => {
            "Registrado: você já conhece **{title}**. Uma afirmação, não uma evidência. Diga \"desfazer\" ou aperte Esc para retirar."
        }
        ShowTell => {
            "Vou te dizer, como pediu. Registrado como dito, nunca como entendido. Esc antes de chegar cancela."
        }
        ShowHandoff => {
            "Passar **{title}** para o agente? Ele trabalha numa cópia de staging e você revisa o resultado. Enter executa · Esc cancela."
        }
        ShowHandoffInPlace => {
            "Passar **{title}** para o agente? Isto não é um repositório git: ele edita seus arquivos diretamente, sem staging nem revisão verificada. Enter executa · Esc cancela."
        }
        ShowDelegate => {
            "Delegar **{title}** sem entender? Fica registrado como delegado: você não responde pelo resultado. Enter executa · Esc cancela."
        }
        DelegateWhy => "Não é uma entrega com sua revisão: você ainda não mostrou {concepts}.",
        ShowVerdict => {
            "Julgar isto como seu veredito sobre o candidato? “{words}” Enter julga · Esc guarda como nota."
        }
        ShowApprove => {
            "Aprovar o resultado do agente para **{title}**? Enter aprova · Esc cancela."
        }
        ShowObject => {
            "Devolver **{title}** ao agente com sua objeção? “{words}” Enter envia · Esc cancela."
        }
        NotRunVerdict => "Não julgado como veredito: guardado como nota.",
        NotRunHandoff => "Não executado: **{title}** não foi passado ao agente.",
        NotRunDelegate => "Não executado: **{title}** não foi delegado.",
        NotRunApprove => "Não executado: **{title}** não foi aprovado.",
        NotRunObject => "Não executado: sua objeção sobre **{title}** não foi enviada.",
        NotRunStale => "Não executado: o que esperava seu OK não se aplica mais.",
        AskWhich => {
            "O que você quis dizer: {choices}? Nada é registrado até você dizer; um número ou algumas palavras bastam."
        }
        AskAgentPart => "deixar o agente fazer **{title}**",
        AskKnown => "registrar que já conheço **{title}** (sem verificação)",
        AskTell => "só me diga",
        AskAgent => "deixe o agente fazer esta parte",
        AskCheck => "me teste em **{title}**",
        AskNote => "só uma nota",
        AskWhichPart => "Qual parte você quer dizer: {choices}? Nada é registrado até você dizer.",
        Or => " ou ",
        Undone => {
            "Desfeito: sua afirmação sobre **{title}** foi retirada. O registro a mantém como retirada."
        }
        NothingToUndo => {
            "Nada a desfazer. Só uma afirmação do seu último passo pode ser retirada. Um veredito julgado, uma revisão, uma entrega, uma delegação e uma resposta que te deram ficam como estão."
        }
        NothingWaiting => "Nada espera seu OK.",
        NextStep => "Próximo: {at} · {question}",
        NextStepNoPlace => {
            "Próximo: nada nas suas fontes cita **{concept}** ainda; pergunte-me onde procurar. · {question}"
        }
        StepPart => "Para que serve esta parte, com suas palavras?",
        StepWideExample => "Ache um lugar onde {concept} é usado. O que ele faz ali?",
        StepWidePurpose => "Para que serve {concept} nesta parte, com suas palavras?",
        StepWideMechanism => "Como {concept} funciona, passo a passo?",
        StepNarrowExample => {
            "Escolha uma linha onde {concept} importa. O que mudaria se fosse diferente?"
        }
        StepNarrowPurpose => {
            "Por que {concept} é necessário exatamente ali, e o que quebra sem ele?"
        }
        StepNarrowMechanism => "Siga {concept} passo a passo: o que acontece primeiro, e depois?",
        Horizon => "Horizonte: {what}.",
        HorizonNeeds => "precisa de {parts}",
        HorizonLeads => "leva a {parts}",
        EvidenceRatio => {
            "Das ideias acima, {passes} passaram numa verificação e {claims} se apoiam só na sua palavra."
        }
        ClaimWithdrawn => "Afirmação sobre **{concept}** retirada ({date}).",
        StepHistory => "O que levou a uma aprovação até agora: {kinds}.",
        StepKindRate => "{kind}, {passes} de {judged} vezes",
        SteeredTimes => "guiado por você {n} vezes",
        AngleExample => "começar por um exemplo",
        AnglePurpose => "perguntar para que serve",
        AngleMechanism => "seguir como funciona",
        AgentWork => "**Entregue ao agente**",
        AgentInReview => {
            "**{title}**: revisável, entregue depois que você mostrou em que se apoia; aguarda sua revisão."
        }
        AgentApproved => {
            "**{title}**: revisável, entregue depois que você mostrou em que se apoia; você aprovou."
        }
        AgentMissedCheck => {
            "**{title}**: aprovado sem perceber a verificação; não conta como revisão, você não responde por ele."
        }
        AgentDelegated => {
            "**{title}**: delegado, feito sem que você entendesse; você não responde por ele."
        }
        AgentCaughtCheck => {
            "**{title}**: revisável; na revisão você pegou a verificação plantada e objetou, então o resultado real do agente foi gravado."
        }
        ShowAgentParts => {
            "Deixar o agente fazer estas partes, uma após a outra? {parts}. Enter executa todas · Esc cancela."
        }
        AgentPartHandoff => "**{title}** (entrega: você revisa o resultado)",
        AgentPartDelegate => "**{title}** (delegado: você não responde por ele)",
        AgentPartDelegateWhy => "**{title}** (delegado: você ainda não mostrou {concepts})",
        ConfirmHandoff => "sim, entregue",
        ConfirmDelegate => "sim, delegue",
        ConfirmVerdict => "sim, julgue",
        ConfirmApprove => "sim, aprove",
        ConfirmObject => "sim, devolva",
        CancelShown => "não",
        ShowKnownShown => {
            "Registrado: você já conhece **{title}**. Você já mostrou cada ideia em que ele se apoia, então ele se sustenta nas suas verificações, não só na sua palavra. Diga \"desfazer\" ou aperte Esc para retirar."
        }
        ShowKnownSome => {
            "Registrado: você já conhece **{title}**. Só a sua palavra para {concepts}; o resto você já mostrou. Diga \"desfazer\" ou aperte Esc para retirar."
        }
        StepHistoryNote => {
            "É tudo o que o Pathfinder guarda sobre como você aprende, no registro local desta pasta."
        }
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
