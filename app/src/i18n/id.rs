//! Indonesian.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "Kandidat untuk **{title}**:",
        IsItRight => "Benar? Jelaskan alasannya.",
        PassedOneMore => "Lulus. Satu lagi:",
        PartHint => "Tulis jalan pikiranmu di sini. `/summary` · `/known` · `/delegate`",
        RestsOn => "Bergantung pada",
        ToldNotShown => "Dicatat sebagai diberi tahu: {labels}.",
        GapNeeds => "Perlu **{concept}** dulu: {why}.",
        GapClaimed => "kamu bilang sudah tahu, tapi belum ada bukti",
        GapDelegated => "kamu menyerahkannya tanpa memahaminya",
        GapShaky => "pemeriksaan terakhirmu gagal",
        GapUncovered => "belum ada bagian di peta yang mencakupnya",
        NeededBy => "Dibutuhkan oleh {part}.",
        AddGapParts => "Tambahkan bagian untuk {names}?",
        And => " dan ",
        ProposedChange => "Usulan perubahan peta:",
        AcceptReject => "`/accept` atau `/reject`.",
        Needs => "butuh {deps}",
        FixedReply => "Itu yang sedang kamu nilai. Putusanmu dulu.",
        GateCapped => {
            "(Pemeriksaan bersama untuk hari ini habis; balasan tentang kandidat menunggu putusanmu. Kunci sendiri: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => {
            "(Pemeriksaan tidak terjangkau, balasan ditahan. Coba lagi sebentar lagi.)"
        }
        Understood => "**{title}** dipahami. `/next` · `/handoff`",
        KeepWorking => "Lanjutkan, lalu jelaskan lagi.",
        AllResolved => "Semua bagian sudah selesai.",
        NoOtherPart => "Tidak ada bagian lain yang belum selesai.",
        MapUpdated => "Peta diperbarui.",
        ChangeRejected => "Perubahan peta ditolak.",
        NoChangeWaiting => "Tidak ada perubahan peta yang menunggu.",
        RecordedKnown => "**{title}** dicatat sebagai sudah diketahui.",
        Delegated => "**{title}** diserahkan tanpa dipahami; tidak ditinjau.",
        QuickCheckLead => "Cek singkat:",
        ExamLead => "Latihan ujian, satu pertanyaan setiap kali.",
        ExamQuestionFor => "Pertanyaan tentang **{title}**:",
        ExamAnswer => "Jawabanmu?",
        RecordTitle => "**Catatanmu di folder ini**",
        ToldOnly => "hanya diberi tahu",
        ReadOnly => "dibaca",
        ReadAt => "Baca:",
        Logged => "Dicatat.",
        NoRecord => "Belum ada catatan.",
        RecordSummary => "Ditunjukkan: {shown} · hanya diberi tahu: {told} · goyah: {shaky}",
        Checks => "{passed} lulus, {failed} gagal",
        EvidenceNote => "Hanya pemeriksaan yang lulus yang dihitung.",
        OpenMisconception => "Miskonsepsi terbuka tentang {concept}: \"{statement}\"",
        WhereToLook => "**Tempat mencari:**",
        NotFoundOnline => "**Tidak ditemukan** di sini maupun daring.",
        NotFoundHere => "**Tidak ditemukan** di sini.",
        TryTerms => "Coba: {terms}",
        NotSearched => "Tidak dicari: {services}.",
        Told => "**Diberi tahu**",
        FirstWrongStep => "**Langkah pertama yang tidak berlaku** ({at}):",
        NoWrongStep => "**Tidak ada langkah yang salah.**",
        CheckUnclear => {
            "Tidak bisa menunjuk langkah dalam pekerjaanmu. Tulis satu langkah per baris, lalu `/check` lagi."
        }
        StepAt => "baris {n}",
        PhotoUnreadable => "Foto tidak bisa dibaca.",
        PhotoCopy => "Dibaca dari {files}. Periksa apakah ini yang kamu tulis:",
        FindHits => "**{n}** untuk {terms}:",
        FindNone => "Tidak ada baris di folder ini yang memuat {terms}.",
        WebNone => "Tidak ada hasil daring untuk {terms}.",
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "terbuka",
        PartStatus::Understood => "dipahami",
        PartStatus::InReview => "sedang ditinjau",
        PartStatus::Done => "dipahami, selesai",
        PartStatus::Known => "diklaim sudah tahu",
        PartStatus::Delegated => "diserahkan tanpa dipahami",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "tidak diketahui",
        ConceptState::Claimed => "diklaim",
        ConceptState::Delegated => "diserahkan",
        ConceptState::Shaky => "goyah",
        ConceptState::Shown => "ditunjukkan",
        ConceptState::Solid => "kokoh",
    }
}
