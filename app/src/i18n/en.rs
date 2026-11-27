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
        PartHint => {
            "Think out loud here: your reasoning as you go, then the part in your own words when you're ready. If you already know it, or want the agent to do it, just say so."
        }
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
        AcceptReject => "Say yes to add it, or no.",
        Needs => "needs {deps}",
        FixedReply => crate::secrets::FIXED_REPLY,
        GateCapped => {
            "(Shared checks used up for today; replies about the candidate wait for your verdict. Own key: `pathfinder auth set typesafe`.)"
        }
        GateUnavailable => "(Check unreachable, reply held back. Try again in a moment.)",
        Understood => {
            "**{title}** understood. Say when you want to go on, or to hand it to the agent."
        }
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
        ShowKnown => {
            "Recorded: you already know **{title}**. A claim, not evidence. Say undo, or press Esc, to take it back."
        }
        ShowTell => {
            "Telling you, as you asked. Recorded as told, never as understood. Esc before it arrives cancels it."
        }
        ShowHandoff => {
            "Hand **{title}** to the agent? It works in a staging copy and you review the result. Enter runs · Esc cancels."
        }
        ShowHandoffInPlace => {
            "Hand **{title}** to the agent? This isn't a git repository, so it edits your files directly, with no staging or checked review. Enter runs · Esc cancels."
        }
        ShowDelegate => {
            "Delegate **{title}** without understanding it? It's recorded as delegated: you won't vouch for the result. Enter runs · Esc cancels."
        }
        ShowVerdict => {
            "Judge this as your verdict on the candidate? “{words}” Enter judges · Esc keeps it as a note."
        }
        NotRunVerdict => "Not judged as a verdict: kept as a note.",
        NotRunHandoff => "Not run: **{title}** wasn't handed to the agent.",
        NotRunDelegate => "Not run: **{title}** wasn't delegated.",
        NotRunStale => "Not run: what waited for your OK no longer applies.",
        AskWhich => {
            "Which did you mean: {choices}? Nothing is recorded until you say; a number or a few words is enough."
        }
        AskKnown => "record that I already know **{title}** (no check)",
        AskTell => "just tell me",
        AskAgent => "let the agent do this part",
        AskCheck => "check me on **{title}**",
        AskNote => "just a note",
        AskWhichPart => "Which part do you mean: {choices}? Nothing is recorded until you say.",
        Or => " or ",
        Undone => {
            "Undone: your claim on **{title}** is withdrawn. The record keeps it as withdrawn."
        }
        NothingToUndo => {
            "Nothing to undo. Only a claim from your last step can be taken back. A judged verdict, a review, a handoff, a delegation and an answer you were told stay as they are."
        }
        NothingWaiting => "Nothing is waiting for your OK.",
        NextStep => "Next: {at} · {question}",
        NextStepNoPlace => {
            "Next: nothing in your sources names **{concept}** yet; ask me where to look. · {question}"
        }
        StepPart => "What is this part for, in your own words?",
        StepWideExample => "Find one place where “{concept}” comes up. What happens there?",
        StepWidePurpose => {
            "What role does “{concept}” play in this part? Put it in your own words."
        }
        StepWideMechanism => "How does “{concept}” work here, step by step?",
        StepNarrowExample => {
            "Pick one line where “{concept}” matters. What would change if it were different?"
        }
        StepNarrowPurpose => "What would break exactly there without “{concept}”?",
        StepNarrowMechanism => "Trace “{concept}” step by step: what happens first, and what next?",
        Horizon => "Horizon: {what}.",
        HorizonNeeds => "needs {parts}",
        HorizonLeads => "leads to {parts}",
        EvidenceRatio => "Passes: {passes} · claims: {claims}.",
        ClaimWithdrawn => "Claim on **{concept}** withdrawn ({date}).",
        StepHistory => "How you got here: {kinds}.",
        StepKindRate => "{kind} {passes} of {judged} led to a pass",
        SteeredTimes => "steered by you {n} times",
        StepHistoryNote => {
            "That is all Pathfinder keeps about how you learn, in this folder's local record."
        }
    }
}

fn part(s: PartStatus) -> &'static str {
    s.label()
}

fn state(s: ConceptState) -> &'static str {
    s.label()
}
