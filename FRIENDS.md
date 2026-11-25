# Pathfinder

Pathfinder makes sure you understand the work you do with AI. It answers, checks you, and keeps an honest record of what you understood. It runs in your terminal, on your own computer and your own ChatGPT account.

## Install (Mac)

```sh
curl -fsSL https://pathfinder.lprsrts.com/install.sh | sh
```

This installs one program, `harness`, into `~/.local/bin` (or `$XDG_BIN_HOME`, or wherever `PATHFINDER_BIN_DIR` says). It is signed and notarised by Apple. If the script says that folder isn't on your PATH, run the line it prints for your shell.

Pathfinder keeps its settings where your system expects them; `harness paths` shows where.

## Start

1. Sign in with your ChatGPT account (this opens your browser):

   ```sh
   harness auth login chatgpt
   ```

2. Go to the folder you're working in (a project, a course, a paper) and start Pathfinder:

   ```sh
   cd ~/path/to/project
   harness tui
   ```

3. On the first start it asks **whether to share usage data with me**. This is optional. Usage events are counts and timings, never your messages. A thread is shared only when you type `/share` and confirm.

4. Then press Enter to **sign in with your email**. Your browser opens; enter the email address you gave me, type the code Cloudflare emails you, check the page shows the same code as your terminal, and press *Link this computer*. Signing in also turns on the shared gate, which stops the model from giving away an answer while you're judging a candidate. The gate works whether or not you share data.

Then tell it what you want to do and understand, and work with it. F1 shows the keys.

## Your data

| | |
|---|---|
| What would be sent right now | `harness data show` |
| Your settings | `harness data` |
| Stop sending | `harness data off` |
| Erase everything already sent | `harness data delete` |
| Share a thread that went well or badly | type `/share what happened` in it |

Details: https://pathfinder.lprsrts.com/privacy

## Updates

When a new version is out, Pathfinder says so at the bottom of the screen. Quit, then run:

```sh
harness update
```

## When something breaks

Message me, and if you're happy to, `/share` the thread it happened in with a short note.
