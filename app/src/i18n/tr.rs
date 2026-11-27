//! Turkish.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "**{title}** için aday:",
        IsItRight => "Doğru mu? Nedenini söyle.",
        PassedOneMore => "Geçti. Bir tane daha:",
        PartHint => {
            "Burada sesli düşün: çalışırken akıl yürütmeni, hazır olduğunda da bu bölümü kendi sözlerinle yaz. Zaten biliyorsan ya da ajanın yapmasını istiyorsan, söylemen yeterli."
        }
        RestsOn => "Dayandıkları",
        ToldNotShown => "Söylendi olarak kaydedildi: {labels}.",
        GapNeeds => "Önce **{concept}** gerekiyor: {why}.",
        GapClaimed => "bildiğini söyledin, henüz kanıtı yok",
        GapDelegated => "anlamadan devrettin",
        GapShaky => "son kontrolün başarısız oldu",
        GapUncovered => "haritada henüz onu kapsayan bir şey yok",
        NeededBy => "{part} için gerekli.",
        AddGapParts => "{names} için bir parça eklensin mi?",
        And => " ve ",
        ProposedChange => "Önerilen harita değişikliği:",
        AcceptReject => "Eklemek için evet de, ya da hayır.",
        Needs => "gerektirir: {deps}",
        FixedReply => "Şu an bunu değerlendiriyorsun. Önce kararın.",
        GateCapped => {
            "(Bugünkü ortak kontroller bitti; adayla ilgili yanıtlar kararını bekliyor. Kendi anahtarın: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => "(Kontrole ulaşılamadı, yanıt bekletildi. Birazdan tekrar dene.)",
        Understood => "**{title}** anlaşıldı. Devam etmek ya da ajana vermek istediğinde söyle.",
        KeepWorking => "Çalışmaya devam et, sonra yeniden açıkla.",
        AllResolved => "Tüm parçalar çözüldü.",
        NoOtherPart => "Çözülmemiş başka parça yok.",
        MapUpdated => "Harita güncellendi.",
        ChangeRejected => "Harita değişikliği reddedildi.",
        NoChangeWaiting => "Bekleyen harita değişikliği yok.",
        RecordedKnown => "**{title}** zaten biliniyor olarak kaydedildi.",
        Delegated => "**{title}** anlaşılmadan devredildi; incelenmedi.",
        QuickCheckLead => "Kısa kontrol:",
        ExamLead => "Sınav pratiği, her seferinde bir soru.",
        ExamQuestionFor => "**{title}** hakkında soru:",
        ExamAnswer => "Cevabın?",
        RecordTitle => "**Bu klasördeki kaydın**",
        ToldOnly => "yalnızca söylendi",
        ReadOnly => "okundu",
        ReadAt => "Oku:",
        Logged => "Not alındı.",
        NoRecord => "Henüz kayıt yok.",
        RecordSummary => "Gösterilen: {shown} · yalnızca söylenen: {told} · sallantıda: {shaky}",
        Checks => "{passed} geçti, {failed} kaldı",
        EvidenceNote => "Yalnızca geçilen kontroller sayılır.",
        OpenMisconception => "{concept} konusunda açık yanılgı: \"{statement}\"",
        WhereToLook => "**Nereye bakmalı:**",
        NotFoundOnline => "**Bulunamadı**, ne burada ne çevrimiçi.",
        NotFoundHere => "**Bulunamadı**, burada yok.",
        TryTerms => "Dene: {terms}",
        NotSearched => "Aranmadı: {services}.",
        Told => "**Söylendi**",
        FirstWrongStep => "**Tutmayan ilk adım** ({at}):",
        NoWrongStep => "**Tutmayan adım bulunamadı.**",
        CheckUnclear => {
            "Çalışmanda bir adımı gösteremedim. Her satıra bir adım yaz ve yeniden `/check` kullan."
        }
        StepAt => "satır {n}",
        PhotoUnreadable => "Fotoğraf okunamadı.",
        PhotoCopy => "{files} dosyasından okundu. Yazdığın bu mu, kontrol et:",
        FindHits => "**{n}** sonuç, {terms}:",
        FindNone => "Bu klasörde {terms} içeren satır yok.",
        WebNone => "Çevrimiçi hiçbir şey {terms} ile eşleşmedi.",
        ShowKnown => {
            "Kaydedildi: **{title}** konusunu zaten biliyorsun. Bir iddia, kanıt değil. Geri almak için \"geri al\" de ya da Esc'ye bas."
        }
        ShowTell => {
            "İstediğin gibi söylüyorum. Söylendi olarak kaydedilir, asla anlaşıldı olarak değil. Gelmeden önce Esc iptal eder."
        }
        ShowHandoff => {
            "**{title}** ajana verilsin mi? Bir hazırlık kopyasında çalışır ve sonucu sen incelersin. Enter çalıştırır · Esc iptal eder."
        }
        ShowHandoffInPlace => {
            "**{title}** ajana verilsin mi? Burası bir git deposu değil: dosyalarını doğrudan düzenler, hazırlık kopyası ve kontrollü inceleme olmadan. Enter çalıştırır · Esc iptal eder."
        }
        ShowDelegate => {
            "**{title}** anlamadan devredilsin mi? Devredildi olarak kaydedilir: sonuca kefil olmazsın. Enter çalıştırır · Esc iptal eder."
        }
        ShowVerdict => {
            "Bu, aday hakkındaki kararın olarak değerlendirilsin mi? “{words}” Enter değerlendirir · Esc not olarak tutar."
        }
        NotRunVerdict => "Karar olarak değerlendirilmedi: not olarak tutuldu.",
        NotRunHandoff => "Çalıştırılmadı: **{title}** ajana verilmedi.",
        NotRunDelegate => "Çalıştırılmadı: **{title}** devredilmedi.",
        NotRunStale => "Çalıştırılmadı: onayını bekleyen şey artık geçerli değil.",
        AskWhich => {
            "Hangisini demek istedin: {choices}? Sen söyleyene kadar hiçbir şey kaydedilmez; bir numara ya da birkaç kelime yeter."
        }
        AskKnown => "**{title}** konusunu zaten bildiğimi kaydet (kontrolsüz)",
        AskTell => "bana söyleyiver",
        AskAgent => "bu bölümü ajan yapsın",
        AskCheck => "beni **{title}** üzerinde sına",
        AskNote => "sadece bir not",
        AskWhichPart => {
            "Hangi bölümü kastediyorsun: {choices}? Sen söyleyene kadar hiçbir şey kaydedilmez."
        }
        Or => " ya da ",
        Undone => {
            "Geri alındı: **{title}** hakkındaki iddian çekildi. Kayıt onu çekilmiş olarak tutar."
        }
        NothingToUndo => {
            "Geri alınacak bir şey yok. Yalnızca son adımındaki bir iddia geri alınabilir. Değerlendirilmiş bir karar, bir inceleme, bir devir, bir anlamadan devir ve sana söylenen bir yanıt olduğu gibi kalır."
        }
        NothingWaiting => "Onayını bekleyen bir şey yok.",
        NextStep => "Sıradaki: {at} · {question}",
        NextStepNoPlace => {
            "Sıradaki: kaynaklarında henüz **{concept}** geçen bir şey yok; nereye bakacağını bana sor. · {question}"
        }
        StepPart => "Bu bölüm ne işe yarıyor, kendi sözlerinle?",
        StepWideExample => "{concept} kavramının kullanıldığı bir yer bul. Orada ne yapıyor?",
        StepWidePurpose => "{concept} bu bölümde ne işe yarıyor, kendi sözlerinle?",
        StepWideMechanism => "{concept} adım adım nasıl çalışıyor?",
        StepNarrowExample => {
            "{concept} kavramının önemli olduğu bir satır seç. Farklı olsaydı ne değişirdi?"
        }
        StepNarrowPurpose => "{concept} neden tam orada gerekli ve o olmadan ne bozulur?",
        StepNarrowMechanism => "{concept} kavramını adım adım izle: önce ne olur, sonra ne?",
        Horizon => "Ufuk: {what}.",
        HorizonNeeds => "gerekenler: {parts}",
        HorizonLeads => "şuna götürür: {parts}",
        EvidenceRatio => "Geçilen: {passes} · iddia: {claims}.",
        ClaimWithdrawn => "**{concept}** hakkındaki iddia çekildi ({date}).",
        StepHistory => "Buraya nasıl geldin: {kinds}.",
        StepKindRate => "{kind}: {judged} adımdan {passes} tanesi geçişe götürdü",
        SteeredTimes => "{n} kez sen yönlendirdin",
        StepHistoryNote => {
            "Pathfinder'ın nasıl öğrendiğine dair tuttuğu her şey bu, bu klasörün yerel kaydında."
        }
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "açık",
        PartStatus::Understood => "anlaşıldı",
        PartStatus::InReview => "incelemede",
        PartStatus::Done => "anlaşıldı, tamam",
        PartStatus::Known => "bilindiği belirtildi",
        PartStatus::Delegated => "anlaşılmadan devredildi",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "bilinmiyor",
        ConceptState::Claimed => "iddia edildi",
        ConceptState::Delegated => "devredildi",
        ConceptState::Shaky => "sallantıda",
        ConceptState::Shown => "gösterildi",
        ConceptState::Solid => "sağlam",
    }
}
