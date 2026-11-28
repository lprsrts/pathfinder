# Pathfinder theory

*2026-11-27. This is the canonical theory. Prompts, critics and code follow from it. A feature that can't point to a section here doesn't belong, and a change to the theory is made here first.*

## 1. Purpose

Pathfinder is a human-in-the-loop harness. It makes sure you understand a task before the work is handed to an agent.

It is for all kinds of real work: coding, hardware, analysis, research, study and learning itself.

## 2. Understanding means you could judge the work

Understanding a task means you know what has to be done and why, well enough to:

- direct the agent, and
- tell whether its result is right.

You don't need to be able to do the work yourself.

## 3. The map

Onboarding sets the goal and sketches what the task depends on. That sketch is a rough map:

- It has no size limit. It is as big as the task needs.
- It grows and changes as the work reveals more.

## 4. You decide the split during the work

Which parts you do yourself and which go to the agent is not decided up front. It comes up during the related work, and you raise it. The model follows your call.

## 5. Evidence of understanding

Understanding is shown in three stages:

1. **Log.** While you work, you log your thinking and the reasons behind your steps and choices. This is where the evidence starts.
2. **Summary.** You explain the part as a summary.
3. **Candidate.** You're shown a candidate (an approach or a result) and judge whether it is right and why. This decides it.

## 6. Two kinds of skip

You can skip any part at any point. A skip is one of two kinds, recorded separately:

- **Already known.** Your word is enough. It is recorded as your claim, distinct from understanding shown through section 5.
- **Not understanding.** You hand the part off without understanding it, and you won't be able to judge its result.

## 7. Handoff and review

- After the agent does a part, you review its result. That's what understanding was for.
- When you find a problem, your objection and your reason go back to the agent, and you review again.

## 8. Record

These outcomes are kept distinct, in the session and, eventually, across sessions:

- understood (section 5),
- claimed as already known,
- handed off without understanding.

## 9. Why a harness

Sections 1–8 can be asked of any agent in plain words. An agent told to check your understanding is still the one that knows the answers, writes the record and does the work, so it can give the answer away, mark you as understanding, or act on what you can't judge, and nothing outside it would notice.

Pathfinder keeps these guarantees outside any model:

- what each model can see (section 11),
- what reaches you (section 11),
- what counts as evidence of your understanding (section 10),
- how the agent's work is labelled (section 12).

A model can propose; only the harness records. This is what Pathfinder adds to an agent, and what a set of instructions to an agent can't.

## 10. The understanding model

Pathfinder keeps a model of what you understand, finer than the map's parts:

- **Concepts** are the ideas the parts rest on, linked by what each needs first. Parts stay how you work through the task; concepts are what the record is made of.
- For each concept it keeps, separately:
  - your **claims** ("I know this"), as claims;
  - **evidence**: dated candidates you judged, with your reasons and whether you were right;
  - **misconceptions** your log or your judgments showed.
- Only evidence counts as understanding. A claim never becomes evidence, and a model's impression of your log ("seems to get it") is neither.

It is built as you work. Your log adds the concepts you touched and flags suspected misconceptions; your judgments add evidence. Only a failed judgment or a missed review demotes demonstrated understanding. Thinking out loud in your log never revokes it on its own, so you stay free to think candidly. Evidence ages: solid understanding cools over time and draws fresh checks rather than vouching forever. The model is kept across sessions (section 8).

It is used, not just kept:

- Candidates aim at what you most need to check: an open misconception, or a concept the part rests on that has no evidence yet.
- When a part rests on a concept you skipped or handed off, Pathfinder says so and the gap goes on the map.

## 11. What each model sees, and what reaches you

No model answers your questions in its own words. A question gets the places in your sources (or online) that address it, each with a short line quoted from there and checked by code against the source; you read and conclude. Asking to be told (`/tell`, or in plain words, section 15) is the one way to get a model's answer, and it is recorded as told, never as understood (section 10).

Some things must not reach you yet: whether a candidate is right and why, and an answer you are meant to produce yourself. These are kept from you in two ways:

