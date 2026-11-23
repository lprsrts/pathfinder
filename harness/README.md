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

## Models and CLIs

Pathfinder runs models through `pi`, Claude Code (`claude`) or Codex (`codex`), so each can use the logins and subscriptions of its own CLI. It uses a fast model (System 1: onboarding, log replies) and a stronger one (System 2: map, candidates, judgment, agent work).

List the models in `~/.config/harness/config.json`. They appear, grouped by CLI, in the two model pickers of Zed's agent panel; `system1`/`system2` are the defaults for new sessions. The file is re-read on every turn, so edits apply without restarting Zed.

```json
{
  "models": [
    { "id": "claude-haiku", "label": "Haiku", "backend": "claude", "model": "haiku", "effort": "low" },
    { "id": "codex-sol", "label": "GPT-6.1 Sol", "backend": "codex", "model": "gpt-6.1-sol", "effort": "high" },
    { "id": "pi-luna", "label": "GPT-6 Luna", "backend": "pi", "model": "openai/gpt-6-luna", "effort": "low" }
  ],
  "system1": "claude-haiku",
  "system2": "codex-sol"
}
```

- `backend`: `pi`, `claude` or `codex`. `model` is passed to that CLI (empty uses its default); `effort` maps to pi's `--thinking`, Claude Code's `--effort` and Codex's `model_reasoning_effort`.
- Agent work (`/handoff`, `/delegate`) may edit files and run shell commands in the workspace: Codex in its `workspace-write` sandbox, Claude Code with its sandbox enabled.
- Binaries: `pi_bin`, `claude_bin`, `codex_bin` (or `HARNESS_PI_BIN`, `HARNESS_CLAUDE_BIN`, `HARNESS_CODEX_BIN`). `HARNESS_DB_PATH` moves the session database.

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
- `/focus <id>` — switch to any part
- `/map` — show the map
