//! Vietnamese.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "Đề xuất giải thích cho **{title}**:",
        IsItRight => "Có đúng không? Hãy nói vì sao.",
        PassedOneMore => "Đạt. Thêm một câu nữa:",
        PartHint => "Hãy viết ra suy nghĩ của bạn ở đây. `/summary` · `/known` · `/delegate`",
        RestsOn => "Dựa trên",
        ToldNotShown => "Đã ghi là được cho biết: {labels}.",
        GapNeeds => "Cần **{concept}** trước: {why}.",
        GapClaimed => "bạn nói đã biết nhưng chưa có bằng chứng",
        GapDelegated => "bạn đã giao đi mà chưa hiểu",
        GapShaky => "lần kiểm tra gần nhất của bạn không đạt",
        GapUncovered => "chưa có gì trên bản đồ bao gồm nó",
        NeededBy => "Cần cho {part}.",
        AddGapParts => "Thêm một phần cho {names}?",
        And => " và ",
        ProposedChange => "Đề xuất thay đổi bản đồ:",
        AcceptReject => "`/accept` hoặc `/reject`.",
        Needs => "cần {deps}",
        FixedReply => "Đó là điều bạn đang đánh giá. Hãy đưa ra kết luận trước.",
        GateCapped => {
            "(Lượt kiểm tra dùng chung hôm nay đã hết; câu trả lời về đề xuất sẽ chờ kết luận của bạn. Khóa riêng: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => {
            "(Không kết nối được dịch vụ kiểm tra, câu trả lời bị giữ lại. Hãy thử lại sau giây lát.)"
        }
        Understood => "**{title}**: đã hiểu. `/next` · `/handoff`",
        KeepWorking => "Tiếp tục làm, rồi giải thích lại.",
        AllResolved => "Mọi phần đã được giải quyết.",
        NoOtherPart => "Không còn phần nào chưa giải quyết.",
        MapUpdated => "Đã cập nhật bản đồ.",
        ChangeRejected => "Đã từ chối thay đổi bản đồ.",
        NoChangeWaiting => "Không có thay đổi bản đồ nào đang chờ.",
        RecordedKnown => "**{title}**: đã ghi là đã biết.",
        Delegated => "**{title}**: đã giao đi mà chưa hiểu; chưa được xem xét.",
        QuickCheckLead => "Kiểm tra nhanh:",
        ExamLead => "Luyện thi, mỗi lần một câu hỏi.",
        ExamQuestionFor => "Câu hỏi về **{title}**:",
        ExamAnswer => "Câu trả lời của bạn?",
        RecordTitle => "**Hồ sơ của bạn trong thư mục này**",
        ToldOnly => "chỉ được cho biết",
        ReadOnly => "đã đọc",
        ReadAt => "Đọc:",
        Logged => "Đã ghi.",
        NoRecord => "Chưa có hồ sơ.",
        RecordSummary => "Đã chứng tỏ: {shown} · chỉ được cho biết: {told} · chưa vững: {shaky}",
        Checks => "{passed} đạt, {failed} không đạt",
        EvidenceNote => "Chỉ tính các lần kiểm tra đạt.",
        OpenMisconception => "Hiểu lầm chưa giải quyết về {concept}: \"{statement}\"",
        WhereToLook => "**Nơi cần xem:**",
        NotFoundOnline => "**Không tìm thấy** ở đây lẫn trên mạng.",
        NotFoundHere => "**Không tìm thấy** ở đây.",
        TryTerms => "Thử: {terms}",
        NotSearched => "Chưa tìm: {services}.",
        Told => "**Được cho biết**",
        FirstWrongStep => "**Bước đầu tiên không đúng** ({at}):",
        NoWrongStep => "**Không tìm thấy bước nào sai.**",
        CheckUnclear => {
            "Không xác định được bước nào trong bài của bạn. Hãy viết mỗi dòng một bước rồi `/check` lại."
        }
        StepAt => "dòng {n}",
        PhotoUnreadable => "Không đọc được ảnh.",
        PhotoCopy => "Đọc từ {files}. Hãy kiểm tra xem đây có đúng là điều bạn đã viết:",
        FindHits => "**{n}** kết quả cho {terms}:",
        FindNone => "Không có dòng nào trong thư mục này chứa {terms}.",
        WebNone => "Không có kết quả trực tuyến nào khớp với {terms}.",
        ShowKnown => {
            "Đã ghi: bạn đã biết **{title}**. Một lời tự nhận, không phải bằng chứng. `/undo` (Esc) rút lại."
        }
        ShowTell => {
            "Sẽ nói cho bạn, như bạn yêu cầu. Ghi là đã được nói, không bao giờ là đã hiểu. Esc trước khi câu trả lời đến sẽ hủy."
        }
        ShowHandoff => {
            "Giao **{title}** cho tác tử? Nó làm trong một bản sao staging và bạn xem lại kết quả. Enter chạy · Esc hủy."
        }
        ShowHandoffInPlace => {
            "Giao **{title}** cho tác tử? Đây không phải kho git: nó sửa trực tiếp tệp của bạn, không có staging hay xem xét có kiểm tra. Enter chạy · Esc hủy."
        }
        ShowDelegate => {
            "Giao **{title}** đi mà chưa hiểu? Sẽ ghi là đã giao: bạn không bảo đảm cho kết quả. Enter chạy · Esc hủy."
        }
        ShowVerdict => {
            "Xem đây là phán quyết của bạn về ứng viên? “{words}” Enter chấm · Esc giữ làm ghi chú."
        }
        NotRunVerdict => "Không chấm như phán quyết: giữ làm ghi chú.",
        NotRunHandoff => "Chưa chạy: **{title}** chưa được giao cho tác tử.",
        NotRunDelegate => "Chưa chạy: **{title}** chưa được giao đi.",
        NotRunStale => "Chưa chạy: điều đang chờ bạn đồng ý không còn áp dụng.",
        AskWhich => {
            "Ý bạn là {choices}? Chưa ghi gì cho đến khi bạn nói. Nếu không thì cứ tiếp tục."
        }
        AskKnown => "rằng bạn đã biết **{title}** (`/known`)",
        AskTell => "được nói cho biết (`/tell <question>`)",
        AskAgent => "giao phần này cho tác tử (`/handoff` hoặc `/delegate`)",
        Or => " hay ",
        Undone => "Đã hoàn tác: lời tự nhận về **{title}** đã rút lại. Hồ sơ giữ nó ở dạng đã rút.",
        NothingToUndo => {
            "Không có gì để hoàn tác. Chỉ lời tự nhận ở bước vừa rồi mới rút lại được. Phán quyết đã chấm, lần xem xét, lần giao, lần giao khi chưa hiểu và câu trả lời bạn được nói vẫn giữ nguyên."
        }
        NothingWaiting => "Không có gì đang chờ bạn đồng ý.",
        NextStep => "Tiếp theo: {at} · {question}",
        NextStepNoPlace => {
            "Tiếp theo: chưa chỗ nào trong nguồn của bạn nhắc **{concept}**; `/find <terms>` tìm trong đó. · {question}"
        }
        StepWideExample => "Tìm một chỗ dùng {concept}. Nó làm gì ở đó?",
        StepWidePurpose => "{concept} dùng để làm gì trong phần này, theo lời bạn?",
        StepWideMechanism => "{concept} hoạt động thế nào, từng bước?",
        StepNarrowExample => {
            "Chọn một dòng nơi {concept} quan trọng. Nếu nó khác đi thì điều gì thay đổi?"
        }
        StepNarrowPurpose => "Vì sao cần {concept} đúng ở đó, và thiếu nó thì hỏng gì?",
        StepNarrowMechanism => "Lần theo {concept} từng bước: điều gì xảy ra trước, rồi gì tiếp?",
        Horizon => "Chân trời: {what}.",
        HorizonNeeds => "cần {parts}",
        HorizonLeads => "dẫn tới {parts}",
        EvidenceRatio => "Đạt: {passes} · tự nhận: {claims}.",
        ClaimWithdrawn => "Lời tự nhận về **{concept}** đã rút ({date}).",
        StepHistory => "Bạn đến đây thế nào: {kinds}.",
        StepKindRate => "{kind} {passes} trên {judged} dẫn tới đạt",
        SteeredTimes => "bạn tự định hướng {n} lần",
        StepHistoryNote => {
            "Đó là tất cả những gì Pathfinder giữ về cách bạn học, trong hồ sơ cục bộ của thư mục này."
        }
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "đang mở",
        PartStatus::Understood => "đã hiểu",
        PartStatus::InReview => "đang xem xét",
        PartStatus::Done => "đã hiểu, xong",
        PartStatus::Known => "tự nhận đã biết",
        PartStatus::Delegated => "giao đi mà chưa hiểu",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "chưa biết",
        ConceptState::Claimed => "tự nhận",
        ConceptState::Delegated => "đã giao",
        ConceptState::Shaky => "chưa vững",
        ConceptState::Shown => "đã chứng tỏ",
        ConceptState::Solid => "vững chắc",
    }
}
