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
