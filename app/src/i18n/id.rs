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
        PartHint => {
            "Tulis jalan pikiranmu di sini: alasanmu sambil bekerja, lalu bagian ini dengan kata-katamu sendiri saat kamu siap. Kalau sudah tahu atau ingin agen yang mengerjakannya, bilang saja."
        }
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
        AcceptReject => "Bilang ya untuk menambahkannya, atau tidak.",
        Needs => "butuh {deps}",
        FixedReply => "Itu yang sedang kamu nilai. Putusanmu dulu.",
        GateCapped => {
            "(Pemeriksaan bersama untuk hari ini habis; balasan tentang kandidat menunggu putusanmu. Kunci sendiri: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => {
            "(Pemeriksaan tidak terjangkau, balasan ditahan. Coba lagi sebentar lagi.)"
        }
        Understood => {
            "**{title}** dipahami. Bilang kalau kamu mau lanjut atau menyerahkannya ke agen."
        }
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
        ShowKnown => {
            "Dicatat: kamu sudah tahu **{title}**. Sebuah klaim, bukan bukti. Bilang \"batalkan\" atau tekan Esc untuk menariknya kembali."
        }
        ShowTell => {
            "Aku beri tahu, seperti yang kamu minta. Dicatat sebagai diberi tahu, tidak pernah sebagai dipahami. Esc sebelum jawabannya tiba membatalkannya."
        }
        ShowHandoff => {
            "Serahkan **{title}** ke agen? Ia bekerja di salinan staging dan kamu meninjau hasilnya. Enter jalankan · Esc batal."
        }
        ShowHandoffInPlace => {
            "Serahkan **{title}** ke agen? Ini bukan repositori git: ia mengubah berkasmu langsung, tanpa staging atau tinjauan yang diperiksa. Enter jalankan · Esc batal."
        }
        ShowDelegate => {
            "Serahkan **{title}** tanpa memahaminya? Dicatat sebagai diserahkan: kamu tidak menjamin hasilnya. Enter jalankan · Esc batal."
        }
        ShowVerdict => {
            "Nilai ini sebagai putusanmu atas kandidat? “{words}” Enter menilai · Esc menyimpannya sebagai catatan."
        }
        NotRunVerdict => "Tidak dinilai sebagai putusan: disimpan sebagai catatan.",
        NotRunHandoff => "Tidak dijalankan: **{title}** tidak diserahkan ke agen.",
        NotRunDelegate => "Tidak dijalankan: **{title}** tidak diserahkan.",
        NotRunStale => "Tidak dijalankan: yang menunggu OK-mu sudah tidak berlaku.",
        AskWhich => {
            "Maksudmu yang mana: {choices}? Tidak ada yang dicatat sampai kamu bilang; cukup nomor atau beberapa kata."
        }
        AskKnown => "catat bahwa saya sudah tahu **{title}** (tanpa pengecekan)",
        AskTell => "langsung beri tahu saya",
        AskAgent => "biar agen yang mengerjakan bagian ini",
        AskCheck => "uji saya tentang **{title}**",
        AskNote => "hanya catatan",
        AskWhichPart => {
            "Bagian mana yang kamu maksud: {choices}? Tidak ada yang dicatat sampai kamu bilang."
        }
        Or => " atau ",
        Undone => {
            "Dibatalkan: klaimmu atas **{title}** ditarik. Catatan menyimpannya sebagai ditarik."
        }
        NothingToUndo => {
            "Tidak ada yang bisa dibatalkan. Hanya klaim dari langkah terakhirmu yang bisa ditarik. Putusan yang sudah dinilai, tinjauan, penyerahan, delegasi, dan jawaban yang diberitahukan padamu tetap seperti adanya."
        }
        NothingWaiting => "Tidak ada yang menunggu OK-mu.",
        NextStep => "Berikutnya: {at} · {question}",
        NextStepNoPlace => {
            "Berikutnya: belum ada di sumbermu yang menyebut **{concept}**; tanyakan padaku di mana mencarinya. · {question}"
        }
        StepPart => "Untuk apa bagian ini, dengan kata-katamu sendiri?",
        StepWideExample => "Temukan satu tempat di mana {concept} dipakai. Apa fungsinya di sana?",
        StepWidePurpose => "Untuk apa {concept} di bagian ini, dengan kata-katamu sendiri?",
        StepWideMechanism => "Bagaimana {concept} bekerja, langkah demi langkah?",
        StepNarrowExample => {
            "Pilih satu baris di mana {concept} penting. Apa yang berubah jika berbeda?"
        }
        StepNarrowPurpose => {
            "Mengapa {concept} diperlukan tepat di sana, dan apa yang rusak tanpanya?"
        }
        StepNarrowMechanism => {
            "Telusuri {concept} langkah demi langkah: apa yang terjadi dulu, lalu apa?"
        }
        Horizon => "Cakrawala: {what}.",
        HorizonNeeds => "perlu {parts}",
        HorizonLeads => "menuju {parts}",
        EvidenceRatio => "Lulus: {passes} · klaim: {claims}.",
        ClaimWithdrawn => "Klaim atas **{concept}** ditarik ({date}).",
        StepHistory => "Bagaimana kamu sampai di sini: {kinds}.",
        StepKindRate => "{kind} {passes} dari {judged} berujung lulus",
        SteeredTimes => "diarahkan olehmu {n} kali",
        StepHistoryNote => {
            "Hanya itu yang disimpan Pathfinder tentang caramu belajar, di catatan lokal folder ini."
        }
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
