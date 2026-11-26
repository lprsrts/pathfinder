//! Chinese (Simplified).

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "关于 **{title}** 的候选解释：",
        IsItRight => "这对吗？说说理由。",
        PassedOneMore => "通过。再来一个：",
        PartHint => "在这里把思路说出来。`/summary` · `/known` · `/delegate`",
        RestsOn => "依赖于",
        ToldNotShown => "已记为“被告知”：{labels}。",
        GapNeeds => "需要先掌握 **{concept}**：{why}。",
        GapClaimed => "你说过你会，但还没有证据",
        GapDelegated => "你在没理解的情况下把它交出去了",
        GapShaky => "你上次对它的检查没通过",
        GapUncovered => "地图上还没有任何部分涵盖它",
        NeededBy => "{part} 需要它。",
        AddGapParts => "为 {names} 添加一个部分？",
        And => "和",
        ProposedChange => "建议的地图修改：",
        AcceptReject => "`/accept` 或 `/reject`。",
        Needs => "需要 {deps}",
        FixedReply => "这正是你在评判的内容。先给出你的判断。",
        GateCapped => {
            "（今天的共享检查次数已用完；关于候选解释的回复要等你给出判断。使用自己的密钥：`pathfinder auth set typesafe`。）"
        }
        GateUnavailable => "（无法连接检查服务，回复已暂缓。请稍后再试。）",
        Understood => "**{title}** 已理解。`/next` · `/handoff`",
        KeepWorking => "继续做，然后再解释一遍。",
        AllResolved => "所有部分都已解决。",
        NoOtherPart => "没有其他未解决的部分。",
        MapUpdated => "地图已更新。",
        ChangeRejected => "地图修改已拒绝。",
        NoChangeWaiting => "没有待处理的地图修改。",
        RecordedKnown => "**{title}** 已记为已掌握。",
        Delegated => "**{title}** 已在未理解的情况下交出；未经审查。",
        QuickCheckLead => "快速检查：",
        ExamLead => "考试练习，一次一个问题。",
        ExamQuestionFor => "关于 **{title}** 的问题：",
        ExamAnswer => "你的回答？",
        RecordTitle => "**你在此文件夹中的记录**",
        ToldOnly => "仅被告知",
        ReadOnly => "已读",
        ReadAt => "阅读：",
        Logged => "已记录。",
        NoRecord => "还没有记录。",
        RecordSummary => "已展示：{shown} · 仅被告知：{told} · 不牢固：{shaky}",
        Checks => "通过 {passed} 次，未通过 {failed} 次",
        EvidenceNote => "只有通过的检查才算数。",
        OpenMisconception => "关于 {concept} 的未解决误解：“{statement}”",
        WhereToLook => "**去哪里看：**",
        NotFoundOnline => "**未找到**，本地和网上都没有。",
        NotFoundHere => "**未找到**，本地没有。",
        TryTerms => "试试：{terms}",
        NotSearched => "未搜索：{services}。",
        Told => "**告知**",
        FirstWrongStep => "**第一个不成立的步骤**（{at}）：",
        NoWrongStep => "**没有找到不成立的步骤。**",
        CheckUnclear => "无法在你的作业中定位到某一步。请每行写一步，然后再用 `/check`。",
        StepAt => "第 {n} 行",
        PhotoUnreadable => "无法读取照片。",
        PhotoCopy => "读取自 {files}。请核对这是否是你写的内容：",
        FindHits => "**{n}** 条匹配 {terms}：",
        FindNone => "此文件夹中没有包含 {terms} 的行。",
        WebNone => "网上没有与 {terms} 匹配的内容。",
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "未完成",
        PartStatus::Understood => "已理解",
        PartStatus::InReview => "审查中",
        PartStatus::Done => "已理解，已完成",
        PartStatus::Known => "自称已掌握",
        PartStatus::Delegated => "未理解即交出",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "未知",
        ConceptState::Claimed => "自称",
        ConceptState::Delegated => "已交出",
        ConceptState::Shaky => "不牢固",
        ConceptState::Shown => "已展示",
        ConceptState::Solid => "牢固",
    }
}
