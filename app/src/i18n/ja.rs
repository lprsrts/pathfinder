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
        PartHint => {
            "ここで考えを書き出してください。作業しながらの考えと、準備ができたらこのパートを自分の言葉で。もう知っている、またはエージェントに任せたいなら、そう言ってください。"
        }
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
        AcceptReject => "追加するなら「はい」、しないなら「いいえ」と言ってください。",
        Needs => "{deps} が必要",
        FixedReply => "それは今あなたが判定している内容です。まず判定をどうぞ。",
        GateCapped => {
            "（本日の共有チェックは上限に達しました。候補に関する返答は判定の後になります。自分のキー：`pathfinder auth set typesafe`。）"
        }
        GateUnavailable => {
            "（チェックに接続できないため返答を保留しました。少し待ってから再度お試しください。）"
        }
        Understood => {
            "**{title}** を理解しました。先に進むときやエージェントに任せるときは言ってください。"
        }
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
        ShowKnown => {
            "記録しました: **{title}** はもう知っている、と。根拠ではなく申告です。「取り消して」と言うか Esc で取り消せます。"
        }
        ShowTell => {
            "ご要望どおりお答えします。理解ではなく「教わった」と記録されます。届く前に Esc で取り消せます。"
        }
        ShowHandoff => {
            "**{title}** をエージェントに渡しますか？ステージング用のコピーで作業し、結果はあなたが確認します。Enter で実行 · Esc で取消。"
        }
        ShowHandoffInPlace => {
            "**{title}** をエージェントに渡しますか？ここは git リポジトリではないので、ステージングも確認付きレビューもなくファイルを直接編集します。Enter で実行 · Esc で取消。"
        }
        ShowDelegate => {
            "**{title}** を理解しないまま任せますか？委任として記録され、結果を保証することにはなりません。Enter で実行 · Esc で取消。"
        }
        ShowVerdict => {
            "これを候補への判定として採点しますか？「{words}」 Enter で採点 · Esc でメモとして残す。"
        }
        NotRunVerdict => "判定としては採点せず、メモとして残しました。",
        NotRunHandoff => "未実行: **{title}** はエージェントに渡していません。",
        NotRunDelegate => "未実行: **{title}** は委任していません。",
        NotRunStale => "未実行: 承認を待っていた内容はもう当てはまりません。",
        AskWhich => {
            "どれのことですか：{choices}？ はっきりするまで何も記録しません。番号か一言で答えてください。"
        }
        AskKnown => "**{title}** はもう知っていると記録する（確認なし）",
        AskTell => "答えを教えてほしい",
        AskAgent => "このパートをエージェントに任せる",
        AskCheck => "**{title}** について確認してほしい",
        AskNote => "ただのメモ",
        AskWhichPart => "どのパートのことですか：{choices}？ はっきりするまで何も記録しません。",
        Or => "、または",
        Undone => {
            "取り消しました: **{title}** についての申告は撤回されました。記録には撤回として残ります。"
        }
        NothingToUndo => {
            "取り消せるものはありません。取り消せるのは直前のステップの申告だけです。採点済みの判定、レビュー、引き渡し、委任、教わった答えはそのまま残ります。"
        }
        NothingWaiting => "承認を待っているものはありません。",
        NextStep => "次: {at} · {question}",
        NextStepNoPlace => {
            "次: 手元の資料にはまだ **{concept}** が出てくる箇所がありません。どこを見ればいいか聞いてください。 · {question}"
        }
        StepPart => "このパートは何のためにありますか？自分の言葉で。",
        StepWideExample => {
            "{concept} が使われている箇所を一つ探してください。そこで何をしていますか？"
        }
        StepWidePurpose => "このパートで {concept} は何のためにありますか？自分の言葉で。",
        StepWideMechanism => "{concept} はどう動きますか？一歩ずつ。",
        StepNarrowExample => {
            "{concept} が効いている行を一つ選んでください。違っていたら何が変わりますか？"
        }
        StepNarrowPurpose => "{concept} はなぜまさにそこで必要で、ないと何が壊れますか？",
        StepNarrowMechanism => {
            "{concept} を一歩ずつ追ってください。まず何が起き、次に何が起きますか？"
        }
        Horizon => "見通し: {what}。",
        HorizonNeeds => "前提: {parts}",
        HorizonLeads => "次へ: {parts}",
        EvidenceRatio => "合格: {passes} · 申告: {claims}。",
        ClaimWithdrawn => "**{concept}** の申告を撤回 ({date})。",
        StepHistory => "ここまでの道のり: {kinds}。",
        StepKindRate => "{kind} {judged} 回中 {passes} 回が合格につながった",
        SteeredTimes => "あなたが {n} 回方向を指定",
        StepHistoryNote => {
            "Pathfinder があなたの学び方について記録するのはこれだけで、このフォルダーのローカル記録に残ります。"
        }
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
