//! Urdu.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "**{title}** کی ممکنہ وضاحت:",
        IsItRight => "کیا یہ درست ہے؟ وجہ بتائیں۔",
        PassedOneMore => "پاس۔ ایک اور:",
        PartHint => "یہاں اپنی سوچ لکھتے جائیں۔ `/summary` · `/known` · `/delegate`",
        RestsOn => "بنیاد",
        ToldNotShown => "بتایا گیا کے طور پر درج: {labels}۔",
        GapNeeds => "پہلے **{concept}** ضروری ہے: {why}۔",
        GapClaimed => "آپ نے کہا کہ آپ جانتے ہیں، مگر ابھی کوئی ثبوت نہیں",
        GapDelegated => "آپ نے اسے سمجھے بغیر سونپ دیا",
        GapShaky => "اس پر آپ کی پچھلی جانچ ناکام رہی",
        GapUncovered => "نقشے پر ابھی کچھ بھی اسے شامل نہیں کرتا",
        NeededBy => "{part} کے لیے ضروری۔",
        AddGapParts => "{names} کے لیے ایک حصہ شامل کریں؟",
        And => " اور ",
        ProposedChange => "نقشے میں مجوزہ تبدیلی:",
        AcceptReject => "`/accept` یا `/reject`۔",
        Needs => "{deps} درکار",
        FixedReply => "آپ ابھی اسی کو جانچ رہے ہیں۔ پہلے اپنا فیصلہ دیں۔",
        GateCapped => {
            "(آج کی مشترکہ جانچیں ختم ہو گئیں؛ امیدوار سے متعلق جواب آپ کے فیصلے کے بعد آئیں گے۔ اپنی کلید: `pathfinder auth set typesafe`۔)"
        }
        GateUnavailable => {
            "(جانچ تک رسائی نہیں ہو سکی، جواب روک لیا گیا۔ تھوڑی دیر بعد دوبارہ کوشش کریں۔)"
        }
        Understood => "**{title}** سمجھ لیا گیا۔ `/next` · `/handoff`",
        KeepWorking => "کام جاری رکھیں، پھر اسے دوبارہ سمجھائیں۔",
        AllResolved => "تمام حصے حل ہو گئے۔",
        NoOtherPart => "کوئی اور حل طلب حصہ نہیں۔",
        MapUpdated => "نقشہ اپ ڈیٹ ہو گیا۔",
        ChangeRejected => "نقشے کی تبدیلی مسترد کر دی گئی۔",
        NoChangeWaiting => "نقشے کی کوئی تبدیلی منتظر نہیں۔",
        RecordedKnown => "**{title}** پہلے سے معلوم کے طور پر درج۔",
        Delegated => "**{title}** سمجھے بغیر سونپا گیا؛ جائزہ نہیں ہوا۔",
        QuickCheckLead => "مختصر جانچ:",
        ExamLead => "امتحان کی مشق، ایک وقت میں ایک سوال۔",
        ExamQuestionFor => "**{title}** پر سوال:",
        ExamAnswer => "آپ کا جواب؟",
        RecordTitle => "**اس فولڈر میں آپ کا ریکارڈ**",
        ToldOnly => "صرف بتایا گیا",
        ReadOnly => "پڑھا",
        ReadAt => "پڑھیں:",
        Logged => "درج کر لیا۔",
        NoRecord => "ابھی کوئی ریکارڈ نہیں۔",
        RecordSummary => "دکھایا: {shown} · صرف بتایا گیا: {told} · کمزور: {shaky}",
        Checks => "{passed} پاس، {failed} ناکام",
        EvidenceNote => "صرف پاس ہونے والی جانچیں شمار ہوتی ہیں۔",
        OpenMisconception => "{concept} پر کھلی غلط فہمی: \"{statement}\"",
        WhereToLook => "**کہاں دیکھیں:**",
        NotFoundOnline => "**نہیں ملا**، نہ یہاں نہ آن لائن۔",
        NotFoundHere => "**نہیں ملا**، یہاں نہیں۔",
        TryTerms => "آزمائیں: {terms}",
        NotSearched => "تلاش نہیں کیا گیا: {services}۔",
        Told => "**بتایا گیا**",
        FirstWrongStep => "**پہلا قدم جو درست نہیں بیٹھتا** ({at}):",
        NoWrongStep => "**کوئی غلط قدم نہیں ملا۔**",
        CheckUnclear => {
            "آپ کے کام میں کوئی قدم نشان زد نہیں ہو سکا۔ ہر سطر میں ایک قدم لکھیں اور پھر `/check` کریں۔"
        }
        StepAt => "سطر {n}",
        PhotoUnreadable => "تصویر پڑھی نہیں جا سکی۔",
        PhotoCopy => "{files} سے پڑھا۔ دیکھ لیں کہ یہی آپ نے لکھا تھا:",
        FindHits => "**{n}** نتائج، {terms}:",
        FindNone => "اس فولڈر کی کسی سطر میں {terms} نہیں۔",
        WebNone => "آن لائن {terms} سے کچھ نہیں ملا۔",
        ShowKnown => {
            "درج: آپ **{title}** پہلے سے جانتے ہیں۔ یہ دعویٰ ہے، ثبوت نہیں۔ `/undo` (Esc) اسے واپس لیتا ہے۔"
        }
        ShowTell => {
            "آپ کے کہنے پر بتا رہا ہوں۔ اسے بتایا گیا درج کیا جائے گا، سمجھا گیا کبھی نہیں۔ جواب آنے سے پہلے Esc منسوخ کرتا ہے۔"
        }
        ShowHandoff => {
            "**{title}** ایجنٹ کو سونپیں؟ یہ اسٹیجنگ کاپی میں کام کرتا ہے اور نتیجے کا جائزہ آپ لیتے ہیں۔ Enter چلائے · Esc منسوخ کرے۔"
        }
        ShowHandoffInPlace => {
            "**{title}** ایجنٹ کو سونپیں؟ یہ git ریپوزیٹری نہیں: یہ آپ کی فائلیں براہ راست بدلے گا، بغیر اسٹیجنگ یا جانچے گئے جائزے کے۔ Enter چلائے · Esc منسوخ کرے۔"
        }
        ShowDelegate => {
            "**{title}** سمجھے بغیر سونپیں؟ اسے سونپا گیا درج کیا جائے گا: نتیجے کی ضمانت آپ نہیں دیں گے۔ Enter چلائے · Esc منسوخ کرے۔"
        }
        ShowVerdict => {
            "اسے امیدوار پر آپ کا فیصلہ مان کر جانچیں؟ “{words}” Enter جانچے · Esc نوٹ کے طور پر رکھے۔"
        }
        NotRunVerdict => "فیصلے کے طور پر نہیں جانچا گیا: نوٹ کے طور پر رکھا۔",
        NotRunHandoff => "نہیں چلا: **{title}** ایجنٹ کو نہیں سونپا گیا۔",
        NotRunDelegate => "نہیں چلا: **{title}** نہیں سونپا گیا۔",
        NotRunStale => "نہیں چلا: جو آپ کی منظوری کا منتظر تھا وہ اب لاگو نہیں۔",
        AskWhich => {
            "کیا آپ کا مطلب تھا {choices}؟ آپ کے کہنے تک کچھ درج نہیں ہوگا۔ ورنہ بس آگے بڑھیں۔"
        }
        AskKnown => "کہ آپ **{title}** پہلے سے جانتے ہیں (`/known`)",
        AskTell => "کہ آپ کو بتایا جائے (`/tell <question>`)",
        AskAgent => "کہ یہ حصہ ایجنٹ کو دیا جائے (`/handoff` یا `/delegate`)",
        Or => " یا ",
        Undone => "واپس لیا: **{title}** پر آپ کا دعویٰ ہٹا دیا گیا۔ ریکارڈ اسے ہٹایا گیا رکھتا ہے۔",
        NothingToUndo => {
            "واپس لینے کو کچھ نہیں۔ صرف آپ کے پچھلے قدم کا دعویٰ واپس لیا جا سکتا ہے۔ جانچا گیا فیصلہ، جائزہ، سپردگی، سمجھے بغیر سپردگی اور آپ کو بتایا گیا جواب جوں کے توں رہتے ہیں۔"
        }
        NothingWaiting => "کچھ بھی آپ کی منظوری کا منتظر نہیں۔",
        NextStep => "اگلا: {at} · {question}",
        NextStepNoPlace => {
            "اگلا: آپ کے ماخذ میں ابھی کہیں **{concept}** کا نام نہیں؛ `/find <terms>` ان میں تلاش کرتا ہے۔ · {question}"
        }
        StepWideExample => "ایک جگہ تلاش کریں جہاں {concept} استعمال ہوا ہے۔ وہاں یہ کیا کرتا ہے؟",
        StepWidePurpose => "اس حصے میں {concept} کس لیے ہے، اپنے الفاظ میں؟",
        StepWideMechanism => "{concept} قدم بہ قدم کیسے کام کرتا ہے؟",
        StepNarrowExample => "ایک سطر چنیں جہاں {concept} اہم ہے۔ مختلف ہوتا تو کیا بدلتا؟",
        StepNarrowPurpose => "{concept} ٹھیک وہیں کیوں ضروری ہے، اور اس کے بغیر کیا ٹوٹتا ہے؟",
        StepNarrowMechanism => "{concept} کو قدم بہ قدم دیکھیں: پہلے کیا ہوتا ہے، پھر کیا؟",
        Horizon => "افق: {what}۔",
        HorizonNeeds => "ضرورت {parts}",
        HorizonLeads => "آگے {parts}",
        EvidenceRatio => "پاس: {passes} · دعوے: {claims}۔",
        ClaimWithdrawn => "**{concept}** پر دعویٰ ہٹا دیا گیا ({date})۔",
        StepHistory => "آپ یہاں کیسے پہنچے: {kinds}۔",
        StepKindRate => "{kind} {judged} میں سے {passes} پاس تک لے گئے",
        SteeredTimes => "آپ نے {n} بار سمت دی",
        StepHistoryNote => {
            "آپ کیسے سیکھتے ہیں، اس بارے میں Pathfinder بس اتنا ہی رکھتا ہے، اس فولڈر کے مقامی ریکارڈ میں۔"
        }
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "کھلا",
        PartStatus::Understood => "سمجھا گیا",
        PartStatus::InReview => "جائزے میں",
        PartStatus::Done => "سمجھا گیا، مکمل",
        PartStatus::Known => "معلوم ہونے کا دعویٰ",
        PartStatus::Delegated => "سمجھے بغیر سونپا گیا",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "نامعلوم",
        ConceptState::Claimed => "دعویٰ",
        ConceptState::Delegated => "سونپا گیا",
        ConceptState::Shaky => "کمزور",
        ConceptState::Shown => "دکھایا",
        ConceptState::Solid => "پختہ",
    }
}
