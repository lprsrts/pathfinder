<img src="collector/public/icon.svg" width="56" alt="" align="left">

# Pathfinder

<br clear="left">

Pathfinder makes sure you understand the work you do with AI. It answers your questions, checks you at the moments that matter, and keeps an honest record of what you understood, what you only claimed, and what you handed off.

It runs in your terminal (or in Zed), on your own computer, with your own model account.

<img src="collector/public/screenshot.svg" alt="Pathfinder in a terminal, showing a candidate to judge after an explanation of retry jitter">

## How it works

- **Ask anything.** Pathfinder answers from the folder you're working in: code, papers, slides, PDFs. Answers count as *told*, not as understood.
- **Say when something has clicked.** You get one short *candidate*: an explanation that is either right or has exactly one real mistake. You say which and why. Passing is the only thing that counts as understanding.
- **While you judge, nothing gives the answer away.** A gate checks every reply against the candidate's key before you see it, and holds back anything that would settle it for you.
- **Hand work to the agent when you're ready.** `/handoff` gives a part you understand to the agent and shows you the result to review. `/delegate` hands off what you don't need to understand, and the record says so.
- **`/why` shows your record:** every idea you touched, and whether you showed it, claimed it or were only told.

The design is in [docs/theory.md](docs/theory.md).

## Install

On a Mac (Apple silicon or Intel):

```sh
curl -fsSL https://pathfinder.lprsrts.com/install.sh | sh
```

This installs one program, `pathfinder`, into `~/.local/bin` (or `$XDG_BIN_HOME`, or `$PATHFINDER_BIN_DIR`). It is signed and notarised by Apple, and the installer checks it against a signed release manifest. Linux users can build from source (below).

You need a model account: a paid ChatGPT plan (Plus or Pro), or an Anthropic or OpenAI API key. Usage counts against that plan or key. Optional: `poppler` (`brew install poppler`) lets Pathfinder read PDFs page by page.

## First run

```sh
pathfinder auth login chatgpt   # or: pathfinder auth set anthropic
cd ~/path/to/your/work
pathfinder
```

F1 shows the keys. [docs/guide.md](docs/guide.md) walks through a session.

## Data and network

- **Your work goes to your model provider**, under your account: your messages, files and the model's replies. Pathfinder itself keeps everything else on your computer (`pathfinder paths` shows where).
- **Commands run in a sandbox** (Seatbelt on macOS, bubblewrap on Linux) that can't read your credentials or reach the network without asking.
- **The beta.** You can join with your email. Joining turns on a shared gate service, so the gate works without a decision key of your own. Only the gate's checks go through that service. Sharing usage data (counts and timings, never your messages) and sharing chosen threads are separate, optional choices. `pathfinder data show` prints exactly what is queued, and `pathfinder data delete` erases what was sent. The [privacy notice](https://pathfinder.lprsrts.com/privacy/) has the details, and the full list of usage events is in [schema/src/lib.rs](schema/src/lib.rs).
- **Fully on your own servers.** With your own model endpoints and `"local_only": true`, nothing goes to a model company or to the beta service. See [docs/reference.md](docs/reference.md#self-hosted-and-open-models).

## Bugs and feedback

Type `/bug what happened` in Pathfinder, or run `pathfinder bug "what happened"`. You see the whole report before anything is sent. You can also [open an issue](https://github.com/lprsrts/pathfinder/issues). Security problems: see [SECURITY.md](SECURITY.md).

## Build from source

You need a recent stable Rust toolchain.

```sh
cd app
cargo build --release      # the program is target/release/pathfinder
cargo test
```

The repository has three crates:

| Folder | What it is |
|---|---|
| [app/](app/) | The Pathfinder program: the terminal UI, the Zed agent server, models, sandbox, gate and record |
| [schema/](schema/) | The usage events and API types the app and the collector share |
| [collector/](collector/) | The beta service: sign-in, consented usage events, shared threads, bug reports, the shared gate, releases ([README](collector/README.md)) |

[CONTRIBUTING.md](CONTRIBUTING.md) explains how to work on it.

## Documentation

- [docs/guide.md](docs/guide.md): using Pathfinder
- [docs/reference.md](docs/reference.md): models and routes, keys, Zed, settings, the gate, checked review, coverage, evals, commands
- [docs/beta.md](docs/beta.md): joining the beta
- [docs/theory.md](docs/theory.md): why it works the way it does
- [CHANGELOG.md](CHANGELOG.md)

## License

Pathfinder is free software under the [GNU Affero General Public License v3.0 or later](LICENSE). Parts of the macOS sandbox policy are adapted from OpenAI Codex under the Apache License 2.0; see [NOTICE](NOTICE).