- **Separation.** Each model is given only what its job needs. The model that looks for sources for your questions never sees a candidate's key. Models that hold a secret get no unnecessary tools, their reasoning is never shown to you, and a judge holds its reply until it confirms you gave a verdict.
- **A check on every reply.** Separation stops a model from leaking a stored secret, not from re-deriving an answer when you quote or paraphrase a candidate back to it. While a secret is active, a separate check inspects each draft before it reaches you, together with the turns since the secret opened, to catch multi-turn narrowing. A quoted line can settle a candidate as well as a model's own words can. If the reply reveals, hints at, or confirms the secret, a fixed reply is sent instead.

Separation stops what a model never saw from leaking. The check catches what slips through or gets re-derived anyway.

## 12. Agent work and its provenance

Each change the agent makes is tied to the concepts it relies on, your rationale for the part (your summary and review objections), and the state of your understanding when it is made:

- **Reviewable**: every concept it relies on has evidence, so your review of it counts (section 7).
- **Claimed**: at least one concept rests only on your claim ("already known"), with none unknown, shaky or delegated. Kept distinct: never counted as vouched, and never conflated with handing off without understanding.
- **Delegated**: relies on an unknown, shaky, delegated or untagged concept, or was handed off without understanding (section 6), however carefully you look at it.

The labels are kept with the work, per file and per change (including deletions). Together they give **understanding coverage**: how much of the work you can vouch for, on what evidence, and why it was made. It is meant for you and for anyone who relies on the work, such as a supervisor or a reviewer, which is why the agent doing the work can't write it.

## 13. Checked review

Review can be as shallow as reading a diff and approving it. Pathfinder checks it the way a candidate checks a summary: now and then, the result you review contains a planted conceptual flaw in something the change relies on. If you approve it without catching the flaw, the review didn't happen and is not recorded as one. Objecting to a real bug in another part of the change goes back to the agent as usual and is never counted as a missed check.

Handoff runs in an isolated staging copy, and live diffs and prose summaries of the real edit are held back until your verdict, so neither the workspace, test output nor the agent's report gives away whether a review is checked. The planted flaw is revealed when you give your verdict, and only the agent's real result is written to disk.

## 14. Limits

- **The record is a map, not a grade.** The evidence behind every entry is visible, so you can see why the record says what it says and argue with it.
- **Understanding is not verification.** Vouching for a change means you understood the concepts behind it and reviewed it, not that it is free of runtime, integration or concurrency bugs. Automated tests and formal checks stay separate, and should be shown beside understanding coverage rather than conflated with it.
- **Tying work to concepts can be wrong.** You can see and correct it, and when it is unsure, or when later formatting breaks a hash, coverage is counted as lower, never higher.
- **Local integrity is not cryptographic proof.** The hash chain and tool boundaries keep the agent from altering the record (as far as command permissions and routes enforce them). Proving the record to an untrusting third party requires external signing.
- **Separation has a boundary.** Without the reply check (or on external CLI routes that keep their own transcripts), Pathfinder can mask history, but cannot stop a quoted line or a told answer from settling a candidate you paraphrase back to it.
- **The sandbox is Pathfinder's own.** It hides Pathfinder's files from its own agent's commands (A2). Claude Code and Codex on the CLI routes run in their own sandboxes, which can read them.
- **Checks have a cost.** How often reviews are checked (section 13) is a visible setting of the session. A check you can't see is not allowed.

## 15. You work in plain language

Everything in sections 1–14 can be done by saying it. "Where does the gate decide what to hold back?", "show me that file", "I think I get this part now", "just tell me", "let the agent do the tests": decision models read what you mean, and the harness acts on it. Commands are shortcuts for people who want them, never the only way in.

Two kinds of message are treated differently:

- **Intent**: asking, looking something up, opening a source, exploring. Pathfinder acts on it directly.
- **A commitment**: something that changes what the record says about you. Claiming you know a part, asking to be told (section 11), handing a part off or delegating it (sections 6 and 7), giving a verdict on a candidate or on the agent's result.

A commitment read from plain language is still a model's proposal (section 9). Before it counts, it is shown back to you in one line, saying what will be recorded, and it can be undone in one step. Undoing never lowers the bar: a claim can be taken back right after you make it, but a verdict that has been judged, or an answer you have seen, is evidence about you and stays. So a commitment that starts work or settles a check (handing off, delegating, a verdict, approving or objecting to the agent's result) waits for your OK instead of being undone afterwards. Pathfinder never records a commitment you didn't see, so the record stays something you said, not something a model guessed about you. When a decision model is unsure whether you meant a commitment, it asks instead of guessing.

