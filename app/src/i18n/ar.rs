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
        ShowKnown => "سُجّل: أنت تعرف **{title}** مسبقًا. ادعاء، لا دليل. `/undo` (Esc) يتراجع عنه.",
        ShowTell => {
            "سأخبرك كما طلبت. يُسجّل على أنه أُخبرت به، لا على أنك فهمته أبدًا. Esc قبل وصول الإجابة يلغيها."
        }
        ShowHandoff => {
            "تسليم **{title}** للوكيل؟ يعمل في نسخة تجهيز وأنت تراجع النتيجة. Enter للتنفيذ · Esc للإلغاء."
        }
        ShowHandoffInPlace => {
            "تسليم **{title}** للوكيل؟ هذا ليس مستودع git: سيعدّل ملفاتك مباشرة، دون نسخة تجهيز أو مراجعة مُتحقّق منها. Enter للتنفيذ · Esc للإلغاء."
        }
        ShowDelegate => {
            "تسليم **{title}** دون فهمه؟ سيُسجّل على أنه مُسلّم: لن تكفل النتيجة. Enter للتنفيذ · Esc للإلغاء."
        }
        ShowVerdict => "أأحكم على هذا كحكمك على المرشّح؟ «{words}» Enter للحكم · Esc لإبقائه ملاحظة.",
        NotRunVerdict => "لم يُحكم عليه كحكم: أُبقي ملاحظة.",
        NotRunHandoff => "لم يُنفّذ: لم يُسلَّم **{title}** للوكيل.",
        NotRunDelegate => "لم يُنفّذ: لم يُسلَّم **{title}**.",
        NotRunStale => "لم يُنفّذ: ما كان ينتظر موافقتك لم يعد قائمًا.",
        AskWhich => "هل قصدت {choices}؟ لا يُسجّل شيء حتى تقول. وإلا فتابع.",
        AskKnown => "أنك تعرف **{title}** مسبقًا (`/known`)",
        AskTell => "أن تُخبَر بالإجابة (`/tell <question>`)",
        AskAgent => "أن يتولى الوكيل هذا الجزء (`/handoff` أو `/delegate`)",
        Or => " أم ",
        Undone => "تم التراجع: سُحب ادعاؤك بشأن **{title}**. يحتفظ السجل به على أنه مسحوب.",
        NothingToUndo => {
            "لا شيء للتراجع عنه. يمكن سحب ادعاء من خطوتك الأخيرة فقط. الحكم المُقيَّم والمراجعة والتسليم والتسليم دون فهم والإجابة التي أُخبرت بها تبقى كما هي."
        }
        NothingWaiting => "لا شيء ينتظر موافقتك.",
        NextStep => "التالي: {at} · {question}",
        NextStepNoPlace => {
            "التالي: لا موضع في مصادرك يذكر **{concept}** بعد؛ `/find <terms>` يبحث فيها. · {question}"
        }
        StepWideExample => "اعثر على موضع يُستخدم فيه {concept}. ماذا يفعل هناك؟",
        StepWidePurpose => "ما الغرض من {concept} في هذا الجزء، بكلماتك؟",
        StepWideMechanism => "كيف يعمل {concept}، خطوة بخطوة؟",
        StepNarrowExample => "اختر سطرًا يهم فيه {concept}. ما الذي سيتغير لو كان مختلفًا؟",
        StepNarrowPurpose => "لماذا يلزم {concept} هناك تحديدًا، وما الذي ينكسر بدونه؟",
        StepNarrowMechanism => "تتبّع {concept} خطوة بخطوة: ماذا يحدث أولًا، ثم ماذا؟",
        Horizon => "الأفق: {what}.",
        HorizonNeeds => "يحتاج {parts}",
        HorizonLeads => "يقود إلى {parts}",
        EvidenceRatio => "النجاحات: {passes} · الادعاءات: {claims}.",
        ClaimWithdrawn => "سُحب الادعاء بشأن **{concept}** ({date}).",
        StepHistory => "كيف وصلت إلى هنا: {kinds}.",
        StepKindRate => "{kind} {passes} من {judged} قادت إلى نجاح",
        SteeredTimes => "وجّهته أنت {n} مرات",
        StepHistoryNote => {
            "هذا كل ما يحتفظ به Pathfinder عن طريقة تعلّمك، في السجل المحلي لهذا المجلد."
        }
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
