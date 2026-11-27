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
        ShowKnown => {
            "Записано: ты уже знаешь **{title}**. Это заявление, не подтверждение. `/undo` (Esc) отменит его."
        }
        ShowTell => {
            "Отвечаю, как просили. Записано как сказанное, а не понятое. Esc до ответа отменяет его."
        }
        ShowHandoff => {
            "Передать **{title}** агенту? Он работает в копии для проверки, а ты проверяешь результат. Enter запускает · Esc отменяет."
        }
        ShowHandoffInPlace => {
            "Передать **{title}** агенту? Это не git-репозиторий: он меняет твои файлы напрямую, без копии и без проверенного ревью. Enter запускает · Esc отменяет."
        }
        ShowDelegate => {
            "Передать **{title}** без понимания? Будет записано как переданное: за результат ты не ручаешься. Enter запускает · Esc отменяет."
        }
        ShowVerdict => {
            "Считать это твоим вердиктом по кандидату? «{words}» Enter оценивает · Esc оставляет как заметку."
        }
        NotRunVerdict => "Не оценено как вердикт: оставлено как заметка.",
        NotRunHandoff => "Не запущено: **{title}** не передано агенту.",
        NotRunDelegate => "Не запущено: **{title}** не передано.",
        NotRunStale => "Не запущено: то, что ждало твоего OK, больше не применимо.",
        AskWhich => {
            "Ты имел в виду {choices}? Ничего не записано, пока ты не скажешь. Иначе просто продолжай."
        }
        AskKnown => "что ты уже знаешь **{title}** (`/known`)",
        AskTell => "чтобы тебе сказали (`/tell <question>`)",
        AskAgent => "отдать часть агенту (`/handoff` или `/delegate`)",
        Or => " или ",
        Undone => {
            "Отменено: твоё заявление о **{title}** отозвано. Запись хранит его как отозванное."
        }
        NothingToUndo => {
            "Нечего отменять. Отозвать можно только заявление с последнего шага. Оценённый вердикт, ревью, передача агенту, передача без понимания и полученный ответ остаются."
        }
        NothingWaiting => "Ничто не ждёт твоего OK.",
        NextStep => "Дальше: {at} · {question}",
        NextStepNoPlace => {
            "Дальше: в твоих источниках пока нет места, где названо **{concept}**; `/find <terms>` ищет в них. · {question}"
        }
        StepWideExample => "Найди место, где используется {concept}. Что оно там делает?",
        StepWidePurpose => "Для чего {concept} в этой части, своими словами?",
        StepWideMechanism => "Как работает {concept}, шаг за шагом?",
        StepNarrowExample => {
            "Выбери строку, где важно {concept}. Что изменилось бы, будь оно другим?"
        }
        StepNarrowPurpose => "Почему {concept} нужно именно там и что сломается без него?",
        StepNarrowMechanism => {
            "Проследи {concept} шаг за шагом: что происходит сначала, а что потом?"
        }
        Horizon => "Горизонт: {what}.",
        HorizonNeeds => "нужны {parts}",
        HorizonLeads => "ведёт к {parts}",
        EvidenceRatio => "Пройдено: {passes} · заявлений: {claims}.",
        ClaimWithdrawn => "Заявление о **{concept}** отозвано ({date}).",
        StepHistory => "Как ты сюда пришёл: {kinds}.",
        StepKindRate => "{kind}: {passes} из {judged} привели к успеху",
        SteeredTimes => "направлено тобой {n} раз",
        StepHistoryNote => {
            "Это всё, что Pathfinder хранит о том, как ты учишься, в локальной записи этой папки."
        }
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
