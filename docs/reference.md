# Pathfinder reference

How Pathfinder runs, what you can set, and what each part does. New to it? Start with the [guide](guide.md).

## Connect in Zed

Pathfinder also runs as an agent in Zed's agent panel, with the same threads, record and settings as the terminal. Add it to Zed's settings (`zed: open settings`), with the path `which pathfinder` prints:

```json
{
  "agent_servers": {
    "Pathfinder": {
      "command": "/Users/you/.local/bin/pathfinder",
      "args": ["acp"]
    }
  }
}
```

Zed doesn't start Pathfinder from your shell, so it doesn't see variables set in your shell's startup files. If you set `PATHFINDER_HOME` or `XDG_CONFIG_HOME`/`XDG_DATA_HOME` there, add them under `"env": { … }` in that entry too, so Zed and the terminal use the same settings.

## Where files live

`pathfinder paths` shows the folders, the log and the crash log. For each kind of file (settings and secrets, the database, state, the model cache), Pathfinder uses the first of:

1. `PATHFINDER_HOME`, everything in one folder.
2. The XDG variable (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME`), on any system, if it's an absolute path.
3. A folder that already exists, the platform's place first, then the XDG default (`pathfinder`, or `harness` from before 0.3), so updating never loses settings.
4. The platform's own place: `~/Library/Application Support/com.lprsrts.pathfinder` (and `~/Library/Caches/…`) on macOS, `%APPDATA%\Pathfinder` on Windows, and `~/.config/pathfinder`, `~/.local/share/pathfinder` and so on elsewhere.

Without `HOME`, the account's home folder comes from the system.

## Models and routes

Pathfinder uses a fast model (System 1: onboarding, log replies) and a stronger one (System 2: map, candidates, judgment, agent work). Each model runs on a **route**:

| Route | What runs it | Credentials |
|---|---|---|
| `chatgpt` | Pathfinder's own agent, OpenAI Responses API | Sign in with ChatGPT (your Plus/Pro plan) |
| `anthropic` | Pathfinder's own agent, Anthropic Messages API | API key (see below) |
| `openai` | Pathfinder's own agent, OpenAI Responses API | API key (see below) |
| `claude` | Claude Code CLI (`claude -p`) | the CLI's own login |
| `codex` | Codex CLI (`codex exec`) | the CLI's own login |
| `endpoint` | Pathfinder's own agent, OpenAI-compatible Chat Completions: a self-hosted vLLM, Ollama, llama.cpp or LM Studio server, or a hosted open-model service | optional key (see [Self-hosted and open models](#self-hosted-and-open-models)) |

**Pathfinder's own agent is the default.** With no `system1`/`system2` set, new threads pick from your own-agent routes first, in this order: `chatgpt` (your plan, no per-token cost), `endpoint` (your own servers), `anthropic`, `openai`. The `claude` and `codex` routes are opt-in: Pathfinder then drives another agent, with its own prompts and loop on top of Pathfinder's, so behaviour is less predictable and replies are slower.

On the own-agent routes Pathfinder runs its own tool loop and streams the model's thinking and tool calls into Zed. Conversation turns can read, list, find and search the workspace. Agent work (`/handoff`, `/delegate`) can also edit and write files, shown as diffs and written through Zed so they appear in its Review Changes view (keep or reject each change), and run commands in Zed's terminal, so you watch them. The agent says what it's about to do before each step, and the stop button ends the run. File tools stay inside the workspace and never write to `.git`.

**Commands run in a sandbox** (macOS Seatbelt, or bubblewrap on Linux when `bwrap` is installed). Inside it a command can read files, write only in the workspace (not `.git` or `.pathfinder`) and the temp folders, has no network, can't read credential folders (`~/.ssh`, `~/.aws`, `~/.gnupg`, Keychains, Pathfinder's own config folder wherever it is, `~/.config/gh` and similar), and runs without environment variables whose names look like secrets (`*KEY*`, `*TOKEN*`, `*SECRET*`, `*PASSWORD*`). Sandboxed commands run without asking. When a command needs the internet (installing packages) or must leave the sandbox (`git commit`, writing elsewhere), the agent says so with a reason and you're asked first: Allow, Always allow for that program and kind of access, or Deny. When a sandboxed command fails in a way that looks like the sandbox stopped it, the agent is told how to ask. Without a sandbox (other platforms, `bwrap` missing, or `PATHFINDER_NO_SANDBOX=1`), every command asks. To be asked before every command anyway, set `"ask_every_command": true` in the config. The macOS policy is adapted from OpenAI Codex (Apache-2.0; see NOTICE).

How the loop behaves (it borrows what Codex, Claude Code, opencode, Crush and Gemini CLI do):

- **Context:** the system prompt names the workspace, today's date, the platform and the tool access; agent work also gets the project's `AGENTS.md` or `CLAUDE.md`. Old tool output is cleared once it passes 300 KB (the last three steps stay), and a per-workspace cache key lets the OpenAI routes reuse the cached prompt.
- **Tools:** reads run in parallel; edits and commands in order. `grep` searches PDFs page by page (`slides.pdf p.12: …`); `read` gives numbered lines, at most 2,000 characters each, and says where to continue. `edit` matches typographic quotes, dashes and trailing spaces loosely when there's exactly one such match, keeps Windows line endings, and on a miss shows the closest lines. A missing file gets "did you mean" suggestions.
- **Commands:** output and errors interleaved; long output keeps its start and end (where errors are); output before a timeout is kept.
- **Recovery:** tool calls with broken arguments (often cut off by the output limit) go back to the model as an error instead of ending the run; an empty answer gets one nudge; the same call with the same result a third time is flagged, a fifth time ends tool use; three steps before the limit the model is told to converge. A final answer that fails Pathfinder's format check goes back to the model in the same conversation, up to twice, so it fixes the answer without redoing its reads; only then does the check retry from scratch or move to the main model. Stalled streams (no data for 150 s) and overloaded providers are retried up to five times with backoff.

### Sign in with ChatGPT

```bash
pathfinder auth login chatgpt
```

Opens your browser to sign in with your ChatGPT account; model usage then counts against your ChatGPT plan (Plus users share one five-hour limit across all connected apps; see chatgpt.com/settings/usage). This is OpenAI's official flow for open-source apps: Pathfinder registers itself once, keeps the tokens in its auth file, and refreshes them as needed. In Zed, the **Sign in with ChatGPT** button runs the same command. `pathfinder auth logout chatgpt` signs out and revokes the token (`--forget` also drops the registration, to switch accounts). Set `PATHFINDER_NO_BROWSER=1` to print the sign-in URL instead of opening a browser.

Claude and Gemini subscriptions can't be used this way: their providers don't allow third-party apps to. Use an API key, or the `claude` route for your Claude Code login.

### API keys

```bash
pathfinder auth set anthropic     # or: pathfinder auth set openai
```

The key is checked with the provider, then stored in `auth.json` in the config folder (`pathfinder paths`), a file only you can read (mode 0600), the same way Codex and other CLIs keep credentials. When no model is available, Zed shows an **Add an API key** button that runs the same command in a terminal. Other commands:

- `pathfinder auth status` shows which keys are available and where they come from.
- `pathfinder auth remove <provider>` deletes Pathfinder's stored key.
- `pathfinder auth import <provider>` copies an existing `<provider>-api-key` macOS keychain item (e.g. one your scripts already use) into the auth file. This is the only time Pathfinder reads the keychain.

Keys are looked up in this order: `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`, then the auth file. The file holds secrets in plain text, protected by file permissions; use the environment variables if you'd rather keep keys elsewhere.

### Choosing models

With a key in place, Pathfinder lists the provider's current models in Zed's pickers by itself (cached for a day; `pathfinder models --refresh` updates now). To pin your own list or defaults, use `config.json` in the config folder (`pathfinder paths`). They appear, grouped by route, in the two model pickers of Zed's agent panel; `system1`/`system2` are the defaults for new sessions. The file is re-read on every turn, so edits apply without restarting Zed.

```json
{
  "models": [
    { "id": "haiku", "label": "Haiku 5.5", "route": "anthropic", "model": "claude-haiku-5-5", "effort": "low" },
    { "id": "sonnet", "label": "Sonnet 5.5", "route": "anthropic", "model": "claude-sonnet-5-5", "effort": "medium" },
    { "id": "codex-sol", "label": "GPT-6.1 Sol", "route": "codex", "model": "gpt-6.1-sol", "effort": "high" }
  ],
  "system1": "haiku",
  "system2": "sonnet"
}
```

- Set `"discover": false` to show only the models you list.
- `model` is the provider's model id (or the CLI's model name; empty uses its default). `effort` is the reasoning effort (`low`, `medium`, `high`, ...).
- On the CLI routes, agent work may edit files and run shell commands in the workspace: Codex in its `workspace-write` sandbox, Claude Code with its sandbox enabled.
- Binaries: `claude_bin`, `codex_bin` (or `PATHFINDER_CLAUDE_BIN`, `PATHFINDER_CODEX_BIN`). `PATHFINDER_DB_PATH` moves the session database.

### Self-hosted and open models

Pathfinder can run entirely on models you host, for example a GPU server at an institute, with nothing sent to a model company. Each server is an **endpoint** in `config.json`:

```json
{
  "endpoints": [
    { "id": "lab", "base_url": "http://gpu-box:8000/v1", "models": ["qwen3.8-27b", "gemma-3-27b-it"] }
  ],
  "decisions": { "endpoint": "lab", "model": "gemma-3-27b-it" },
  "system1": "lab:gemma-3-27b-it",
  "system2": "lab:qwen3.8-27b",
  "local_only": true
}
```

- `base_url` goes up to and including `/v1`. Leave `models` out to offer whatever the server lists (`/models`, refreshed with `pathfinder models --refresh`).
- `key` names an auth file entry for servers that need one: `pathfinder auth set <name>`, or `pathfinder auth import <name>` to copy a keychain item. No `key`, no auth header.
- `extra` is merged into every request, for server-specific options. On OpenRouter, `{"provider": {"require_parameters": true}}` keeps requests on providers that honour log probabilities.
- `decisions` runs the decision service (the gate and step routing) on an endpoint model through **token log probabilities**: each question becomes a one-letter answer, and the letters' probabilities are its distribution. The server must return `logprobs` (vLLM, llama.cpp and many hosted services do), and the model should answer without thinking first. A server that returns none makes the check fail, so the gate withholds the reply rather than guessing.
- `local_only` hides every other route and every hosted decision service, and stops model discovery from contacting them: on-prem means nothing leaves.

Images reach endpoint models that see them (`view` and attachments). PDFs don't go to these servers as files; the model reads them with `read` and `view`. Pathfinder and each person's understanding record stay on their own machine; the server only answers requests.

How good the loop is depends on the models. With 27B-class open models it works end to end (answers grounded in slides, candidates with one real flaw, the gate holding against a paraphrased question, verdicts judged right), but they make more factual slips than the large hosted models and sometimes write LaTeX, which Zed shows as source. In testing, an 8B decision model read an explanation of the whole part as an ordinary note, while a 27B one got it right, so don't go too small. Run `pathfinder eval` against the models before relying on them.

## In the terminal

```bash
pathfinder ~/projects/thesis   # or: cd there && pathfinder
```

The same Pathfinder as in Zed, in a full-screen terminal UI: the same threads, record, checks, exams and settings, so a thread started in one can't be told from one started in the other. The conversation streams on the left, with thinking folded into one line (Ctrl+T shows it) and each tool call on one line with its result; the side panel shows what the thread is doing (chat, quick check, exam, task), anything waiting for your answer, and the plan. `/why` and `/map` print their diagrams as indented trees.

Enter sends, Alt+Enter (or Ctrl+J) adds a line, Esc stops a reply. The input edits like a shell: Ctrl+A and Home/End, Ctrl+U and Ctrl+K delete to the start and end of the line, Ctrl+W deletes a word, Ctrl+←/→ move by a word. Ctrl+N starts a thread, Ctrl+O reopens an earlier one in the same folder, Ctrl+P changes the models, decisions and checks, F1 lists the keys. Attach a PDF or image with `/attach <file>`, or drop the file onto the terminal: a path in your message that names an image or PDF is attached. During agent work the TUI asks before each command (once, always for that program, or no). Logs go to `tui.log` (`pathfinder paths` shows where; it keeps one older copy).

### Terminal commands

Typing `/` shows the commands that match, and Tab completes one. `/help` lists them all with the keys.

| Command | What it does |
|---|---|
| `/help`, `/?` | These commands and the keys |
| `/quit`, `/exit`, `/q` | Quit; the conversation stays in your terminal |
| `/new`, `/clear` | Start a new thread |
| `/resume`, `/threads` | Open an earlier thread in this folder |
| `/model`, `/models`, `/settings` | Models, decisions, checks |
| `/status`, `/version` | Version, models, gate, what you share |
| `/copy` | Copy the last reply |
| `/thinking` | Show or hide thinking |
| `/join` | Join the beta, or change what you share |
| `/data` | What you share, and how to change it |
| `/update` | Check for a new version |
| `/attach <file>` | Attach a PDF or image |
| `/share [note]`, `/bug [what happened]` | Send this thread or a bug report, shown in full first |

The task commands (`/why`, `/map`, `/handoff` and the rest) work the same as in Zed; see [Slash commands in Zed](#slash-commands-in-zed). An unknown command says so instead of being sent to the model.

## Just asking

Pathfinder is also an everyday assistant. Questions are answered in full by the main model, in your language, from the workspace and any attached files: concepts, derivations, summaries of documents, solutions when you ask for them, and reviews of your own work ("I did it this way, is it right?"), which go through it step by step. Nothing is mapped and nothing counts as understood: if the question names an idea already on your record, Pathfinder logs that you were told about it (`told`, which never raises its state). Checks come at natural moments, read from what you mean rather than from any phrase: when you say something has clicked, one candidate follows straight away, and asking anything else instead drops it with nothing recorded; when you ask to be tested, Pathfinder asks open questions one at a time, grades each answer and goes on until you stop or change the subject. Answers and verdicts go on the record per idea. Questions also work in a thread whose task is finished.

Attach documents, papers or screenshots: images (PNG, JPEG, GIF, WebP) and PDFs are sent to the model as content it can see, on every route, and a screenshot pasted into the chat is saved and attached the same way. Zed can't render LaTeX in the agent panel, so math is written in plain text and Unicode.

Open the folder you're working in as the workspace and the model finds things itself: `read` returns a PDF's text page by page (`pages: "3-7"`), and `view` shows it up to four pages as images, for circuits, diagrams and plots that text can't carry. Both use poppler (`brew install poppler`); without it, attach the file instead.

Pathfinder's own lines (the candidate prompt, part hints, map-change proposals, test questions) follow the conversation's language, German or English; models write everything else in your language.

## How a session goes

1. Say what you want done. Onboarding settles the goal, then sketches a map of what the task depends on (no size limit).
2. Work on a part by chatting. Your thinking and the reasons behind your steps are the log; the model replies briefly and may propose map changes. Pathfinder reads what each message means, so plain words drive the loop: "I think I get it: …" explains the part, "I already know this" skips it, "let's move on" goes to the next part, "yes, add it" accepts a map change.
3. When you explain the part, you're shown a candidate (an approach or result, right or with one real flaw) and say whether it's right and why. That decides whether the part is understood. While it waits you can still ask questions; see [While you judge](#while-you-judge).
4. You decide what to give the agent: `/handoff` an understood part and review the result, skip a part you already know, or `/delegate` to hand it off without understanding it.

## What understanding is made of

Each part rests on one to four core concepts, which the map names (`Rests on:` under the part). Every concept's state (unknown, claimed, delegated, shaky, shown, solid) comes from an append-only log of events in the session database, kept per workspace (git root), so it carries over to new sessions in the same repo:

- Only a passed candidate counts as evidence. "Already known" records a claim, which never becomes evidence.
- A part is understood once each core concept has evidence, so a part can take more than one candidate. Each candidate is aimed at the concepts with the least evidence.
- A wrong belief stated in your log is recorded only when a decision model is confident it's there (p ≥ 0.8). So is one that a failed verdict shows. The next candidate on that concept is then flawed on purpose in that same way, and passing it closes the misconception.
- Thinking aloud never lowers a state. A failed check does.
- `solid` (two passes, one on a flawed candidate) cools back to `shown` after 30 days without a fresh check.
- When a part rests on a concept no open part covers, or one you claimed, handed off or failed, the part's introduction says so and proposes a small part for it.

`/why` shows what the current part's status rests on.

## While you judge

A candidate's key (right or flawed, and why) is kept in the session database's secrets table, never in the thread's state. Only the model that writes the candidate and the Judge read it, and the reasoning of both is never shown. The Judge's feedback appears only once it has confirmed your message is a verdict.

Until you give your verdict, every reply goes through a gate:

1. The reply and its reasoning are held. The reply model is asked to answer narrowly: exactly what you asked, with no asides tying the answer back to the topic, since those asides are what tend to give a candidate away. If you ask how to check the candidate, it only says to work it through from what's given and compare; naming the rule, method, step or trap to use counts as a hint.
2. A pre-check looks for text copied from the key.
3. A decision model asks whether the reply, read with the conversation since the candidate appeared, reveals, hints at or confirms the key. Sharing the candidate's topic doesn't count: a general explanation that doesn't touch the deciding point passes. Under 0.15 the reply passes and from 0.5 it's blocked. In between, a second reader that never sees the key reads the conversation with the reply and says whether it now tells you the candidate is right, flawed, or neither. The key is one bit (right or flawed); the reply is blocked if it moves that reader toward the true answer by more than the stakes allow (bits = log₂ of the reader's probability for the truth over the even prior of ½). b bits let someone guessing from the hint be right with probability ½ · 2^b, so the limits are set as guessing odds: 0.2 bits (57.5%) for a learning candidate, and 0.14 bits (55%) for a review check, whose verdict vouches for the agent's work or marks it shaky. This replaced a single 0.3 cut-off, which blocked some harmless definitions and missed some hints.
4. A blocked reply is rewritten once without the candidate, keeping only the direct answer to what you asked. If that is blocked too, or the check fails, you get a fixed reply.

When you give your verdict the key is retired and replies stream again.

With Decisions off, the gate can't run. The reply model then only has the candidate hidden from its history, so it can still work the answer out if you paraphrase the candidate to it.


Before an LLM sees a chat message, a decision model can say which step it is (a log entry, an explanation of the part, a skip, a move, a map-change answer, or a verdict on a waiting candidate). Skips and moves then happen without an LLM call, a question about a waiting candidate skips the judge, and log replies get the decision in their prompt. If the decision model is unsure, unset or unreachable, the LLM decides as part of its reply.

| Service | Model | Key |
|---|---|---|
| TypeSafe Jev | `jev-1.14.0` | `pathfinder auth set typesafe` (or `TYPESAFE_API_KEY`); without a key of your own, the shared gate once you've joined the beta |
| OpenAI Decisions API | `gpt-6-luna` | your OpenAI API key (ChatGPT sign-in tokens aren't accepted) |
| Gemini Decision 1 | `gemini-decision-1` | `pathfinder auth set gemini` (or `GEMINI_API_KEY`) |

In a thread, the **Decisions** switch turns it on or off and the **Decision model** picker (shown when more than one service has a key) chooses the service; otherwise the first one with a key is used, in that order. `PATHFINDER_DECISION=openai|gemini|typesafe|off` sets the default for every thread (`off` wins). Each decision shows in the thinking panel with its probability.

## Handoff and checked review

In a git repository, `/handoff` runs the agent in a staging copy of your workspace: tracked and untracked files, with ignored folders such as virtualenvs and `node_modules` linked in.

While the agent works:
- its commands still run in Zed's terminal, in the copy;
- its prose and diffs are held back;
- your files don't change.

When it finishes, you get the review as diffs in the thread.

Now and then one hunk of a review is replaced by a plausible flaw in a concept the change relies on. The **Checks** picker sets how often:

| Checks | Part's concepts `shown` | Part's concepts all `solid` |
|---|---|---|
| Off | never | never |
| Light (default) | 0.15 | 0.05 |
| Normal | 0.25 | 0.15 |

A planted flaw must still parse, where a checker is available (Python, JavaScript, JSON). A decision model must also confirm it changes the behaviour the part is about. Without a decision service, reviews are never checked.

Every review holds a secret, checked or not, so Pathfinder's replies about it are gated the same way in both cases. How your verdict counts:

- **`/approve` on a checked review:** a missed check. The concepts it relied on drop to shaky, and the ledger records `checked_missed`, so nothing is vouched.
- **`/object` that names the planted flaw:** a caught check. It counts as fresh evidence (`review_ok`) and the lines are vouched.
- **Any other objection:** goes back to the agent in the copy as usual. It is never scored as a miss.

Either way, only the agent's real result is written, through Zed so it shows in Review Changes. A file you changed meanwhile is left alone and named. The agent's report follows your verdict. `/delegate` never runs in staging and is never checked.

`PATHFINDER_CHECK_ROLL=0` checks every review that can be checked, which is useful for trying it out.

## Understanding coverage

In a git repository, Pathfinder records every change the agent makes:

- **Snapshots:** it snapshots the workspace before and after each run, through a scratch index so your own index is never touched. This works on every route.
- **Tagging:** each hunk is tagged with the part's core concepts it relies on. One decision request covers each hunk, with a question per concept plus "something else"; a concept is tagged at p ≥ 0.6.
- **Labels,** set by your understanding at that moment:
  - **reviewable:** every tagged concept has evidence;
  - **claimed:** the rest rest only on your claim;
  - **delegated:** anything else. Untagged hunks, hunks with no decision service, and everything from `/delegate` count as delegated.
- **The ledger:** each entry holds the hunk, path-scoped hashes of its new lines, the concepts, your part summary and the evidence with the time it was computed. Entries are hash-chained in the session database and can't be edited or deleted. `/approve` and `/object` add review entries later.

`/coverage`, or `pathfinder coverage [--export] [path]` from a terminal, counts the current lines of the files the agent changed:

- vouched: reviewable and approved;
- reviewable but unreviewed;
- claimed;
- delegated;
- not from Pathfinder.

`--export` writes `.pathfinder/coverage.json` with the chain head. A broken chain vouches for nothing. A line that has been reformatted since falls back to "not from Pathfinder", never higher. The agent can't write under `.pathfinder/`. Coverage measures understanding, not correctness, so run your tests separately.

## Evals

`pathfinder eval [suite ...] [--out report.md]` runs the checks described in [the theory](theory.md#a10-how-we-know-it-works) before a release. With no suite named, it runs all seven. It exits non-zero if a suite with a target misses it.

The five live suites drive the same code paths Zed does. They use your configured models and decision service, in a throwaway database, with the scripted cases in `evals/fixtures.json`. The cases cover code and non-code work (an expense report, a project schedule, a survey write-up):

- **gate:** hand-labelled drafts (safe, leaking, and pointed: check advice that names the rule, method, step or trap the candidate turns on), checked by the gate alone, reader included (no Tutor), twice each to show its noise. Target: every leaking or pointed draft blocked, and at most one safe draft in ten. It needs no main model and runs in seconds, so use it when changing the gate's question.
- **leak:** pressure scripts while a candidate waits: frustration, paraphrasing the deciding fact, and narrowing over several turns. A grader that never sees the key reads the conversation and says whether the replies tell you the candidate is right, flawed, or neither. The leak score is the probability it puts on the key's answer; a grader told the key and asked "does this reveal it?" leans to yes on anything on topic. The grader is first checked on the gate suite's labelled drafts and must flag at least 9 in 10 leaking ones and at most 1 in 10 safe ones, or the run fails. It uses a second decision service when one is configured. Target: no reply at p ≥ 0.7; replies from 0.45 to 0.7 are listed as borderline for you to read. Harmless control questions measure over-blocking, i.e. how often they get the fixed reply.
- **discrimination:** a student who understands the part (a model given notes) and a guesser judge correct and flawed candidates. The report shows the pass rate of each and the gap between them.
- **checks:** plants flaws in sample hunks and checks they parse and carry no hint. The caught-judge then gets four objections: one naming the flaw, one paraphrasing it, one about a real bug elsewhere, and a vague one. Target: no objection about something else is credited as a catch, and no plant has a hint.
- **tagging:** hand-tagged hunks against the Tagger, at several thresholds. Target: no unsafe miss, i.e. no hunk that gets some tags but misses one it needs, since approving it would vouch for an idea you never showed. A hunk with no tags is recorded as delegated, so a miss there, like an extra tag, only undercounts what you understood; recall and precision are reported.

Set `PATHFINDER_EVAL_DUMP=FILE` to also write every gate-suite draft's scores (and every leak-suite reply's, to `FILE.leak.jsonl`) for analysis; `python3 -I evals/analyse_gate.py FILE` tests the gate's signals and rules on them, held out by case.

The two record suites read your real session database without changing it:

- **evidence:** passes against claims for each part.
- **coverage:** every ledger chain verifies, a forged entry is caught, and a claimed hunk with an approved review is never vouched.

## Thread toolbar

Main model, Effort (main model reasoning: default, low, medium, high), Fast model, the Decisions switch and the Decision model picker. Each thread keeps its own settings. Zed draws Decisions as a switch where it supports on/off options, and as an On/Off dropdown otherwise.

## Slash commands in Zed

Shortcuts for the same steps. Agent work runs only through `/handoff` and `/delegate`.

- `/plan`: build the map now from onboarding
- `/summary <text>`: explain the current part; a candidate to judge follows
- `/handoff [notes]`: give an understood part to the agent; you review the result
- `/approve`: accept the agent's result
- `/object <reason>`: send the result back to the agent with your reason
- `/known`: skip the part as already known (recorded as your claim)
- `/delegate [notes]`: hand the part off without understanding it (not reviewed)
- `/accept`, `/reject`: settle a proposed map change
- `/replan [what changed]`: ask for a map change
- `/next`: move to the next unresolved part
- `/focus <id>`: switch to any part; also takes its number in the plan (`/focus 3`) or the start of its id (`/focus fwd`)
- `/map`: show the map as a diagram (parts, their status, what needs what)
- `/why`: in a task: what the current part's status rests on, each concept's state, checks and open misconceptions; otherwise your record for this folder as a diagram: every idea you've touched, shown, claimed or only been told, linked by what builds on what
- `/coverage`: how much of the agent's work in this repository you can vouch for
