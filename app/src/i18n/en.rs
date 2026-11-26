//! English.

use super::{Line, Table};
use crate::domain::PartStatus;
use crate::understanding::ConceptState;

pub(super) const TABLE: Table = Table { line, part, state };

fn line(l: Line) -> &'static str {
    use Line::*;
    match l {
        CandidateFor => "Candidate for **{title}**:",
        IsItRight => "Is it right? Say why.",
        PassedOneMore => "Passed. One more:",
        PartHint => "Think out loud here. `/summary` · `/known` · `/delegate`",
        RestsOn => "Rests on",
        ToldNotShown => "Recorded as told: {labels}.",
        GapNeeds => "It needs **{concept}** first: {why}.",
        GapClaimed => "you claimed to know it, with no evidence yet",
        GapDelegated => "you handed it off without understanding it",
        GapShaky => "your last check on it failed",
        GapUncovered => "nothing on the map covers it yet",
        NeededBy => "Needed by {part}.",
        AddGapParts => "Add a part for {names}?",
        And => " and ",
        ProposedChange => "Proposed map change:",
        AcceptReject => "`/accept` or `/reject`.",
        Needs => "needs {deps}",
        FixedReply => crate::secrets::FIXED_REPLY,
        GateCapped => {
            "(Shared checks used up for today; replies about the candidate wait for your verdict. Own key: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => "(Check unreachable, reply held back. Try again in a moment.)",
        Understood => "**{title}** understood. `/next` · `/handoff`",
        KeepWorking => "Keep working, then explain it again.",
        AllResolved => "Every part is resolved.",
        NoOtherPart => "No other unresolved part.",
        MapUpdated => "Map updated.",
        ChangeRejected => "Map change rejected.",
        NoChangeWaiting => "No map change is waiting.",
        RecordedKnown => "**{title}** recorded as already known.",
        Delegated => "**{title}** handed off without understanding; not reviewed.",
        QuickCheckLead => "Quick check:",
        ExamLead => "Exam practice, one question at a time.",
        ExamQuestionFor => "Question on **{title}**:",
        ExamAnswer => "Your answer?",
        RecordTitle => "**Your record in this folder**",
        ToldOnly => "told only",
        ReadOnly => "read",
        ReadAt => "Read:",
        Logged => "Logged.",
        NoRecord => "No record yet.",
        RecordSummary => "Shown: {shown} · told only: {told} · shaky: {shaky}",
        Checks => "{passed} passed, {failed} failed",
        EvidenceNote => "Only passed checks count.",
        OpenMisconception => "Open misconception on {concept}: \"{statement}\"",
        WhereToLook => "**Where to look:**",
        NotFoundOnline => "**Not found** here or online.",
        NotFoundHere => "**Not found** here.",
        TryTerms => "Try: {terms}",
        NotSearched => "Not searched: {services}.",
        Told => "**Told**",
        FirstWrongStep => "**First step that doesn't hold** ({at}):",
        NoWrongStep => "**No step found that doesn't hold.**",
        CheckUnclear => {
            "Couldn't point to a step in your work. Write it out one step per line and `/check` again."
        }
        StepAt => "line {n}",
        PhotoUnreadable => "Couldn't read the photo.",
        PhotoCopy => "Read from {files}. Check that this is what you wrote:",
        FindHits => "**{n}** for {terms}:",
        FindNone => "No line in this workspace contains {terms}.",
        WebNone => "Nothing online matched {terms}.",
    }
}

fn part(s: PartStatus) -> &'static str {
    s.label()
}

fn state(s: ConceptState) -> &'static str {
    s.label()
}