## 16. The path: an exact next step, a loose horizon

People come to understanding by different routes: purpose first or mechanism first, an example before the principle or the reverse, reading or trying. Most don't know which route is theirs, and it changes from task to task.

So Pathfinder doesn't lay out the route. It shows two things:

- **The next step, exactly.** Where to look (the exact source and lines; finding them is Pathfinder's job, not yours) and an open question to take there.
- **The horizon, loosely.** The map's parts and what each depends on (section 3): where you're heading, not the order of the steps that will get you there.

The route between them stays loose for three reasons:

- **It depends on you.** Each next step is chosen from what you just did: your log, your summaries, and the understanding model's state (section 10). A concept you have never touched gets an orienting step; a shaky one gets a narrower one.
- **A precise route gives part of the answer away.** The order of insights that leads to understanding is much of the understanding. Laying it all out in advance does that reasoning for you.
- **You can steer.** "Give me an example first" or "start from what it's for" changes the next step. Over a task, Pathfinder learns which kinds of step work for you and leans on them, and the record of how you got there stays yours to read.

Being exact about where and open about what to conclude is the same rule as section 11, applied to the path: Pathfinder finds your sources but leaves the conclusions to you.

---

# Part II. Architecture

*Part II draws how Part I is built. Each diagram names the sections it serves; an element that can't is removed. When the two disagree, Part I wins.*

Legend for all diagrams: **solid** arrows carry ordinary data, **thick** arrows are writes only the harness makes (§9), **dotted** arrows carry a secret (§11). Decision models return probabilities and never text, so they can be shown a secret safely.

## A1. The whole system

*§9: models propose, the harness records. §11: who can see a secret.*

```mermaid
flowchart TB
    U([You, in the terminal])

    subgraph H[Harness: the only writer of the stores]
        direction LR
        R[Router] --> O[Orchestrator]
        O --> G[Gate]
        O ==>|events| LM[(Understanding model<br/>concepts · events · misconceptions)]
        O ==>|entries| LG[(Ledger, hash-chained)]
        O ==>|create, retire| SR[(Secret registry)]
        LM -->|states, gaps, targets| O
    end

    subgraph LMS[Language models: text out]
        direction LR
        TU[Tutor<br/>also proposes concepts, beliefs]
        MP[Mapper]
        EX[Examiner]
        JU[Judge]
        AG[Agent]
        PL[Planter]
    end

    subgraph DMS[Decision models: probabilities out, never text]
        direction LR
        DR{{route}}
        DX{{match concepts, confirm belief}}
        DG{{leak probe}}
        DT{{tag hunk}}
        DP{{plant check}}
    end

    U -->|message, while working on a part| R
    U -->|commands, answers to a question, onboarding| O
    G -->|Tutor, /tell and /check replies,<br/>while a secret is active| U
    O -->|candidate, verdict feedback,<br/>review, agent stream| U
    O -->|each role gets only its view · A2| LMS
    LMS -->|proposals: reply, map, verdict<br/>edits: staged on a handoff in git, live otherwise| O
    TU -->|draft| G
    EX -.->|key| SR
    PL -.->|planted hunk| SR
    SR -.->|key| JU
    SR -.->|probes| DG
    R <--> DR
    G <--> DG
    O <-->|log entries| DX
    O <-->|agent hunks| DT
    PL -.->|planted hunk| DP
```

Without a decision service, or with decisions off, the Gate isn't asked: a Tutor reply goes out with the candidate masked from its history, and `/tell` and `/check` are held (A6, §14).

## A2. What each role may see

*§11: least view. The type system enforces it: a secret is a `Secret<T>` with no `Display` or `Serialize`, and only the views marked ● can turn it into prompt text.*

| Role | Kind | Tools | Map, log | Concept states | Candidate text | Key ● | Planted hunk ● | Returns |
|---|---|---|:-:|:-:|:-:|:-:|:-:|---|
| Router | decision | none | goal, part, log count, recent turns | | ✓ | | | step, part, steer |
| Tutor | language | read-only | ✓ | ✓ | masked | | | cited lines (JSON), map change |
| Extractor | decision+language | none; its language half is the Tutor's, read-only | ✓ | ✓ | | | | concepts, misconceptions |
| Mapper | language | read-only | ✓ | ids, labels | | | | parts, concepts, prerequisites |
| Examiner | language | read-only | map outline, part's log | targets | writes it | writes it | | candidate, key |
| Judge | language | none | part title | | ✓ | ✓ | ✓ | pass, misconception, feedback |
| Gate | decision | none | recent turns | | | probe | probe | p per secret |
| Agent | language | full (staging on handoff, in a git repo) | part only | | | | | changes, report |
| Tagger | decision | none | part only | | | | | concepts, p |
| Planter | language | none | part title, concept labels, the file and its hunk | | | | writes it | flawed hunk |

The Agent never sees concept states or the ledger, so it can't shape its work to look reviewable (§12). On the own-agent routes this holds on disk too: its file tools refuse, and its sandboxed commands can neither read, write nor move out of hiding, anything private to Pathfinder (the database with the ledger and secrets, its logs and caches, the config with credentials, other reviews' staging copies), apart from its own staging copy, and on Linux they can't see or signal Pathfinder's process. Where that sandbox can't be applied in full, there is none and every command is asked first. Two exceptions: a command you approve to run outside the sandbox runs without that hiding, and `pathfinder coverage --export` writes a report built from the ledger to `.pathfinder/coverage.json`, which the agent can read but not write. Judge runs with no workspace tools, so untrusted files can't inject into a secret-bearing prompt, and buffers its reply until it confirms the message is a verdict. Thinking from Examiner, Judge and Planter is dropped at the sink by role (§11). Strict secret isolation holds on API routes; external CLI routes keep their own transcripts and sandboxes (§14).

