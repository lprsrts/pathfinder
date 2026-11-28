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
        DelegateWhy => "Bukan serah terima dengan tinjauanmu: kamu belum menunjukkan {concepts}.",
        ShowVerdict => {
            "Nilai ini sebagai putusanmu atas kandidat? “{words}” Enter menilai · Esc menyimpannya sebagai catatan."
        }
        ShowApprove => "Setujui hasil agen untuk **{title}**? Enter menyetujui · Esc membatalkan.",
        ShowObject => {
            "Kembalikan **{title}** ke agen dengan keberatanmu? “{words}” Enter mengirimnya · Esc membatalkan."
        }
        NotRunVerdict => "Tidak dinilai sebagai putusan: disimpan sebagai catatan.",
        NotRunHandoff => "Tidak dijalankan: **{title}** tidak diserahkan ke agen.",
        NotRunDelegate => "Tidak dijalankan: **{title}** tidak diserahkan.",
        NotRunApprove => "Tidak dijalankan: **{title}** tidak disetujui.",
        NotRunObject => "Tidak dijalankan: keberatanmu tentang **{title}** tidak dikirim.",
        NotRunStale => "Tidak dijalankan: yang menunggu OK-mu sudah tidak berlaku.",
        AskWhich => {
            "Maksudmu yang mana: {choices}? Tidak ada yang dicatat sampai kamu bilang; cukup nomor atau beberapa kata."
        }
        AskAgentPart => "biarkan agen mengerjakan **{title}**",
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
        EvidenceRatio => {
            "Dari ide di atas, {passes} lulus pemeriksaan dan {claims} hanya bersandar pada kata-katamu."
        }
        ClaimWithdrawn => "Klaim atas **{concept}** ditarik ({date}).",
        StepHistory => "Yang sejauh ini berujung lulus: {kinds}.",
        StepKindRate => "{kind}, {passes} dari {judged} kali",
        SteeredTimes => "diarahkan olehmu {n} kali",
        AngleExample => "mulai dari contoh",
        AnglePurpose => "bertanya untuk apa",
        AngleMechanism => "menelusuri cara kerjanya",
        AgentWork => "**Diserahkan ke agen**",
        AgentInReview => {
            "**{title}**: dapat ditinjau, diserahkan setelah kamu menunjukkan dasarnya; menunggu tinjauanmu."
        }
        AgentApproved => {
            "**{title}**: dapat ditinjau, diserahkan setelah kamu menunjukkan dasarnya; kamu menyetujuinya."
        }
        AgentMissedCheck => {
            "**{title}**: disetujui tanpa menangkap pemeriksaannya; tidak dicatat sebagai tinjauan, kamu tidak menjaminnya."
        }
        AgentDelegated => {
            "**{title}**: didelegasikan, dikerjakan tanpa kamu pahami; kamu tidak menjaminnya."
        }
        AgentCaughtCheck => {
            "**{title}**: dapat ditinjau; dalam tinjauannya kamu menangkap pemeriksaan yang disisipkan dan berkeberatan, jadi hasil asli agen yang ditulis."
        }
        ShowAgentParts => {
            "Biarkan agen mengerjakan bagian-bagian ini satu per satu? {parts}. Enter jalankan semua · Esc batal."
        }
        AgentPartHandoff => "**{title}** (serah terima: kamu meninjau hasilnya)",
        AgentPartDelegate => "**{title}** (diserahkan: kamu tidak menjaminnya)",
        AgentPartDelegateWhy => "**{title}** (diserahkan: kamu belum menunjukkan {concepts})",
        ConfirmHandoff => "ya, serahkan",
        ConfirmDelegate => "ya, delegasikan",
        ConfirmVerdict => "ya, nilai",
        ConfirmApprove => "ya, setujui",
        ConfirmObject => "ya, kembalikan",
        CancelShown => "tidak",
        ShowKnownShown => {
            "Dicatat: kamu sudah tahu **{title}**. Kamu sudah menunjukkan setiap ide yang menjadi dasarnya, jadi ia berdiri di atas pemeriksaanmu, bukan hanya kata-katamu. Bilang \"batalkan\" atau tekan Esc untuk menariknya kembali."
        }
        ShowKnownSome => {
            "Dicatat: kamu sudah tahu **{title}**. Hanya kata-katamu untuk {concepts}; sisanya sudah kamu tunjukkan. Bilang \"batalkan\" atau tekan Esc untuk menariknya kembali."
        }
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
