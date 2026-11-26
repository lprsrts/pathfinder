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
        PartHint => "Burada sesli düşün. `/summary` · `/known` · `/delegate`",
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
        AcceptReject => "`/accept` ya da `/reject`.",
        Needs => "gerektirir: {deps}",
        FixedReply => "Şu an bunu değerlendiriyorsun. Önce kararın.",
        GateCapped => {
            "(Bugünkü ortak kontroller bitti; adayla ilgili yanıtlar kararını bekliyor. Kendi anahtarın: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => "(Kontrole ulaşılamadı, yanıt bekletildi. Birazdan tekrar dene.)",
        Understood => "**{title}** anlaşıldı. `/next` · `/handoff`",
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
