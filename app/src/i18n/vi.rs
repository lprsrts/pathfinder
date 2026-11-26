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
