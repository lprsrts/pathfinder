//! Arabic.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "شرح مرشّح لـ **{title}**:",
        IsItRight => "هل هذا صحيح؟ قل لماذا.",
        PassedOneMore => "نجحت. واحد آخر:",
        PartHint => "فكّر بصوت عالٍ هنا. `/summary` · `/known` · `/delegate`",
        RestsOn => "يعتمد على",
        ToldNotShown => "سُجّل على أنه قيل لك: {labels}.",
        GapNeeds => "يحتاج أولًا إلى **{concept}**: {why}.",
        GapClaimed => "قلت إنك تعرفه، ولا دليل بعد",
        GapDelegated => "سلّمته دون أن تفهمه",
        GapShaky => "فشل آخر تحقق لك فيه",
        GapUncovered => "لا شيء في الخريطة يغطيه بعد",
        NeededBy => "مطلوب لـ {part}.",
        AddGapParts => "إضافة جزء لـ {names}؟",
        And => " و",
        ProposedChange => "تغيير مقترح على الخريطة:",
        AcceptReject => "`/accept` أو `/reject`.",
        Needs => "يحتاج إلى {deps}",
        FixedReply => "هذا ما تحكم عليه الآن. حكمك أولًا.",
        GateCapped => {
            "(نفدت عمليات التحقق المشتركة لهذا اليوم؛ الردود المتعلقة بالمرشّح تنتظر حكمك. مفتاحك الخاص: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => "(تعذّر الوصول إلى التحقق، حُجب الرد. حاول مجددًا بعد قليل.)",
        Understood => "**{title}**: تم الفهم. `/next` · `/handoff`",
        KeepWorking => "واصل العمل، ثم اشرحه مرة أخرى.",
        AllResolved => "حُلّت جميع الأجزاء.",
        NoOtherPart => "لا يوجد جزء آخر لم يُحل.",
        MapUpdated => "حُدّثت الخريطة.",
        ChangeRejected => "رُفض تغيير الخريطة.",
        NoChangeWaiting => "لا يوجد تغيير على الخريطة بانتظار الرد.",
        RecordedKnown => "**{title}**: سُجّل على أنه معروف مسبقًا.",
        Delegated => "**{title}**: سُلّم دون فهم؛ لم يُراجع.",
        QuickCheckLead => "تحقق سريع:",
        ExamLead => "تدريب على الامتحان، سؤال واحد في كل مرة.",
        ExamQuestionFor => "سؤال عن **{title}**:",
        ExamAnswer => "ما إجابتك؟",
        RecordTitle => "**سجلّك في هذا المجلد**",
        ToldOnly => "قيل فقط",
        ReadOnly => "مقروء",
        ReadAt => "اقرأ:",
        Logged => "سُجّل.",
        NoRecord => "لا يوجد سجل بعد.",
        RecordSummary => "أُثبت: {shown} · قيل فقط: {told} · مهزوز: {shaky}",
        Checks => "{passed} ناجح، {failed} فاشل",
        EvidenceNote => "لا يُحتسب إلا التحقق الناجح.",
        OpenMisconception => "فهم خاطئ قائم حول {concept}: «{statement}»",
        WhereToLook => "**أين تبحث:**",
        NotFoundOnline => "**لم يُعثر عليه** هنا ولا على الإنترنت.",
        NotFoundHere => "**لم يُعثر عليه** هنا.",
        TryTerms => "جرّب: {terms}",
        NotSearched => "لم يُبحث في: {services}.",
        Told => "**قيل لك**",
        FirstWrongStep => "**أول خطوة لا تصح** ({at}):",
        NoWrongStep => "**لم يُعثر على خطوة لا تصح.**",
        CheckUnclear => {
            "تعذّر تحديد خطوة في عملك. اكتب خطوة واحدة في كل سطر ثم استخدم `/check` مجددًا."
        }
        StepAt => "السطر {n}",
        PhotoUnreadable => "تعذّرت قراءة الصورة.",
        PhotoCopy => "قُرئ من {files}. تحقّق من أن هذا ما كتبته:",
        FindHits => "**{n}** نتيجة لـ {terms}:",
        FindNone => "لا يحتوي أي سطر في هذا المجلد على {terms}.",
        WebNone => "لا شيء على الإنترنت يطابق {terms}.",
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "مفتوح",
        PartStatus::Understood => "مفهوم",
        PartStatus::InReview => "قيد المراجعة",
        PartStatus::Done => "مفهوم ومنتهٍ",
        PartStatus::Known => "مُعلن أنه معروف",
        PartStatus::Delegated => "مُسلّم دون فهم",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "غير معروف",
        ConceptState::Claimed => "مُدّعى",
        ConceptState::Delegated => "مُسلّم",
        ConceptState::Shaky => "مهزوز",
        ConceptState::Shown => "مُثبت",
        ConceptState::Solid => "راسخ",
    }
}
