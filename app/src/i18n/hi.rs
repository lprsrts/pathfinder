//! Hindi.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "**{title}** की संभावित व्याख्या:",
        IsItRight => "क्या यह सही है? कारण बताइए।",
        PassedOneMore => "पास। एक और:",
        PartHint => "यहाँ अपनी सोच लिखते जाइए। `/summary` · `/known` · `/delegate`",
        RestsOn => "आधार",
        ToldNotShown => "बताया गया के रूप में दर्ज: {labels}।",
        GapNeeds => "पहले **{concept}** चाहिए: {why}।",
        GapClaimed => "आपने कहा कि आप इसे जानते हैं, पर अभी कोई प्रमाण नहीं",
        GapDelegated => "आपने इसे बिना समझे सौंप दिया",
        GapShaky => "इस पर आपकी पिछली जाँच विफल रही",
        GapUncovered => "नक्शे पर अभी कुछ भी इसे शामिल नहीं करता",
        NeededBy => "{part} के लिए ज़रूरी।",
        AddGapParts => "{names} के लिए एक हिस्सा जोड़ें?",
        And => " और ",
        ProposedChange => "नक्शे में प्रस्तावित बदलाव:",
        AcceptReject => "`/accept` या `/reject`।",
        Needs => "{deps} चाहिए",
        FixedReply => "आप अभी इसी का मूल्यांकन कर रहे हैं। पहले अपना निर्णय दीजिए।",
        GateCapped => {
            "(आज की साझा जाँचें खत्म हो गईं; उम्मीदवार से जुड़े जवाब आपके निर्णय के बाद आएँगे। अपनी कुंजी: `pathfinder auth set typesafe`।)"
        }
        GateUnavailable => "(जाँच तक पहुँच नहीं हो सकी, जवाब रोका गया। थोड़ी देर में फिर कोशिश कीजिए।)",
        Understood => "**{title}** समझ लिया गया। `/next` · `/handoff`",
        KeepWorking => "काम जारी रखिए, फिर इसे दोबारा समझाइए।",
        AllResolved => "सभी हिस्से हल हो गए।",
        NoOtherPart => "कोई और अनसुलझा हिस्सा नहीं।",
        MapUpdated => "नक्शा अपडेट हुआ।",
        ChangeRejected => "नक्शे का बदलाव अस्वीकार किया गया।",
        NoChangeWaiting => "नक्शे का कोई बदलाव प्रतीक्षा में नहीं है।",
        RecordedKnown => "**{title}** पहले से ज्ञात के रूप में दर्ज।",
        Delegated => "**{title}** बिना समझे सौंपा गया; समीक्षा नहीं हुई।",
        QuickCheckLead => "छोटी जाँच:",
        ExamLead => "परीक्षा का अभ्यास, एक बार में एक प्रश्न।",
        ExamQuestionFor => "**{title}** पर प्रश्न:",
        ExamAnswer => "आपका उत्तर?",
        RecordTitle => "**इस फ़ोल्डर में आपका रिकॉर्ड**",
        ToldOnly => "केवल बताया गया",
        ReadOnly => "पढ़ा",
        ReadAt => "पढ़ें:",
        Logged => "दर्ज किया।",
        NoRecord => "अभी कोई रिकॉर्ड नहीं।",
        RecordSummary => "दिखाया: {shown} · केवल बताया गया: {told} · कमज़ोर: {shaky}",
        Checks => "{passed} पास, {failed} विफल",
        EvidenceNote => "केवल पास हुई जाँचें गिनी जाती हैं।",
        OpenMisconception => "{concept} पर खुली ग़लतफ़हमी: \"{statement}\"",
        WhereToLook => "**कहाँ देखें:**",
        NotFoundOnline => "**नहीं मिला**, न यहाँ न ऑनलाइन।",
        NotFoundHere => "**नहीं मिला**, यहाँ नहीं।",
        TryTerms => "आज़माएँ: {terms}",
        NotSearched => "खोजा नहीं गया: {services}।",
        Told => "**बताया गया**",
        FirstWrongStep => "**पहला चरण जो सही नहीं बैठता** ({at}):",
        NoWrongStep => "**कोई ग़लत चरण नहीं मिला।**",
        CheckUnclear => {
            "आपके काम में कोई चरण पहचाना नहीं जा सका। हर पंक्ति में एक चरण लिखिए और फिर `/check` कीजिए।"
        }
        StepAt => "पंक्ति {n}",
        PhotoUnreadable => "फ़ोटो पढ़ी नहीं जा सकी।",
        PhotoCopy => "{files} से पढ़ा। जाँच लीजिए कि यही आपने लिखा था:",
        FindHits => "**{n}** परिणाम, {terms}:",
        FindNone => "इस फ़ोल्डर की किसी पंक्ति में {terms} नहीं है।",
        WebNone => "ऑनलाइन {terms} से कुछ मेल नहीं खाया।",
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "खुला",
        PartStatus::Understood => "समझा गया",
        PartStatus::InReview => "समीक्षा में",
        PartStatus::Done => "समझा गया, पूरा",
        PartStatus::Known => "ज्ञात बताया गया",
        PartStatus::Delegated => "बिना समझे सौंपा गया",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "अज्ञात",
        ConceptState::Claimed => "दावा किया",
        ConceptState::Delegated => "सौंपा गया",
        ConceptState::Shaky => "कमज़ोर",
        ConceptState::Shown => "दिखाया",
        ConceptState::Solid => "पक्का",
    }
}
