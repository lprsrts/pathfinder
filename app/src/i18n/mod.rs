//! The fixed lines Pathfinder writes itself, in the conversation's
//! language. Models write everything else in the user's language already;
//! these lines are the ones that would otherwise stay English.
//!
//! One file per language. A language missing here falls back to English.

use crate::domain::PartStatus;
use crate::understanding::ConceptState;

mod ar;
mod bn;
mod de;
mod en;
mod es;
mod fr;
mod hi;
mod id;
mod it;
mod ja;
mod ko;
mod pt;
mod ru;
mod tr;
mod ur;
mod vi;
mod zh;

/// A line Pathfinder writes itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// Before a candidate: "{title}".
    CandidateFor,
    /// After a candidate.
    IsItRight,
    /// A pass that doesn't settle the part yet.
    PassedOneMore,
    /// Under the first part's card.
    PartHint,
    /// Before the concepts a part rests on.
    RestsOn,
    /// After an answer that names concepts on the record: "{labels}".
    ToldNotShown,
    /// A gap under the part: "{concept}", "{why}".
    GapNeeds,
    GapClaimed,
    GapDelegated,
    GapShaky,
    GapUncovered,
    /// A proposed gap part's note: "{part}".
    NeededBy,
    /// A gap proposal's brief: "{names}".
    AddGapParts,
    /// Joins names in a list.
    And,
    /// Before a proposed map change.
    ProposedChange,
    /// After a proposed map change.
    AcceptReject,
    /// A proposed part's dependencies: "{deps}".
    Needs,
    /// The gate's reply when a draft can't be shown.
    FixedReply,
    /// After the fixed reply: the shared gate's daily checks are used up.
    GateCapped,
    /// After the fixed reply: the gate's service couldn't be reached.
    GateUnavailable,
    /// A part judged understood: "{title}".
    Understood,
    /// A failed check.
    KeepWorking,
    AllResolved,
    NoOtherPart,
    MapUpdated,
    ChangeRejected,
    NoChangeWaiting,
    /// A part skipped as known: "{title}".
    RecordedKnown,
    /// A part delegated: "{title}".
    Delegated,
    /// Before a quick check's candidate.
    QuickCheckLead,
    /// Before an exam's first question.
    ExamLead,
    /// Before an exam question: "{title}".
    ExamQuestionFor,
    /// After an exam question.
    ExamAnswer,
    /// Heading of the folder-wide record.
    RecordTitle,
    /// A concept you were only told about.
    ToldOnly,
    ReadOnly,
    /// Before a part's pointer into the workspace.
    ReadAt,
    /// A log entry, taken without comment.
    Logged,
    /// The folder has no record yet.
    NoRecord,
    /// Under the record: "{shown}", "{told}", "{shaky}".
    RecordSummary,
    /// Checks on a concept: "{passed}", "{failed}".
    Checks,
    /// Under a part's `/why`.
    EvidenceNote,
    /// An open misconception: "{concept}", "{statement}".
    OpenMisconception,
    /// Before sourced hits.
    WhereToLook,
    /// No hit in the workspace or online.
    NotFoundOnline,
    /// No hit in the workspace.
    NotFoundHere,
    /// Suggested searches: "{terms}".
    TryTerms,
    /// Services that couldn't be searched: "{services}".
    NotSearched,
    /// Heading of a `/tell` answer.
    Told,
    /// `/check` found a wrong step: "{at}".
    FirstWrongStep,
    /// `/check` found none.
    NoWrongStep,
    /// `/check` couldn't place the step.
    CheckUnclear,
    /// A line of typed work: "{n}".
    StepAt,
    /// `/check` couldn't copy a photo.
    PhotoUnreadable,
    /// Before the copy of a photo: "{files}".
    PhotoCopy,
    /// `/find` hits: "{n}", "{terms}".
    FindHits,
    /// `/find` found nothing: "{terms}".
    FindNone,
    /// `/web` found nothing: "{terms}".
    WebNone,
    /// A claim read from plain words, shown back as it is recorded: "{title}".
    ShowKnown,
    /// Before an answer asked for in plain words.
    ShowTell,
    /// A plain handoff waiting for Enter: "{title}".
    ShowHandoff,
    /// The same outside a git repository: "{title}".
    ShowHandoffInPlace,
    /// A plain delegation waiting for Enter: "{title}".
    ShowDelegate,
    /// A verdict read from plain words, waiting for Enter: "{words}".
    ShowVerdict,
    /// A verdict you didn't confirm.
    NotRunVerdict,
    /// A handoff you didn't confirm: "{title}".
    NotRunHandoff,
    /// A delegation you didn't confirm: "{title}".
    NotRunDelegate,
    /// What waited no longer applies.
    NotRunStale,
    /// An unsure commitment: "{choices}".
    AskWhich,
    /// One choice of `AskWhich`: "{title}".
    AskKnown,
    /// One choice of `AskWhich`.
    AskTell,
    /// One choice of `AskWhich`.
    AskAgent,
    /// A choice offered by the unsure Router: be checked on "{title}".
    AskCheck,
    /// A choice offered by the unsure Router: keep the message as a note.
    AskNote,
    /// Which part a message meant: "{choices}".
    AskWhichPart,
    /// Between the choices of `AskWhich`.
    Or,
    /// A claim taken back: "{title}".
    Undone,
    /// `/undo` with nothing undoable.
    NothingToUndo,
    /// `/confirm` or `/cancel` with nothing waiting.
    NothingWaiting,
    /// The next step (§16): "{at}", "{question}".
    NextStep,
    /// The next step with no place found: "{concept}", "{question}".
    NextStepNoPlace,
    /// The open question for a part with no concept to aim at yet.
    StepPart,
    /// An orienting step, example first: "{concept}".
    StepWideExample,
    /// An orienting step, purpose first: "{concept}".
    StepWidePurpose,
    /// An orienting step, mechanism first: "{concept}".
    StepWideMechanism,
    /// A narrow step, example first: "{concept}".
    StepNarrowExample,
    /// A narrow step, purpose first: "{concept}".
    StepNarrowPurpose,
    /// A narrow step, mechanism first: "{concept}".
    StepNarrowMechanism,
    /// The horizon (§16): "{what}".
    Horizon,
    /// Parts a part needs: "{parts}".
    HorizonNeeds,
    /// Parts that need it: "{parts}".
    HorizonLeads,
    /// Passes against claims (A10): "{passes}", "{claims}".
    EvidenceRatio,
    /// A claim `/undo` took back: "{concept}", "{date}".
    ClaimWithdrawn,
    /// Which steps led to passes: "{kinds}".
    StepHistory,
    /// One kind of step: "{kind}", "{passes}", "{judged}".
    StepKindRate,
    /// Steps you steered: "{n}".
    SteeredTimes,
    /// Under the step history.
    StepHistoryNote,
}

