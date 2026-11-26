//! Russian.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "Кандидат по теме **{title}**:",
        IsItRight => "Верно? Скажи почему.",
        PassedOneMore => "Зачтено. Ещё один:",
        PartHint => "Рассуждай здесь вслух. `/summary` · `/known` · `/delegate`",
        RestsOn => "Опирается на",
        ToldNotShown => "Записано как сказанное: {labels}.",
        GapNeeds => "Сначала нужно **{concept}**: {why}.",
        GapClaimed => "знание заявлено, но пока не подтверждено",
        GapDelegated => "передано без понимания",
        GapShaky => "последняя проверка не пройдена",
        GapUncovered => "на карте это пока ничем не покрыто",
        NeededBy => "Нужно для {part}.",
        AddGapParts => "Добавить часть для {names}?",
        And => " и ",
        ProposedChange => "Предлагаемое изменение карты:",
        AcceptReject => "`/accept` или `/reject`.",
        Needs => "требует {deps}",
        FixedReply => "Это то, что ты сейчас оцениваешь. Сначала твой вердикт.",
        GateCapped => {
            "(Общие проверки на сегодня закончились; ответы о кандидате ждут твоего вердикта. Свой ключ: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => "(Проверка недоступна, ответ задержан. Попробуй ещё раз чуть позже.)",
        Understood => "**{title}**: понято. `/next` · `/handoff`",
        KeepWorking => "Продолжай работать, потом объясни ещё раз.",
        AllResolved => "Все части решены.",
        NoOtherPart => "Других нерешённых частей нет.",
        MapUpdated => "Карта обновлена.",
        ChangeRejected => "Изменение карты отклонено.",
        NoChangeWaiting => "Нет ожидающих изменений карты.",
        RecordedKnown => "**{title}**: записано как уже известное.",
        Delegated => "**{title}**: передано без понимания; не проверено.",
        QuickCheckLead => "Быстрая проверка:",
        ExamLead => "Подготовка к экзамену, по одному вопросу.",
        ExamQuestionFor => "Вопрос по теме **{title}**:",
        ExamAnswer => "Твой ответ?",
        RecordTitle => "**Твоя история в этой папке**",
        ToldOnly => "только сказано",
        ReadOnly => "прочитано",
        ReadAt => "Читать:",
        Logged => "Записано.",
        NoRecord => "Истории пока нет.",
        RecordSummary => "Показано: {shown} · только сказано: {told} · шатко: {shaky}",
        Checks => "пройдено {passed}, не пройдено {failed}",
        EvidenceNote => "Считаются только пройденные проверки.",
        OpenMisconception => "Открытое заблуждение по теме {concept}: «{statement}»",
        WhereToLook => "**Где смотреть:**",
        NotFoundOnline => "**Не найдено** ни здесь, ни в сети.",
        NotFoundHere => "**Не найдено** здесь.",
        TryTerms => "Попробуй: {terms}",
        NotSearched => "Не искали: {services}.",
        Told => "**Сказано**",
        FirstWrongStep => "**Первый неверный шаг** ({at}):",
        NoWrongStep => "**Неверных шагов не найдено.**",
        CheckUnclear => {
            "Не удалось указать шаг в твоей работе. Запиши по одному шагу в строке и снова используй `/check`."
        }
        StepAt => "строка {n}",
        PhotoUnreadable => "Не удалось прочитать фото.",
        PhotoCopy => "Прочитано из {files}. Проверь, совпадает ли это с твоей записью:",
        FindHits => "**{n}** по запросу {terms}:",
        FindNone => "Ни одна строка в этой папке не содержит {terms}.",
        WebNone => "В сети ничего не нашлось по запросу {terms}.",
    }
}

fn part(s: PartStatus) -> &'static str {
    match s {
        PartStatus::Open => "открыта",
        PartStatus::Understood => "понята",
        PartStatus::InReview => "на проверке",
        PartStatus::Done => "понята, готова",
        PartStatus::Known => "заявлена как известная",
        PartStatus::Delegated => "передана без понимания",
    }
}

fn state(s: ConceptState) -> &'static str {
    match s {
        ConceptState::Unknown => "неизвестно",
        ConceptState::Claimed => "заявлено",
        ConceptState::Delegated => "передано",
        ConceptState::Shaky => "шатко",
        ConceptState::Shown => "показано",
        ConceptState::Solid => "прочно",
    }
}