## A3. A turn

*§4–§7, §15: every message becomes one step; a commitment read from plain words is shown back: a claim can be undone in the next step, a tell cancelled until its answer shows, and a verdict, handoff or delegation waits for your OK (a typed `/handoff` or `/delegate` is already explicit and skips this; a verdict has no typed command); each write commits as it happens and the session is saved at the end, so a failed or cancelled turn keeps what it has already committed.*

```mermaid
flowchart TD
    M[message] --> R{Router<br/>p ≥ 0.6?}
    R -->|no| F[Tutor decides the step itself]
    R -->|"commitment, unsure"| Q[ask which you meant · §15] --> OUT
    R -->|"log, or no sure step on a bare claim,<br/>but the words commit you<br/>(no candidate waiting)"| SL[second look by code] --> Q
    R -->|log| T[Tutor]
    T -->|concepts, belief| X[Extractor]
    R -->|summary| S[choose targets · A5] --> E[Examiner] --> K[register key · A6]
    R -->|"known · tell"| SB[shown back in one line · §15<br/>claim: undo next step · tell: Esc until shown]
    R -->|"verdict · approve · object (result in review) · agent on the part named: handoff if reviewable §12, else delegate"| OK[shown back · waits for Enter · §15<br/>Esc cancels · a verdict stays a note]
    R -->|"log · move · known · agent < 0.9, or unsure, while a candidate waits"| OK
    OK -->|verdict| J[Judge, no tools] --> RT[write pass/fail · retire key]
    SB -->|known| C[claim events] --> GP[walk prerequisites · gaps]
    SB -->|tell| TL[Tutor answers, held · §11] --> SG{strict gate · A6}
    SG -->|cleared, or no secret| TE[told events] --> OUT
    SG -->|"not cleared, or masked mode"| FX
    R -->|move · replan · accept · reject| MV[map and focus]
    OK -->|handoff, in a git repo| AH[Agent in staging] --> TH[Tagger] --> LH[ledger] --> RV[review · A8]
    OK -->|handoff, outside git| AP[Agent in place] --> RV
    OK -->|delegate| AD[Agent in place] --> LD[ledger, in a git repo: all delegated, untagged]
    T --> V[code checks each quote against its source · renders the reply] --> AS{active secret?}
    AS -->|no| OUT
    AS -->|yes: buffer| GT{Gate · A6}
    GT -->|pass| OUT
    GT -->|block| FX[fixed reply] --> OUT
    X --> TX
    K --> OUT
    RT --> OUT
    GP --> OUT
    MV --> OUT
    RV --> OUT
    LD --> OUT
    F --> AS
    OUT[emit to the thread] --> TX[(session saved<br/>events, secrets, ledger<br/>committed as written)]
```

