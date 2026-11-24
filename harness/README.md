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

On the API routes Pathfinder runs its own tool loop (read, list, find, search in the workspace) and streams the model's thinking and tool calls into Zed. Agent work (`/handoff`, `/delegate`) currently needs a `claude` or `codex` main model; it comes to the API routes next.

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
2. Work on a part. Every message is a log entry: your thinking and the reasons behind your steps. The model replies briefly and may propose map changes.
3. `/summary` explains the part; you're then shown a candidate (an approach or result, right or with one real flaw) and say whether it's right and why. That decides whether the part is understood.
4. You decide what to give the agent: `/handoff` an understood part and review the result, `/known` to skip a part you already know, or `/delegate` to hand it off without understanding it.

## Slash commands in Zed

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
