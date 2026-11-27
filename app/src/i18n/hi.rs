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
        PartHint => {
            "यहाँ अपनी सोच लिखते जाइए: काम करते हुए अपना तर्क, और तैयार होने पर इस हिस्से को अपने शब्दों में। अगर आप इसे पहले से जानते हैं या चाहते हैं कि एजेंट इसे करे, तो बस कह दीजिए।"
        }
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
        AcceptReject => "जोड़ने के लिए हाँ कहें, या नहीं।",
        Needs => "{deps} चाहिए",
        FixedReply => "आप अभी इसी का मूल्यांकन कर रहे हैं। पहले अपना निर्णय दीजिए।",
        GateCapped => {
            "(आज की साझा जाँचें खत्म हो गईं; उम्मीदवार से जुड़े जवाब आपके निर्णय के बाद आएँगे। अपनी कुंजी: `pathfinder auth set typesafe`।)"
        }
        GateUnavailable => "(जाँच तक पहुँच नहीं हो सकी, जवाब रोका गया। थोड़ी देर में फिर कोशिश कीजिए।)",
        Understood => "**{title}** समझ लिया गया। आगे बढ़ना हो या इसे एजेंट को देना हो, तो बताइए।",
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
        ShowKnown => {
            "दर्ज: आप **{title}** पहले से जानते हैं। यह दावा है, प्रमाण नहीं। वापस लेने के लिए \"undo\" कहें या Esc दबाएँ।"
        }
        ShowTell => {
            "आपके कहने पर बता रहा हूँ। इसे बताया गया दर्ज किया जाएगा, समझा गया कभी नहीं। उत्तर आने से पहले Esc रद्द करता है।"
        }
        ShowHandoff => {
            "**{title}** एजेंट को सौंपें? यह एक स्टेजिंग प्रति में काम करता है और नतीजे की समीक्षा आप करते हैं। Enter चलाए · Esc रद्द करे।"
        }
        ShowHandoffInPlace => {
            "**{title}** एजेंट को सौंपें? यह git रिपॉज़िटरी नहीं है: यह आपकी फ़ाइलें सीधे बदलेगा, बिना स्टेजिंग या जाँची गई समीक्षा के। Enter चलाए · Esc रद्द करे।"
        }
        ShowDelegate => {
            "**{title}** बिना समझे सौंपें? इसे सौंपा गया दर्ज किया जाएगा: नतीजे की ज़िम्मेदारी आप नहीं लेंगे। Enter चलाए · Esc रद्द करे।"
        }
        ShowVerdict => {
            "इसे उम्मीदवार पर आपका निर्णय मानकर जाँचें? “{words}” Enter जाँचे · Esc नोट के रूप में रखे।"
        }
        NotRunVerdict => "निर्णय के रूप में नहीं जाँचा गया: नोट के रूप में रखा।",
        NotRunHandoff => "नहीं चला: **{title}** एजेंट को नहीं सौंपा गया।",
        NotRunDelegate => "नहीं चला: **{title}** नहीं सौंपा गया।",
        NotRunStale => "नहीं चला: जो आपकी स्वीकृति का इंतज़ार कर रहा था, वह अब लागू नहीं।",
        AskWhich => {
            "आपका मतलब क्या था: {choices}? आपके कहने तक कुछ दर्ज नहीं होगा; एक नंबर या कुछ शब्द काफ़ी हैं।"
        }
        AskKnown => "दर्ज करें कि मैं **{title}** पहले से जानता हूँ (बिना जाँच)",
        AskTell => "बस मुझे बता दीजिए",
        AskAgent => "यह हिस्सा एजेंट को करने दीजिए",
        AskCheck => "**{title}** पर मेरी जाँच करें",
        AskNote => "बस एक नोट",
        AskWhichPart => "आपका मतलब किस हिस्से से है: {choices}? आपके कहने तक कुछ दर्ज नहीं होगा।",
        Or => " या ",
        Undone => "वापस लिया: **{title}** पर आपका दावा हटाया गया। रिकॉर्ड इसे हटाया गया दिखाता है।",
        NothingToUndo => {
            "वापस लेने को कुछ नहीं। केवल आपके पिछले कदम का दावा वापस लिया जा सकता है। जाँचा गया निर्णय, समीक्षा, सौंपना, बिना समझे सौंपना और आपको बताया गया उत्तर वैसे ही रहते हैं।"
        }
        NothingWaiting => "कुछ भी आपकी स्वीकृति का इंतज़ार नहीं कर रहा।",
        NextStep => "अगला: {at} · {question}",
        NextStepNoPlace => {
            "अगला: आपके स्रोतों में अभी कहीं **{concept}** का नाम नहीं है; मुझसे पूछिए कि कहाँ देखें। · {question}"
        }
        StepPart => "यह हिस्सा किसलिए है, अपने शब्दों में?",
        StepWideExample => "एक जगह खोजिए जहाँ {concept} इस्तेमाल हुआ है। वहाँ यह क्या करता है?",
        StepWidePurpose => "इस हिस्से में {concept} किसलिए है, अपने शब्दों में?",
        StepWideMechanism => "{concept} कदम-दर-कदम कैसे काम करता है?",
        StepNarrowExample => "एक पंक्ति चुनिए जहाँ {concept} मायने रखता है। अलग होता तो क्या बदलता?",
        StepNarrowPurpose => "{concept} ठीक वहीं क्यों ज़रूरी है, और इसके बिना क्या टूटता है?",
        StepNarrowMechanism => "{concept} को कदम-दर-कदम देखिए: पहले क्या होता है, फिर क्या?",
        Horizon => "क्षितिज: {what}।",
        HorizonNeeds => "ज़रूरी {parts}",
        HorizonLeads => "आगे {parts}",
        EvidenceRatio => "पास: {passes} · दावे: {claims}।",
        ClaimWithdrawn => "**{concept}** पर दावा हटाया गया ({date})।",
        StepHistory => "आप यहाँ कैसे पहुँचे: {kinds}।",
        StepKindRate => "{kind} {judged} में से {passes} पास तक ले गए",
        SteeredTimes => "आपने {n} बार दिशा दी",
        StepHistoryNote => {
            "आप कैसे सीखते हैं, इसके बारे में Pathfinder बस इतना ही रखता है, इस फ़ोल्डर के स्थानीय रिकॉर्ड में।"
        }
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
