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
        PartHint => {
            "এখানে আপনার ভাবনা লিখে যান: কাজ করতে করতে আপনার যুক্তি, আর প্রস্তুত হলে অংশটি নিজের ভাষায়। আগে থেকেই জানলে বা এজেন্টকে দিয়ে করাতে চাইলে শুধু বলুন।"
        }
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
        AcceptReject => "যোগ করতে হ্যাঁ বলুন, নয়তো না।",
        Needs => "{deps} দরকার",
        FixedReply => "আপনি এখন এটারই বিচার করছেন। আগে আপনার রায় দিন।",
        GateCapped => {
            "(আজকের ভাগ করা যাচাই শেষ; প্রার্থী নিয়ে উত্তর আপনার রায়ের পরে আসবে। নিজের কী: `pathfinder auth set typesafe`।)"
        }
        GateUnavailable => "(যাচাই করা যায়নি, উত্তর আটকে রাখা হলো। একটু পরে আবার চেষ্টা করুন।)",
        Understood => "**{title}** বোঝা হয়েছে। এগোতে চাইলে বা এজেন্টকে দিতে চাইলে বলুন।",
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
        ShowKnown => {
            "লেখা হলো: আপনি **{title}** আগে থেকেই জানেন। এটা দাবি, প্রমাণ নয়। ফিরিয়ে নিতে \"undo\" বলুন বা Esc চাপুন।"
        }
        ShowTell => {
            "আপনার অনুরোধে বলছি। এটা বলা হয়েছে হিসেবে লেখা হবে, কখনো বোঝা হয়েছে হিসেবে নয়। উত্তর আসার আগে Esc বাতিল করে।"
        }
        ShowHandoff => {
            "**{title}** এজেন্টকে দেবেন? এটি একটি স্টেজিং কপিতে কাজ করে এবং ফলাফল আপনি পর্যালোচনা করেন। Enter চালায় · Esc বাতিল করে।"
        }
        ShowHandoffInPlace => {
            "**{title}** এজেন্টকে দেবেন? এটি git রিপোজিটরি নয়: এটি সরাসরি আপনার ফাইল বদলাবে, স্টেজিং বা যাচাই করা পর্যালোচনা ছাড়া। Enter চালায় · Esc বাতিল করে।"
        }
        ShowDelegate => {
            "**{title}** না বুঝে হস্তান্তর করবেন? এটা হস্তান্তরিত হিসেবে লেখা হবে: ফলাফলের দায় আপনি নেবেন না। Enter চালায় · Esc বাতিল করে।"
        }
        ShowVerdict => {
            "এটাকে প্রার্থীর উপর আপনার রায় হিসেবে বিচার করব? “{words}” Enter বিচার করে · Esc নোট হিসেবে রাখে।"
        }
        NotRunVerdict => "রায় হিসেবে বিচার হয়নি: নোট হিসেবে রাখা হলো।",
        NotRunHandoff => "চালানো হয়নি: **{title}** এজেন্টকে দেওয়া হয়নি।",
        NotRunDelegate => "চালানো হয়নি: **{title}** হস্তান্তর করা হয়নি।",
        NotRunStale => "চালানো হয়নি: যা আপনার সম্মতির অপেক্ষায় ছিল তা আর প্রযোজ্য নয়।",
        AskWhich => {
            "আপনি কোনটি বোঝাতে চেয়েছেন: {choices}? আপনি না বলা পর্যন্ত কিছু লেখা হবে না; একটি নম্বর বা কয়েকটি শব্দই যথেষ্ট।"
        }
        AskKnown => "লিখে রাখুন যে আমি **{title}** আগে থেকেই জানি (যাচাই ছাড়া)",
        AskTell => "শুধু আমাকে বলে দিন",
        AskAgent => "এই অংশটি এজেন্টকে করতে দিন",
        AskCheck => "**{title}** নিয়ে আমাকে যাচাই করুন",
        AskNote => "শুধু একটি নোট",
        AskWhichPart => "আপনি কোন অংশের কথা বলছেন: {choices}? আপনি না বলা পর্যন্ত কিছু লেখা হবে না।",
        Or => " নাকি ",
        Undone => {
            "ফিরিয়ে নেওয়া হলো: **{title}** নিয়ে আপনার দাবি প্রত্যাহার করা হয়েছে। রেকর্ডে এটি প্রত্যাহৃত হিসেবে থাকে।"
        }
        NothingToUndo => {
            "ফিরিয়ে নেওয়ার কিছু নেই। শুধু আপনার শেষ ধাপের একটি দাবি ফিরিয়ে নেওয়া যায়। বিচার করা রায়, পর্যালোচনা, হস্তান্তর, না বুঝে হস্তান্তর এবং আপনাকে বলা উত্তর যেমন আছে তেমনই থাকে।"
        }
        NothingWaiting => "কিছুই আপনার সম্মতির অপেক্ষায় নেই।",
        NextStep => "পরের ধাপ: {at} · {question}",
        NextStepNoPlace => {
            "পরের ধাপ: আপনার উৎসে এখনো কোথাও **{concept}** এর নাম নেই; কোথায় দেখবেন আমাকে জিজ্ঞেস করুন। · {question}"
        }
        StepPart => "এই অংশটা কিসের জন্য, নিজের ভাষায়?",
        StepWideExample => "এমন একটি জায়গা খুঁজুন যেখানে {concept} ব্যবহৃত হয়েছে। সেখানে এটি কী করে?",
        StepWidePurpose => "এই অংশে {concept} কিসের জন্য, নিজের ভাষায়?",
        StepWideMechanism => "{concept} ধাপে ধাপে কীভাবে কাজ করে?",
        StepNarrowExample => "এমন একটি লাইন বেছে নিন যেখানে {concept} গুরুত্বপূর্ণ। আলাদা হলে কী বদলাত?",
        StepNarrowPurpose => "{concept} ঠিক সেখানেই কেন দরকার, আর এটি ছাড়া কী ভাঙে?",
        StepNarrowMechanism => "{concept} ধাপে ধাপে অনুসরণ করুন: প্রথমে কী হয়, তারপর কী?",
        Horizon => "দিগন্ত: {what}।",
        HorizonNeeds => "প্রয়োজন {parts}",
        HorizonLeads => "নিয়ে যায় {parts}",
        EvidenceRatio => "পাস: {passes} · দাবি: {claims}।",
        ClaimWithdrawn => "**{concept}** নিয়ে দাবি প্রত্যাহৃত ({date})।",
        StepHistory => "আপনি এখানে কীভাবে এলেন: {kinds}।",
        StepKindRate => "{kind} {judged} এর মধ্যে {passes} পাসে পৌঁছেছে",
        SteeredTimes => "আপনি {n} বার দিক ঠিক করেছেন",
        StepHistoryNote => {
            "আপনি কীভাবে শেখেন সে বিষয়ে Pathfinder এটুকুই রাখে, এই ফোল্ডারের স্থানীয় রেকর্ডে।"
        }
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