A verdict read from plain words, even at low confidence while a candidate waits (any `log`, `move`, `known` or `agent` below 0.9, or no sure step at all, is shown back as a possible verdict), goes through your OK rather than straight to the Judge, because a judged verdict can't be undone (§15 wins over speed); Esc keeps the message as a note, so a question about the candidate is still answered. A quick check's verdict is shown back the same way: it is a verdict on a candidate like any other. Exam answers still go straight to the Judge: you asked to be asked, and an answer to a question is not a verdict on a candidate. Undo appends a `retracted` event that only a claim can be the target of, so no undo can drop a pass, a fail or a review.

## A4. Data model

*§8, §10, §12. `events` is append-only; every state shown to you is computed from it.*

```mermaid
erDiagram
    CONCEPT ||--o{ PREREQUISITE : needs
    CONCEPT |o--o{ EVENT : "evidence for"
    CONCEPT ||--o{ MISCONCEPTION : "wrong belief about"
    SESSION ||--o{ PART_CONCEPT : "part rests on"
    CONCEPT ||--o{ PART_CONCEPT : ""
    SESSION ||--o{ CANDIDATE : asks
    CANDIDATE |o--|| SECRET : "key is"
    SESSION ||--o{ SECRET : holds
    SESSION ||--o{ LEDGER : records
    LEDGER }o--o{ CONCEPT : "relies on"
    LEDGER }o--o{ EVENT : "label rests on"

    CONCEPT {
        text id PK "slug, unique per workspace"
        text workspace "git root"
        text label
        json aliases
        text created_by "map | extractor | user"
    }
    PREREQUISITE {
        text concept PK
        text needs PK
        text source
    }
    PART_CONCEPT {
        text part "in the session's state, not a table"
        text concept "1-4 core per part"
    }
    EVENT {
        int id PK
        text workspace "git root"
        text session_id
        text concept "soft key: a concept id, or sources"
        text kind "claim touched misconception_shown pass fail misconception_resolved delegated review_ok review_missed told read searched retracted step"
        text ref "candidate, misconception, secret, retracted event, part, path or query"
        json detail
        int at "unix seconds"
    }
    MISCONCEPTION {
        text id PK
        text workspace
        text concept FK
        text statement
        text source "log | judge"
    }
    CANDIDATE {
        text id PK
        text part
        json targets "1-2 concepts"
        text misconception FK "if built to embody one"
        int flawed
        text secret FK
        int passed
    }
    SECRET {
        text id PK
        text kind "candidate_key | exam_key | planted_flaw | review"
        text payload "not sent to you before your verdict"
        text probe "the gate's question"
        text status "active | retired"
    }
    LEDGER {
        text id PK
        text kind "change | review"
        text refers_to "review: the change entry it is about"
        text path
        text hunk "old,new line ranges, incl. deletions"
        json line_hashes "path-scoped sha256 per 3-line trimmed window"
        json concepts
        real tag_confidence
        text label "reviewable | claimed | delegated"
        json rationale "change: summary, instructions, objections · review: objection"
        json evidence "event ids at that moment"
        text review "review entries: approved objected checked_caught checked_missed"
        text hash "sha256(prev_hash + entry)"
    }
```

The workspace is a key (the git root) that concepts, prerequisites, events and the ledger carry, not a table. A part's core concepts are a list on the part in the session's state, checked to hold 1–4. A misconception's open or resolved status is folded from its events, not stored. Source events (`read`, `searched`) go under the pseudo-concept `sources`, which is never a concept row. `outcomes` (§8) and `critic_events` tables also exist and are not drawn.

## A5. The understanding model's rules

*§10: only evidence counts. A concept's state, from its events:*

