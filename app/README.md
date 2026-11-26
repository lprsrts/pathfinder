# pathfinder (the app)

The Pathfinder program: the terminal UI, the Zed agent server (ACP), the model routes, the sandbox, the gate and the understanding record.

```sh
cargo build --release   # target/release/pathfinder
cargo test
```

See the [repository README](../README.md) for what Pathfinder is, and [docs/reference.md](../docs/reference.md) for how it works.

- `src/main.rs`: the commands
- `src/runtime.rs`: a turn, from your message to the reply; `src/runtime/`: one module per part of the theory (onboarding, evidence, skips, handoff, record, the gate, concepts, checked review, provenance, evals)
- `src/tui/`: the terminal UI; `src/acp.rs`: the Zed agent server
- `src/agent/`: Pathfinder's own agent loop, tools and sandbox
- `src/llm/`: the model providers; `src/auth/`: keys and Sign in with ChatGPT
- `src/telemetry/`: consent, the usage-event queue, sign-in and sharing
- `src/store.rs`, `src/understanding.rs`, `src/ledger.rs`: the record
- `evals/`: the eval fixtures and the gate analysis script
