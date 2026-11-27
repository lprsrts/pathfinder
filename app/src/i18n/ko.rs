//! Korean.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "**{title}** 설명 후보:",
        IsItRight => "맞나요? 이유도 말해 주세요.",
        PassedOneMore => "통과했어요. 하나 더:",
        PartHint => "여기서 생각을 적어 가며 풀어 보세요. `/summary` · `/known` · `/delegate`",
        RestsOn => "기반",
        ToldNotShown => "들은 것으로 기록했어요: {labels}.",
        GapNeeds => "먼저 **{concept}**이(가) 필요해요: {why}.",
        GapClaimed => "안다고 했지만 아직 근거가 없어요",
        GapDelegated => "이해하지 않고 넘겼어요",
        GapShaky => "지난 확인을 통과하지 못했어요",
        GapUncovered => "아직 지도에서 다루는 부분이 없어요",
        NeededBy => "{part}에 필요해요.",
        AddGapParts => "{names} 파트를 추가할까요?",
        And => ", ",
        ProposedChange => "지도 변경 제안:",
        AcceptReject => "`/accept` 또는 `/reject`.",
        Needs => "필요: {deps}",
        FixedReply => "그건 지금 판정하고 있는 내용이에요. 판정부터 해 주세요.",
        GateCapped => {
            "(오늘 공유 확인 횟수를 다 썼어요. 후보에 관한 답변은 판정 후에 드려요. 개인 키: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => {
            "(확인 서비스에 연결할 수 없어 답변을 보류했어요. 잠시 후 다시 시도해 주세요.)"
        }
        Understood => "**{title}** 이해 완료. `/next` · `/handoff`",
        KeepWorking => "계속 작업한 뒤 다시 설명해 보세요.",
        AllResolved => "모든 파트가 해결됐어요.",
        NoOtherPart => "해결되지 않은 다른 파트가 없어요.",
        MapUpdated => "지도를 업데이트했어요.",
        ChangeRejected => "지도 변경을 거절했어요.",
        NoChangeWaiting => "대기 중인 지도 변경이 없어요.",
        RecordedKnown => "**{title}**을(를) 이미 아는 것으로 기록했어요.",
        Delegated => "**{title}**을(를) 이해하지 않고 넘겼어요. 검토되지 않았어요.",
        QuickCheckLead => "빠른 확인:",
        ExamLead => "시험 연습, 한 번에 한 문제씩.",
        ExamQuestionFor => "**{title}**에 관한 문제:",
        ExamAnswer => "답은요?",
        RecordTitle => "**이 폴더의 기록**",
        ToldOnly => "들은 것만",
        ReadOnly => "읽음",
        ReadAt => "읽기:",
        Logged => "기록했어요.",
        NoRecord => "아직 기록이 없어요.",
        RecordSummary => "보여 줌: {shown} · 들은 것만: {told} · 불안정: {shaky}",
        Checks => "통과 {passed}, 실패 {failed}",
        EvidenceNote => "통과한 확인만 인정돼요.",
        OpenMisconception => "{concept}에 대한 해결되지 않은 오해: \"{statement}\"",
        WhereToLook => "**찾아볼 곳:**",
        NotFoundOnline => "**찾지 못했어요.** 여기에도 온라인에도 없어요.",
        NotFoundHere => "**찾지 못했어요.** 여기에는 없어요.",
        TryTerms => "검색해 보기: {terms}",
        NotSearched => "검색하지 않음: {services}.",
        Told => "**알려 준 답**",
        FirstWrongStep => "**처음으로 성립하지 않는 단계** ({at}):",
        NoWrongStep => "**성립하지 않는 단계를 찾지 못했어요.**",
        CheckUnclear => {
            "작업에서 단계를 찾지 못했어요. 한 줄에 한 단계씩 쓰고 다시 `/check` 해 주세요."
        }
        StepAt => "{n}번째 줄",
        PhotoUnreadable => "사진을 읽지 못했어요.",
        PhotoCopy => "{files}에서 읽었어요. 쓴 내용과 같은지 확인해 주세요:",
        FindHits => "**{n}**건 ({terms}):",
        FindNone => "이 폴더에는 {terms}이(가) 들어 있는 줄이 없어요.",
        WebNone => "온라인에서 {terms}와(과) 일치하는 결과가 없어요.",
        ShowKnown => {
            "기록했어요: **{title}**을(를) 이미 안다고요. 근거가 아니라 주장이에요. `/undo` (Esc)로 되돌릴 수 있어요."
        }
        ShowTell => {
            "요청대로 알려 드릴게요. 이해가 아니라 '들음'으로 기록돼요. 답이 오기 전에 Esc를 누르면 취소돼요."
        }
        ShowHandoff => {
            "**{title}**을(를) 에이전트에게 넘길까요? 스테이징 사본에서 작업하고 결과는 직접 검토해요. Enter 실행 · Esc 취소."
        }
        ShowHandoffInPlace => {
            "**{title}**을(를) 에이전트에게 넘길까요? git 저장소가 아니라서 스테이징이나 확인된 검토 없이 파일을 바로 고쳐요. Enter 실행 · Esc 취소."
        }
        ShowDelegate => {
            "**{title}**을(를) 이해하지 않고 넘길까요? 넘김으로 기록되고, 결과를 보증하지 않게 돼요. Enter 실행 · Esc 취소."
        }
        ShowVerdict => {
            "이것을 후보에 대한 판정으로 채점할까요? “{words}” Enter 채점 · Esc 메모로 남기기."
        }
        NotRunVerdict => "판정으로 채점하지 않고 메모로 남겼어요.",
        NotRunHandoff => "실행 안 됨: **{title}**을(를) 에이전트에게 넘기지 않았어요.",
        NotRunDelegate => "실행 안 됨: **{title}**을(를) 넘기지 않았어요.",
        NotRunStale => "실행 안 됨: 승인을 기다리던 일이 더 이상 해당되지 않아요.",
        AskWhich => {
            "{choices} 뜻이었나요? 말해 주기 전까지는 아무것도 기록하지 않아요. 아니면 그냥 계속하세요."
        }
        AskKnown => "**{title}**을(를) 이미 안다는 (`/known`)",
        AskTell => "답을 알려 달라는 (`/tell <question>`)",
        AskAgent => "이 부분을 에이전트에게 맡기라는 (`/handoff` 또는 `/delegate`)",
        Or => " 또는 ",
        Undone => "되돌렸어요: **{title}**에 대한 주장을 철회했어요. 기록에는 철회로 남아요.",
        NothingToUndo => {
            "되돌릴 것이 없어요. 직전 단계의 주장만 철회할 수 있어요. 채점된 판정, 검토, 넘김, 이해 없는 넘김, 들은 답은 그대로 남아요."
        }
        NothingWaiting => "승인을 기다리는 것이 없어요.",
        NextStep => "다음: {at} · {question}",
        NextStepNoPlace => {
            "다음: 자료에 아직 **{concept}**이(가) 나오는 곳이 없어요. `/find <terms>`로 찾아볼 수 있어요. · {question}"
        }
        StepWideExample => "{concept}이(가) 쓰이는 곳을 하나 찾아보세요. 거기서 무엇을 하나요?",
        StepWidePurpose => "이 부분에서 {concept}은(는) 무엇을 위한 건가요? 자기 말로요.",
        StepWideMechanism => "{concept}은(는) 어떻게 작동하나요? 한 단계씩요.",
        StepNarrowExample => "{concept}이(가) 중요한 줄을 하나 고르세요. 다르면 무엇이 바뀔까요?",
        StepNarrowPurpose => "{concept}이(가) 왜 바로 거기에 필요하고, 없으면 무엇이 깨지나요?",
        StepNarrowMechanism => {
            "{concept}을(를) 한 단계씩 따라가 보세요. 먼저 무엇이, 다음에 무엇이 일어나나요?"
        }
        Horizon => "전망: {what}.",
        HorizonNeeds => "필요: {parts}",
        HorizonLeads => "이어짐: {parts}",
        EvidenceRatio => "통과: {passes} · 주장: {claims}.",
        ClaimWithdrawn => "**{concept}**에 대한 주장 철회 ({date}).",
        StepHistory => "여기까지 온 길: {kinds}.",
        StepKindRate => "{kind} {judged}번 중 {passes}번 통과로 이어짐",
        SteeredTimes => "직접 방향을 정한 횟수 {n}",
        StepHistoryNote => {
            "Pathfinder가 학습 방식에 대해 남기는 건 이것뿐이며, 이 폴더의 로컬 기록에 있어요."
        }
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "진행 중",
        PartStatus::Understood => "이해함",
        PartStatus::InReview => "검토 중",
        PartStatus::Done => "이해함, 완료",
        PartStatus::Known => "안다고 표시함",
        PartStatus::Delegated => "이해 없이 넘김",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "모름",
        ConceptState::Claimed => "주장함",
        ConceptState::Delegated => "넘김",
        ConceptState::Shaky => "불안정",
        ConceptState::Shown => "보여 줌",
        ConceptState::Solid => "확실",
    }
}