```mermaid
stateDiagram-v2
    [*] --> unknown
    unknown --> claimed: claim
    unknown --> delegated: delegated
    unknown --> shown: pass
    unknown --> shaky: fail
    claimed --> shown: pass
    claimed --> shaky: fail
    delegated --> shown: pass
    delegated --> shaky: fail
    shaky --> shown: pass and no open misconception
    shown --> solid: 2nd pass, one flawed, no open misconception
    shown --> shaky: fail or review_missed
    solid --> shown: aged, no pass or review_ok in 30 days
    shown --> solid: aged solid, then any pass or review_ok (even with an open misconception)
    solid --> shaky: fail or review_missed
    note right of claimed: a claim never becomes evidence
    note right of shaky: a misconception closes only by passing a candidate built to embody it
    note right of solid: a log misconception aims T1 and blocks solid, but never demotes without a verdict
```

Ageing makes a state depend on time as well as events: states are computed as of a `now`, and a ledger label stores the `now` it used, so it can be recomputed. `/why` stores nothing: it shows the states as of the moment you ask.

*A part's status, from its 1–4 core concepts:*

```mermaid
flowchart LR
    P1{every core concept<br/>shown or solid?} -->|yes| UND[understood §5]
    P1 -->|no| P2{every core concept<br/>claimed or better?}
    P2 -->|yes| KN[known §6]
    P2 -->|no| P3{handed off?}
    P3 -->|yes| DEL[delegated §6]
    P3 -->|no| OP[open]
```

*What the next candidate checks, first match wins:*

```mermaid
flowchart LR
    T1[a concept with an open misconception<br/>→ flawed candidate that embodies it] --> T2[a concept with no evidence:<br/>unknown, claimed or delegated] --> T3[a shaky concept] --> T4[the concept with<br/>the fewest passes or oldest evidence]
```

