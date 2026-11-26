# Joining the Pathfinder beta

Thanks for trying Pathfinder. It's in a closed beta: it works, it has rough edges, and what you notice is what makes it better.

## What you need

- A Mac (Apple silicon or Intel). Linux works if you build it from source.
- Access to models. Pathfinder's use counts against that account.
- Optional: `brew install poppler`, so Pathfinder can read PDFs page by page.

## Install

```sh
curl -fsSL https://pathfinder.lprsrts.com/install.sh | sh
```

This installs `pathfinder` into `~/.local/bin` (or `$XDG_BIN_HOME`, or `$PATHFINDER_BIN_DIR`). It is signed and notarised by Apple. If the installer says the folder isn't on your PATH, run the line it prints for your shell.

## Start

1. Connect your models:

   ```sh
   pathfinder auth login
   ```

2. Go to the folder you're working in and start Pathfinder:

   ```sh
   cd ~/path/to/your/work
   pathfinder
   ```

3. The first start asks two separate things:
   - **Joining the beta.** Press Enter and sign in with the email address you gave me: your browser opens, Cloudflare emails you a code, and you check that the page shows the same code as your terminal before you press *Link this computer*. Joining turns on the shared gate, the check that keeps a candidate's answer from slipping out while you judge it.
   - **Sharing data**, which is optional: usage events (counts and timings, never your messages), and threads you choose to share with `/share`.

F1 shows the keys. The [guide](guide.md) walks through a session.

## Things to try

1. Ask about something in your folder you only half understand. You get where to read, with a line quoted from there, not an answer. Read it, say when it clicks, and judge the candidate you get.
2. Ask something your folder doesn't cover, or use `/web <terms>`, to search OpenAlex, arXiv and (with a key) Brave. `/find <terms>` searches the folder yourself; `/tell <question>` gets a model's answer, recorded as told.
3. Ask Pathfinder to test you on a topic, and answer a few questions.
4. Give it a small task, work through it yourself, then `/handoff` a part you understand and review what the agent did.
5. Run `/why` and see whether the record matches what you think you know.

## Known issues

- Prebuilt releases are for macOS only.
- In Zed, math shows as LaTeX source; ask for plain text, or use the terminal app.
- Without poppler, PDFs are read only when you attach them.
- The gate needs you to have joined, or a decision key of your own (`pathfinder auth set typesafe`). The shared gate allows a daily number of checks per person; when they're used up, Pathfinder holds back replies about the candidate until you've judged it.
- OpenAlex allows a small number of searches a day without a key; past it, online search notes that OpenAlex wasn't searched. `pathfinder auth set openalex` adds a free key.
- Without poppler, PDFs can't be quoted page by page, so questions about them get fewer sources.
- Open models of 8B parameters or less make the gate and the judging unreliable.

## Your data

| | |
|---|---|
| What would be sent right now | `pathfinder data show` |
| Your settings | `pathfinder data` |
| Stop sending | `pathfinder data off` |
| Erase everything already sent | `pathfinder data delete` |
| Where Pathfinder keeps its files | `pathfinder paths` |

Details: [privacy notice](https://pathfinder.lprsrts.com/privacy/).

## Updates

When a new version is out, Pathfinder says so at the bottom of the screen. Quit, then run `pathfinder update`. What changed is in the [changelog](../CHANGELOG.md).

## When something breaks

Type `/bug what happened` in Pathfinder (or run `pathfinder bug "what happened"`). You see the whole report before it's sent. If Pathfinder crashed, the report includes the crash. You can also email me at alper@saritas.io, or open an [issue](https://github.com/lprsrts/pathfinder/issues).
