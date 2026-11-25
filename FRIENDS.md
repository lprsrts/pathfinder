# Pathfinder for friends

Pathfinder is a study partner that makes sure you understand your material. It doesn't just answer for you. It runs in your terminal, on your own computer and your own ChatGPT account.

## Install (Mac)

```sh
curl -fsSL https://pathfinder.lprsrts.com/install.sh | sh
```

The script installs one program, `harness`, into `~/.local/bin`. It is signed and notarised by Apple. If the script says that folder isn't on your PATH, run the line it prints.

## Start

1. Sign in with your ChatGPT account. This opens your browser:

   ```sh
   harness auth login chatgpt
   ```

2. Go to a folder with your course material (PDFs, notes, exercise sheets) and start it:

   ```sh
   cd ~/Uni/EET
   harness tui
   ```

3. On the first start it asks two things:
   - **Whether to share usage data.** This is optional. Usage events are counts and timings, never your messages. Sharing a thread only ever happens when you type `/share` and confirm.
   - **Your invite code** (I'll send it to you). The code also turns on the shared "gate": the check that stops Pathfinder from giving away the answer while you're judging a candidate solution. It works whether or not you share data.

Then tell it what you're studying ("I have the EET exam on 30 March, start with three-phase systems"), and work with it. F1 shows the keys.

## Your data

| | |
|---|---|
| What would be sent right now | `harness data show` |
| Your settings | `harness data` |
| Stop sending | `harness data off` |
| Erase everything already sent | `harness data delete` |
| Share a thread that went well or badly | type `/share what happened` in it |

The full privacy notice is at https://pathfinder.lprsrts.com/privacy.

## Updates

When a new version is out, Pathfinder says so at the bottom of the screen. Quit, then run:

```sh
harness update
```

## When something breaks

Send me a message, and if you're happy to, `/share` the thread it happened in with a short note.