*Gaps, checked when a part is entered (after a claim or a delegation, only when there is a next part to enter; right after an agent run, only gaps on that run's concepts, the rest wait for the next part entered):*

```mermaid
flowchart LR
    G1[walk the prerequisites<br/>of the part's core concepts] --> G2{any claimed, delegated,<br/>unknown or shaky?}
    G2 -->|yes| G3[say so in the part's introduction<br/>and propose a small part for up to two]
    G2 -->|no| G4[nothing to add]
```

All three are computed by the harness, never by a model. Gaps are raised once per map, a concept another open part covers is skipped, no gap part is proposed while a map change waits, and a quick check skips gaps.

Concept matching: lexical and semantic candidates (slug, label, aliases and local embedding similarity, up to 20), then a decision choice "same as which, or new?"; same at p ≥ 0.7. Misconceptions from your log open only when a decision predicate "does this message state that belief?" reaches p ≥ 0.8; they aim the next candidate (T1) and block promotion to `solid`, but do not demote `shown` or `solid` until a verdict fails.

## A6. Secrets and the gate

*§11: separation first, then a check on every reply while a secret is active.*

```mermaid
stateDiagram-v2
    [*] --> active: Examiner writes a key (candidate or exam), Planter a flaw, or a handoff review opens
    active --> active: every reply buffered and checked against its probe
    active --> retired: nothing waits on it: a verdict, an objection, a skip, move, delegation, quick-check exit or onboarding
    retired --> [*]
    note right of active: Tutor never streams; code renders checked quotes, and the gate checks them while a secret is active
    note right of retired: Judge feedback may reveal it now
```

```mermaid
sequenceDiagram
    participant T as Tutor
    participant O as Orchestrator
    participant D as Decision model
    participant U as You
    T->>O: buffered draft reply
    O->>O: deterministic pre-check (six words in a row from the key, or a long line of it)
    O->>D: one decision call: recent turns since secret opened + draft + a predicate per active secret
    D-->>O: p per secret
    opt some p in [0.15, 0.5)
        O->>D: key-blind reader: candidate + turns since it opened + draft
        D-->>O: P(right), P(flawed), P(neither)
        O->>O: bits toward the true key = log₂(q / 0.5), q = P(true) + ½·P(neither)
    end
    alt pre-check clear, all p < 0.5, and every p ≥ 0.15 moves the reader less than its stakes allow
        O->>U: reply
    else pre-check hit, any p ≥ 0.5, or over the stakes' bits (candidate 0.2 = 57.4% guessing odds, review check 0.14 = 55%)
        O->>U: fixed reply: "That's what you're judging. Verdict first."
    end
    Note over O,D: a check or reader that errors withholds the reply (fail closed).<br/>The reader judges mostly the last reply, so bits don't add up across a conversation reliably; the predicate still counts hints spread across turns.<br/>/tell and /check use a strict gate: shown only when a decision service clears them, else the fixed reply.<br/>Judge buffers feedback until verdict is confirmed.<br/>No decision service: masked-history mode, shown in the toolbar (cannot stop paraphrase re-derivation).
```

## A7. Provenance and understanding coverage

*§12: every agent change is labelled by your evidence when it's made; coverage is computed from the ledger, never written by the agent.*

```mermaid
sequenceDiagram
    participant A as Agent
    participant O as Orchestrator
    participant W as Your files
    participant T as Tagger (decision)
    participant LM as Understanding model
    participant L as Ledger
    O->>O: snapshot the workspace before the run (uncommitted work included)
    alt /handoff in a git repo
        A->>O: edits in the staging copy, held until your verdict (A8)
    else /delegate in a git repo
        A->>W: edits your files as it goes
    end
    O->>O: diff against the snapshot, split into hunks (incl. deletions)
    opt not /delegate, decisions on, the part has concepts, first 40 hunks
        O->>T: hunk + a predicate per part concept + "something else"
        T-->>O: p per concept
    end
    Note over O: tag at p ≥ 0.6, untagged if none<br/>or "something else" ≥ 0.5
    O->>LM: states of tagged concepts, now
    LM-->>O: states + event ids
    Note over O: reviewable if all shown or solid,<br/>claimed if remaining are only claimed,<br/>else delegated (untagged and /delegate count as delegated)
    O->>L: change entry: hunk, path-scoped line hashes, concepts, rationale, label, evidence + its now, hash chain
    Note over O,L: the verdict and objections come later as a review entry pointing at it (append-only).<br/>The agent writes first and the record follows. If the snapshot or diff fails, nothing is recorded.<br/>Outside a git repo no snapshot is taken, so nothing is recorded (A3).
```

```mermaid
flowchart LR
    C[each file the ledger has entries for,<br/>read from the working tree] --> H{line path + 3-line window hash<br/>in a change entry?}
    H -->|no| NP["not from Pathfinder<br/>(pair with git-ai notes when present)"]
    H -->|yes, newest entry| LB{label + review}
    LB -->|reviewable, approved or caught,<br/>chain verifies| V[vouched]
    LB -->|reviewable, not reviewed| RU[reviewable, unreviewed]
    LB -->|claimed| CL[claimed, unverified]
    LB -->|delegated| DL[delegated]
    DE[entries with no line hashes:<br/>deletion-only, or every line left out] --> DC[counted, not bucketed]
    V & RU & CL & DL & NP & DC --> R["per file and total<br/>/coverage · pathfinder coverage --export<br/>.pathfinder/coverage.json + chain head"]
```

Each non-trivial added line is hashed with its file path and its nearest non-blank neighbours above and below (a path-scoped 3-line trimmed window), and deletion-only hunks are recorded as ledger entries and counted beside the buckets. Blank lines, trivial syntax-only lines (such as lone braces or delimiters) and lines whose text was already anywhere in the file before the agent ran (the snapshot, uncommitted work included) are left out; later reformatting that breaks a hash fails closed to `not from Pathfinder`, never falsely vouching. When `git-ai` notes (`refs/notes/ai`) are present, coverage pairs with them so external AI edits are not conflated with human code, and test/command results are reported beside understanding coverage without merging the two (§14). The Agent's tools refuse to write under `.pathfinder/` or the session database. This hunk/git pipeline is the provenance backend for code; non-code work (§1) records provenance at the part and artifact level.

## A8. Checked review

*§13: in a git repo, handoff runs in an isolated staging copy until your verdict (outside one the agent works in place and nothing is planted); now and then the review you see contains a planted conceptual flaw, and only the real result is written to disk.*

```mermaid
sequenceDiagram
    participant AG as Agent
    participant O as Orchestrator
    participant P as Planter
    participant D as Decision model
    participant U as You
    participant J as Judge
    participant W as Your files
    AG->>O: handoff finishes in staging copy (live diffs & report held)
    O->>O: check this review? Off 0 · Light 0.15 shown / 0.05 solid · Normal 0.25 / 0.15
    opt checked
        O->>P: one reviewable hunk + its concepts
        P-->>O: plausible flawed version
        O->>O: still parses? JSON, Python and JS are checked, other types pass
        O->>D: does it change the behaviour the part is about?
        D-->>O: p (keep if it parses and p ≥ 0.7, else retry once, else skip the check)
    end
    O->>O: register a secret: the planted hunk, or the review itself if unchecked
    O->>U: review in the thread: hunks as diffs (one planted if checked, no report contradicting it)
    alt /object on a checked review
        U->>J: objection
        alt names the planted flaw
            J-->>O: caught → review_ok on its concepts
            O->>W: write real edits
        else names a real issue on another hunk
            J-->>O: real objection (retire planted secret, not review_missed)
            O->>U: reveal planted vs real
            O->>AG: send objection back to agent in staging
        end
    else /object on an unchecked review
        O->>AG: send objection back to agent in staging (§7)
    else /approve on a checked review
        O->>U: reveal planted vs real
        O->>O: review_missed → concept shaky, not recorded as reviewed
        O->>W: write real edits
    else /approve on an unchecked review
        O->>O: recorded as approved
        O->>W: write real edits
    end
    Note over O,U: Never on /delegate (§6), and only in a git repo. Real edits reach disk only after verdict.<br/>During /handoff the agent's commands run in the staging copy;<br/>its diffs appear only in the review. Live diffs stay a /delegate feature.
```

## A9. Building it from today's code

```mermaid
flowchart LR
    subgraph Now[Today]
        a1[Part.status, outcomes]
        a2[PendingCandidate in state_json]
        a3[candidate masking in handle_log]
        a4[candidate thinking dropped]
        a5[decide_step]
        a6[agent change_file]
    end
    subgraph Next[Becomes]
        b1[computed from events · outcomes as export]
        b2[candidates + secrets rows]
        b3[Tutor view · A2]
        b4[per-role thinking rule · A2]
        b5[Router, unchanged]
        b6[+ hunks + Tagger · A7]
    end
    a1-->b1
    a2-->b2
    a3-->b3
    a4-->b4
    a5-->b5
    a6-->b6
    b1 --> S1["1. events, concepts, A5 rules, gaps<br/>(lexical matching; embeddings later)"]
    S1 --> S2["2. secrets and the gate ✓"]
    S2 --> S3["3. ledger, tagging, coverage<br/>+ git-ai pairing"]
    S3 --> S4["4. staging copy for /handoff<br/>+ checked review ✓"]
    S4 --> S5["5. plain-language commitments shown back<br/>and undoable · §15 ✓"]
    S5 --> S6["6. next step chosen from concept state<br/>and your steering · §16 ✓"]
```

✓ marks a stage fully in today's code.

## A10. How we know it works

Run against recorded transcripts and scripted students before each release:

- **Gate calibration**: hand-labelled safe and leaking drafts (all learning candidates), checked by the gate alone, to calibrate `BITS_LIMIT`. Target: every leaking draft blocked, at most one safe draft in ten. The leak suite calibrates its grader on the same drafts.
- **Leak rate**: replies a key-blind decision model (a second provider when one is configured, else the gate's own) reads as giving the key away, tested against adversarial pressure transcripts (frustrated user, paraphrasing the candidate back to Tutor, multi-turn narrowing). The grader is calibrated on labelled drafts first, and messages taken as verdicts aren't graded. Target: none sent at p ≥ 0.7; replies from 0.45 to 0.7 are listed as borderline for a person to read.
- **Candidate discrimination**: candidates must separate students who later pass from guessing.
- **Check stealth & fairness**: planted flaws parse where a checker exists (JSON, Python, JS), and objections to real bugs are never scored as `review_missed`. That live diffs and agent reports never contradict a plant holds by construction (the review holds them back) and is covered by unit tests, not this eval.
- **Evidence ratio**: of the concepts `/why` draws for a part, those with a `pass` against those resting on a `claim` alone, so it matches the drawing; not a target.
- **Tagging soundness**: hand-tagged sample vs the Tagger, code and non-code work; no unsafe miss (a hunk tagged with some concepts but missing one it needs, so an approval could vouch for an idea you never showed). Untagged hunks are `delegated` and extra tags only undercount, so precision is reported, not a target.
- **Coverage soundness**: no line counted as `vouched` without ledger evidence whose chain verifies, and no `claimed` line counted as `vouched`.