/// One language's lines and names.
pub(crate) struct Table {
    line: fn(Line) -> &'static str,
    part: fn(PartStatus) -> &'static str,
    state: fn(ConceptState) -> &'static str,
}

/// The languages with their own lines, by two-letter code.
const LANGS: &[(&str, Table)] = &[
    ("en", en::TABLE),
    ("zh", zh::TABLE),
    ("hi", hi::TABLE),
    ("es", es::TABLE),
    ("ar", ar::TABLE),
    ("fr", fr::TABLE),
    ("bn", bn::TABLE),
    ("pt", pt::TABLE),
    ("ru", ru::TABLE),
    ("ur", ur::TABLE),
    ("id", id::TABLE),
    ("de", de::TABLE),
    ("ja", ja::TABLE),
    ("tr", tr::TABLE),
    ("ko", ko::TABLE),
    ("it", it::TABLE),
    ("vi", vi::TABLE),
];

fn table(lang: Option<&str>) -> &'static Table {
    let found = lang
        .and_then(code)
        .and_then(|l| LANGS.iter().find(|(c, _)| c.eq_ignore_ascii_case(&l)));
    &found.unwrap_or(&LANGS[0]).1
}

/// The two-letter code in a language tag: "pt" for "pt-BR", "zh" for
/// "zh_Hans". None for anything that doesn't start with one.
pub fn code(tag: &str) -> Option<String> {
    let primary = tag.trim().split(['-', '_']).next()?;
    (primary.len() == 2 && primary.chars().all(|c| c.is_ascii_alphabetic()))
        .then(|| primary.to_ascii_lowercase())
}

/// The line in `lang` (a two-letter code); English when it's unknown.
pub fn line(lang: Option<&str>, line: Line) -> &'static str {
    (table(lang).line)(line)
}

/// A part status's name in `lang`.
pub fn part_status(lang: Option<&str>, s: PartStatus) -> &'static str {
    (table(lang).part)(s)
}

/// A concept state's name in `lang`.
pub fn state(lang: Option<&str>, s: ConceptState) -> &'static str {
    (table(lang).state)(s)
}

