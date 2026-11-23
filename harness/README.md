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

## Slash commands in Zed

- `/plan` — build the path right away from onboarding
- `/check [answer]` — System 2 verifies you understand the current part
- `/handoff [notes]` — unlocked after `/check`; System 2 finishes the remaining labor
- `/next` — move to the next ready part after passing `/check`
- `/skip [notes]` — skip learning this part and let System 2 handle it (`none` to skip without edits)
- `/replan [what changed]` — update the remaining path mid-session
- `/map` — show the dependency map and current status
- `/concepts` — show tracked concept states (`unknown`, `claimed`, `proven`, `skipped`)
- `/focus <id>` — jump to any ready part
