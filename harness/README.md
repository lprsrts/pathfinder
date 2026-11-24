# harness

Human-in-the-loop pathfinder over ACP (built for Zed, backed by `pi` + SQLite).

## Build and install

The harness is developed with itself, so Zed runs an installed copy rather than `target/`. Work-in-progress builds never replace the tool you are using; install a build only once it passes:

```bash
cd harness
cargo test --release && cargo install --path . --root ~/.local --locked
```

Installed binary: `~/.local/bin/harness`. Start a new Pathfinder thread in Zed to pick up a new install.

## Connect in Zed

Add to `~/.config/zed/settings.json`:

```json
{
  "agent_servers": {
    "Pathfinder": {
      "command": "/Users/alper/.local/bin/harness",
      "args": ["acp"]
    }
  }
}
```

## Models and routes

Pathfinder uses a fast model (System 1: onboarding, log replies) and a stronger one (System 2: map, candidates, judgment, agent work). Each model runs on a **route**:

| Route | What runs it | Credentials |
|---|---|---|
| `chatgpt` | Pathfinder's own agent, OpenAI Responses API | Sign in with ChatGPT (your Plus/Pro plan) |
| `anthropic` | Pathfinder's own agent, Anthropic Messages API | API key (see below) |
| `openai` | Pathfinder's own agent, OpenAI Responses API | API key (see below) |
| `claude` | Claude Code CLI (`claude -p`) | the CLI's own login |
| `codex` | Codex CLI (`codex exec`) | the CLI's own login |

On the API routes Pathfinder runs its own tool loop and streams the model's thinking and tool calls into Zed. Conversation turns can read, list, find and search the workspace. Agent work (`/handoff`, `/delegate`) can also edit and write files, shown as diffs and written through Zed so they appear in its Review Changes view (keep or reject each change), and run commands: Zed asks you first (Allow, Always allow for that program, or Deny) and runs each one in its terminal, so you watch it. The agent says what it's about to do before each step, and the stop button ends the run. File tools stay inside the workspace and never write to `.git`; commands run as you, unsandboxed, which is why each one asks.

### Sign in with ChatGPT

```bash
harness auth login chatgpt
```

Opens your browser to sign in with your ChatGPT account; model usage then counts against your ChatGPT plan (Plus users share one five-hour limit across all connected apps; see chatgpt.com/settings/usage). This is OpenAI's official flow for open-source apps: Pathfinder registers itself once, keeps the tokens in its auth file, and refreshes them as needed. In Zed, the **Sign in with ChatGPT** button runs the same command. `harness auth logout chatgpt` signs out and revokes the token (`--forget` also drops the registration, to switch accounts). Set `HARNESS_NO_BROWSER=1` to print the sign-in URL instead of opening a browser.

Claude and Gemini subscriptions can't be used this way: their providers don't allow third-party apps to. Use an API key, or the `claude` route for your Claude Code login.

### API keys

```bash
harness auth set anthropic     # or: harness auth set openai
```

The key is checked with the provider, then stored in `~/.config/harness/auth.json`, a file only you can read (mode 0600), the same way Codex and other CLIs keep credentials. When no model is available, Zed shows an **Add an API key** button that runs the same command in a terminal. Other commands:

- `harness auth status` shows which keys are available and where they come from.
- `harness auth remove <provider>` deletes Pathfinder's stored key.
- `harness auth import <provider>` copies an existing `<provider>-api-key` macOS keychain item (e.g. one your scripts already use) into the auth file. This is the only time Pathfinder reads the keychain.

Keys are looked up in this order: `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`, then the auth file. The file holds secrets in plain text, protected by file permissions; use the environment variables if you'd rather keep keys elsewhere.

### Choosing models

With a key in place, Pathfinder lists the provider's current models in Zed's pickers by itself (cached for a day; `harness models --refresh` updates now). To pin your own list or defaults, use `~/.config/harness/config.json`. They appear, grouped by route, in the two model pickers of Zed's agent panel; `system1`/`system2` are the defaults for new sessions. The file is re-read on every turn, so edits apply without restarting Zed.

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
- Binaries: `claude_bin`, `codex_bin` (or `HARNESS_CLAUDE_BIN`, `HARNESS_CODEX_BIN`). `HARNESS_DB_PATH` moves the session database.

## How a session goes

1. Say what you want done. Onboarding settles the goal, then sketches a map of what the task depends on (no size limit).
2. Work on a part by chatting. Your thinking and the reasons behind your steps are the log; the model replies briefly and may propose map changes. Pathfinder reads what each message means, so plain words drive the loop: "I think I get it: …" explains the part, "I already know this" skips it, "let's move on" goes to the next part, "yes, add it" accepts a map change.
3. When you explain the part, you're shown a candidate (an approach or result, right or with one real flaw) and say whether it's right and why. That decides whether the part is understood. While it waits you can still ask questions; the reply model can't see the candidate, so it can't vouch for it.
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

## Decision models

Before an LLM sees a chat message, a decision model can say which step it is (a log entry, an explanation of the part, a skip, a move, a map-change answer, or a verdict on a waiting candidate). Skips and moves then happen without an LLM call, a question about a waiting candidate skips the judge, and log replies get the decision in their prompt. If the decision model is unsure, unset or unreachable, the LLM decides as part of its reply.

| Service | Model | Key |
|---|---|---|
| TypeSafe Jev | `jev-1.14.0` | `harness auth set typesafe` (or `TYPESAFE_API_KEY`) |
| OpenAI Decisions API | `gpt-6-luna` | your OpenAI API key (ChatGPT sign-in tokens aren't accepted) |
| Gemini Decision 1 | `gemini-decision-1` | `harness auth set gemini` (or `GEMINI_API_KEY`) |

In a thread, the **Decisions** switch turns it on or off and the **Decision model** picker (shown when more than one service has a key) chooses the service; otherwise the first one with a key is used, in that order. `HARNESS_DECISION=openai|gemini|typesafe|off` sets the default for every thread (`off` wins). Each decision shows in the thinking panel with its probability.

## Thread toolbar

Main model, Effort (main model reasoning: default, low, medium, high), Fast model, the Decisions switch and the Decision model picker. Each thread keeps its own settings. Zed draws Decisions as a switch where it supports on/off options, and as an On/Off dropdown otherwise.

## Slash commands in Zed

Shortcuts for the same steps, and the only way to run the agent:


- `/plan` — build the map now from onboarding
- `/summary <text>` — explain the current part; a candidate to judge follows
- `/handoff [notes]` — give an understood part to the agent; you review the result
- `/approve` — accept the agent's result
- `/object <reason>` — send the result back to the agent with your reason
- `/known` — skip the part as already known (recorded as your claim)
- `/delegate [notes]` — hand the part off without understanding it (not reviewed)
- `/accept`, `/reject` — settle a proposed map change
- `/replan [what changed]` — ask for a map change
- `/next` — move to the next unresolved part
- `/focus <id>` — switch to any part; also takes its number in the plan (`/focus 3`) or the start of its id (`/focus fwd`)
- `/map` — show the map
- `/why` — what the current part's status rests on: each concept's state, checks and open misconceptions
