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

## Configure System 1 and System 2 models

Optional `~/.config/harness/config.json` (defaults to your `pi` default model with `low` vs `high` thinking):

```json
{
  "pi_bin": "pi",
  "system1_model": "anthropic/claude-haiku-4-5",
  "system1_thinking": "low",
  "system2_model": "anthropic/claude-sonnet-4-6",
  "system2_thinking": "high"
}
```

Or via env vars: `HARNESS_S1_MODEL`, `HARNESS_S2_MODEL`, `HARNESS_S1_THINKING`, `HARNESS_S2_THINKING`, `HARNESS_PI_BIN`, `HARNESS_DB_PATH`.

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