/// Whether `text` opens with `line`'s header (one with a "{title}") in any
/// language.
pub fn opens_with(text: &str, which: Line) -> bool {
    let first = text.lines().next().unwrap_or("");
    LANGS.iter().any(|(_, t)| {
        let Some((pre, post)) = (t.line)(which).split_once("{title}") else {
            return false;
        };
        first.len() >= pre.len() + post.len() && first.starts_with(pre) && first.ends_with(post)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regional_tags_use_their_language() {
        assert_eq!(code("pt-BR").as_deref(), Some("pt"));
        assert_eq!(code("zh_Hans").as_deref(), Some("zh"));
        assert_eq!(code("DE").as_deref(), Some("de"));
        assert_eq!(code("deu"), None);
        assert_eq!(code(""), None);
        assert_eq!(
            line(Some("pt-BR"), Line::FixedReply),
            line(Some("pt"), Line::FixedReply)
        );
        assert_ne!(
            line(Some("pt-BR"), Line::FixedReply),
            line(None, Line::FixedReply)
        );
    }

    const ALL_LINES: &[Line] = &[
        Line::CandidateFor,
        Line::IsItRight,
        Line::PassedOneMore,
        Line::PartHint,
        Line::RestsOn,
        Line::ToldNotShown,
        Line::GapNeeds,
        Line::GapClaimed,
        Line::GapDelegated,
        Line::GapShaky,
        Line::GapUncovered,
        Line::NeededBy,
        Line::AddGapParts,
        Line::And,
        Line::ProposedChange,
        Line::AcceptReject,
        Line::Needs,
        Line::FixedReply,
        Line::GateCapped,
        Line::GateUnavailable,
        Line::Understood,
        Line::KeepWorking,
        Line::AllResolved,
        Line::NoOtherPart,
        Line::MapUpdated,
        Line::ChangeRejected,
        Line::NoChangeWaiting,
        Line::RecordedKnown,
        Line::Delegated,
        Line::QuickCheckLead,
        Line::ExamLead,
        Line::ExamQuestionFor,
        Line::ExamAnswer,
        Line::RecordTitle,
        Line::ToldOnly,
        Line::ReadOnly,
        Line::ReadAt,
        Line::Logged,
        Line::NoRecord,
        Line::RecordSummary,
        Line::Checks,
        Line::EvidenceNote,
        Line::OpenMisconception,
        Line::WhereToLook,
        Line::NotFoundOnline,
        Line::NotFoundHere,
        Line::TryTerms,
        Line::NotSearched,
        Line::Told,
        Line::FirstWrongStep,
        Line::NoWrongStep,
        Line::CheckUnclear,
        Line::StepAt,
        Line::PhotoUnreadable,
        Line::PhotoCopy,
        Line::FindHits,
        Line::FindNone,
        Line::WebNone,
        Line::ShowKnown,
        Line::ShowTell,
        Line::ShowHandoff,
        Line::ShowHandoffInPlace,
        Line::ShowDelegate,
        Line::ShowVerdict,
        Line::NotRunVerdict,
        Line::NotRunHandoff,
        Line::NotRunDelegate,
        Line::NotRunStale,
        Line::AskWhich,
        Line::AskKnown,
        Line::AskTell,
        Line::AskAgent,
        Line::AskCheck,
        Line::AskNote,
        Line::AskWhichPart,
        Line::Or,
        Line::Undone,
        Line::NothingToUndo,
        Line::NothingWaiting,
        Line::NextStep,
        Line::NextStepNoPlace,
        Line::StepPart,
        Line::StepWideExample,
        Line::StepWidePurpose,
        Line::StepWideMechanism,
        Line::StepNarrowExample,
        Line::StepNarrowPurpose,
        Line::StepNarrowMechanism,
        Line::Horizon,
        Line::HorizonNeeds,
        Line::HorizonLeads,
        Line::EvidenceRatio,
        Line::ClaimWithdrawn,
        Line::StepHistory,
        Line::StepKindRate,
        Line::SteeredTimes,
        Line::StepHistoryNote,
    ];

    fn placeholders(s: &str) -> Vec<&str> {
        let mut out: Vec<&str> = s
            .match_indices('{')
            .filter_map(|(i, _)| s[i..].find('}').map(|j| &s[i..i + j + 1]))
            .collect();
        out.sort_unstable();
        out
    }

    fn commands(s: &str) -> Vec<&str> {
        let mut out: Vec<&str> = s.split('`').skip(1).step_by(2).collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn the_conversation_language_or_english() {
        assert_eq!(line(Some("de"), Line::IsItRight), "Stimmt das? Sag, warum.");
        assert_eq!(line(Some("xx"), Line::IsItRight), "Is it right? Say why.");
        assert_eq!(line(None, Line::RestsOn), "Rests on");
        assert_eq!(state(Some("DE"), ConceptState::Shaky), "wackelig");
        assert_eq!(line(Some("tr"), Line::Logged), "Not alındı.");
    }

    #[test]
    fn every_language_keeps_placeholders_and_commands() {
        for (code, t) in LANGS {
            for &l in ALL_LINES {
                let (en, own) = (line(None, l), (t.line)(l));
                assert!(!own.trim().is_empty(), "{code} {l:?} is empty");
                assert_eq!(placeholders(en), placeholders(own), "{code} {l:?}");
                assert_eq!(commands(en), commands(own), "{code} {l:?}");
                if en.starts_with("**") {
                    assert!(own.starts_with("**"), "{code} {l:?} bold");
                }
            }
        }
    }

    #[test]
    fn headers_are_recognised_in_every_language() {
        for (code, _) in LANGS {
            let h = line(Some(code), Line::ExamQuestionFor).replace("{title}", "Phasors");
            assert!(
                opens_with(&format!("{h}\n\nWhy?"), Line::ExamQuestionFor),
                "{code}"
            );
        }
        assert!(!opens_with("Just a reply.", Line::ExamQuestionFor));
    }
}
