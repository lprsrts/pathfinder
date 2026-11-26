//! Bengali.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "**{title}**-এর সম্ভাব্য ব্যাখ্যা:",
        IsItRight => "এটা কি ঠিক? কেন, বলুন।",
        PassedOneMore => "পাস। আরেকটি:",
        PartHint => "এখানে আপনার ভাবনা লিখে যান। `/summary` · `/known` · `/delegate`",
        RestsOn => "ভিত্তি",
        ToldNotShown => "বলে দেওয়া হিসেবে লেখা হলো: {labels}।",
        GapNeeds => "আগে **{concept}** দরকার: {why}।",
        GapClaimed => "আপনি জানেন বলেছেন, কিন্তু এখনো প্রমাণ নেই",
        GapDelegated => "আপনি না বুঝে এটা হস্তান্তর করেছেন",
        GapShaky => "এ বিষয়ে আপনার শেষ যাচাই ব্যর্থ হয়েছে",
        GapUncovered => "মানচিত্রে এখনো কিছুই এটাকে ধরে না",
        NeededBy => "{part}-এর জন্য দরকার।",
        AddGapParts => "{names}-এর জন্য একটি অংশ যোগ করবেন?",
        And => " ও ",
        ProposedChange => "মানচিত্রে প্রস্তাবিত পরিবর্তন:",
        AcceptReject => "`/accept` বা `/reject`।",
        Needs => "{deps} দরকার",
        FixedReply => "আপনি এখন এটারই বিচার করছেন। আগে আপনার রায় দিন।",
        GateCapped => {
            "(আজকের ভাগ করা যাচাই শেষ; প্রার্থী নিয়ে উত্তর আপনার রায়ের পরে আসবে। নিজের কী: `pathfinder auth set typesafe`।)"
        }
        GateUnavailable => "(যাচাই করা যায়নি, উত্তর আটকে রাখা হলো। একটু পরে আবার চেষ্টা করুন।)",
        Understood => "**{title}** বোঝা হয়েছে। `/next` · `/handoff`",
        KeepWorking => "কাজ চালিয়ে যান, তারপর আবার ব্যাখ্যা করুন।",
        AllResolved => "সব অংশের সমাধান হয়েছে।",
        NoOtherPart => "আর কোনো অমীমাংসিত অংশ নেই।",
        MapUpdated => "মানচিত্র হালনাগাদ হয়েছে।",
        ChangeRejected => "মানচিত্রের পরিবর্তন বাতিল হয়েছে।",
        NoChangeWaiting => "মানচিত্রের কোনো পরিবর্তন অপেক্ষায় নেই।",
        RecordedKnown => "**{title}** আগে থেকেই জানা হিসেবে লেখা হলো।",
        Delegated => "**{title}** না বুঝে হস্তান্তর করা হয়েছে; পর্যালোচনা হয়নি।",
        QuickCheckLead => "ছোট যাচাই:",
        ExamLead => "পরীক্ষার অনুশীলন, একবারে একটি প্রশ্ন।",
        ExamQuestionFor => "**{title}** নিয়ে প্রশ্ন:",
        ExamAnswer => "আপনার উত্তর?",
        RecordTitle => "**এই ফোল্ডারে আপনার রেকর্ড**",
        ToldOnly => "শুধু বলা হয়েছে",
        ReadOnly => "পড়া হয়েছে",
        ReadAt => "পড়ুন:",
        Logged => "লেখা হলো।",
        NoRecord => "এখনো কোনো রেকর্ড নেই।",
        RecordSummary => "দেখানো: {shown} · শুধু বলা: {told} · দুর্বল: {shaky}",
        Checks => "{passed} পাস, {failed} ফেল",
        EvidenceNote => "শুধু পাস করা যাচাই গোনা হয়।",
        OpenMisconception => "{concept} নিয়ে খোলা ভুল ধারণা: \"{statement}\"",
        WhereToLook => "**কোথায় দেখবেন:**",
        NotFoundOnline => "**পাওয়া যায়নি**, এখানেও না, অনলাইনেও না।",
        NotFoundHere => "**পাওয়া যায়নি**, এখানে নেই।",
        TryTerms => "চেষ্টা করুন: {terms}",
        NotSearched => "খোঁজা হয়নি: {services}।",
        Told => "**বলে দেওয়া**",
        FirstWrongStep => "**প্রথম যে ধাপ টেকে না** ({at}):",
        NoWrongStep => "**টেকে না এমন কোনো ধাপ পাওয়া যায়নি।**",
        CheckUnclear => {
            "আপনার কাজে কোনো ধাপ চিহ্নিত করা গেল না। প্রতি লাইনে একটি ধাপ লিখে আবার `/check` করুন।"
        }
        StepAt => "লাইন {n}",
        PhotoUnreadable => "ছবিটি পড়া গেল না।",
        PhotoCopy => "{files} থেকে পড়া হয়েছে। আপনি যা লিখেছেন তা-ই কি না দেখে নিন:",
        FindHits => "**{n}** ফলাফল, {terms}:",
        FindNone => "এই ফোল্ডারের কোনো লাইনে {terms} নেই।",
        WebNone => "অনলাইনে {terms}-এর সঙ্গে কিছু মেলেনি।",
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "খোলা",
        PartStatus::Understood => "বোঝা হয়েছে",
        PartStatus::InReview => "পর্যালোচনায়",
        PartStatus::Done => "বোঝা হয়েছে, সম্পন্ন",
        PartStatus::Known => "জানা বলে দাবি",
        PartStatus::Delegated => "না বুঝে হস্তান্তর",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "অজানা",
        ConceptState::Claimed => "দাবি করা",
        ConceptState::Delegated => "হস্তান্তরিত",
        ConceptState::Shaky => "দুর্বল",
        ConceptState::Shown => "দেখানো",
        ConceptState::Solid => "পাকা",
    }
}
