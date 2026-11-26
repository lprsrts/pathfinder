//! Japanese.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "**{title}** の説明候補：",
        IsItRight => "正しいですか？理由も答えてください。",
        PassedOneMore => "合格です。もう一問：",
        PartHint => "ここで考えを書き出してください。`/summary` · `/known` · `/delegate`",
        RestsOn => "前提",
        ToldNotShown => "「教わった」として記録しました：{labels}。",
        GapNeeds => "先に **{concept}** が必要です：{why}。",
        GapClaimed => "わかると申告済みですが、まだ根拠がありません",
        GapDelegated => "理解しないまま任せました",
        GapShaky => "前回の確認で不合格でした",
        GapUncovered => "まだマップのどこにも含まれていません",
        NeededBy => "{part} に必要です。",
        AddGapParts => "{names} のパートを追加しますか？",
        And => "と",
        ProposedChange => "マップの変更案：",
        AcceptReject => "`/accept` または `/reject`。",
        Needs => "{deps} が必要",
        FixedReply => "それは今あなたが判定している内容です。まず判定をどうぞ。",
        GateCapped => {
            "（本日の共有チェックは上限に達しました。候補に関する返答は判定の後になります。自分のキー：`pathfinder auth set typesafe`。）"
        }
        GateUnavailable => {
            "（チェックに接続できないため返答を保留しました。少し待ってから再度お試しください。）"
        }
        Understood => "**{title}** を理解しました。`/next` · `/handoff`",
        KeepWorking => "作業を続けて、もう一度説明してください。",
        AllResolved => "すべてのパートが解決しました。",
        NoOtherPart => "未解決のパートはほかにありません。",
        MapUpdated => "マップを更新しました。",
        ChangeRejected => "マップの変更を却下しました。",
        NoChangeWaiting => "保留中のマップ変更はありません。",
        RecordedKnown => "**{title}** を既知として記録しました。",
        Delegated => "**{title}** を理解しないまま任せました。未確認です。",
        QuickCheckLead => "簡単な確認：",
        ExamLead => "試験の練習です。一問ずつ出します。",
        ExamQuestionFor => "**{title}** についての問題：",
        ExamAnswer => "あなたの答えは？",
        RecordTitle => "**このフォルダーでの記録**",
        ToldOnly => "教わっただけ",
        ReadOnly => "読んだ",
        ReadAt => "読む：",
        Logged => "記録しました。",
        NoRecord => "まだ記録がありません。",
        RecordSummary => "示した：{shown} · 教わっただけ：{told} · 不安定：{shaky}",
        Checks => "合格 {passed}、不合格 {failed}",
        EvidenceNote => "合格した確認だけが数えられます。",
        OpenMisconception => "{concept} についての未解決の誤解：「{statement}」",
        WhereToLook => "**参照先：**",
        NotFoundOnline => "**見つかりません**（ここにもオンラインにもありません）。",
        NotFoundHere => "**見つかりません**（ここにはありません）。",
        TryTerms => "検索候補：{terms}",
        NotSearched => "未検索：{services}。",
        Told => "**教えた内容**",
        FirstWrongStep => "**最初に成り立たないステップ**（{at}）：",
        NoWrongStep => "**成り立たないステップは見つかりませんでした。**",
        CheckUnclear => {
            "作業の中のステップを特定できませんでした。1行に1ステップずつ書いて、もう一度 `/check` してください。"
        }
        StepAt => "{n} 行目",
        PhotoUnreadable => "写真を読み取れませんでした。",
        PhotoCopy => "{files} から読み取りました。書いた内容と一致しているか確認してください：",
        FindHits => "**{n}** 件（{terms}）：",
        FindNone => "このフォルダーに {terms} を含む行はありません。",
        WebNone => "オンラインで {terms} に一致するものはありませんでした。",
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "未完了",
        PartStatus::Understood => "理解済み",
        PartStatus::InReview => "確認中",
        PartStatus::Done => "理解済み・完了",
        PartStatus::Known => "既知と申告",
        PartStatus::Delegated => "理解せず委任",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "不明",
        ConceptState::Claimed => "申告のみ",
        ConceptState::Delegated => "委任",
        ConceptState::Shaky => "不安定",
        ConceptState::Shown => "示した",
        ConceptState::Solid => "確実",
    }
}
