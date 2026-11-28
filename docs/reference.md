# Pathfinder reference

How Pathfinder runs, what you can set, and what each part does. New to it? Start with the [guide](guide.md).

## Where files live

`pathfinder paths` shows the folders, the log and the crash log. For each kind of file (settings and secrets, the database, state, the model cache), Pathfinder uses the first of:

1. `PATHFINDER_HOME`, everything in one folder.
2. The XDG variable (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME`), on any system, if it's an absolute path.
3. A folder that already exists, the platform's place first, then the XDG default (`pathfinder`, or `harness` from before 0.3), so updating never loses settings.
4. The platform's own place: `~/Library/Application Support/com.lprsrts.pathfinder` (and `~/Library/Caches/…`) on macOS, `%APPDATA%\Pathfinder` on Windows, and `~/.config/pathfinder`, `~/.local/share/pathfinder` and so on elsewhere.

Without `HOME`, the account's home folder comes from the system.

## Models and routes

Pathfinder uses a fast model (System 1: onboarding, log replies) and a stronger one (System 2: map, candidates, judgment, agent work). Each model runs on a **route**:

| Route | What runs it | Credentials |
|---|---|---|
| `chatgpt` | Pathfinder's own agent, OpenAI Responses API | Sign in with ChatGPT (your Plus/Pro plan) |
| `anthropic` | Pathfinder's own agent, Anthropic Messages API | API key (see below) |
| `openai` | Pathfinder's own agent, OpenAI Responses API | API key (see below) |
| `claude` | Claude Code CLI (`claude -p`) | the CLI's own login |
| `codex` | Codex CLI (`codex exec`) | the CLI's own login |
| `endpoint` | Pathfinder's own agent, OpenAI-compatible Chat Completions: a self-hosted vLLM, Ollama, llama.cpp or LM Studio server, or a hosted open-model service | optional key (see [Self-hosted and open models](#self-hosted-and-open-models)) |

**Pathfinder's own agent is the default.** With no `system1`/`system2` set, new threads pick from your own-agent routes first, in this order: `chatgpt` (your plan, no per-token cost), `endpoint` (your own servers), `anthropic`, `openai`. The `claude` and `codex` routes are listed when their CLIs are installed but used only when you pick them, or when no own-agent route has a model: Pathfinder then drives another agent, with its own prompts and loop on top of Pathfinder's, so behaviour is less predictable and replies are slower.

On the own-agent routes Pathfinder runs its own tool loop and streams the model's thinking and tool calls into the thread. Conversation turns can read, list, find and search the workspace. Agent work (`/handoff`, `/delegate`) can also edit and write files, written to the workspace and shown as diffs in the thread, and run commands on your computer (sandboxed where it can be; see below), with their output in the thread. The agent says what it's about to do before each step, and Esc ends the run. File tools stay inside the workspace and never write to `.git`.

**Commands run in a sandbox** (macOS Seatbelt, or bubblewrap on Linux when `bwrap` is installed). Inside it a command can read files, write only in the workspace (not `.git` or `.pathfinder`) and the temp folders, has no network, can't read credential folders (`~/.ssh`, `~/.aws`, `~/.gnupg`, Keychains, `~/.config/gh` and similar), can neither read nor write anything private to Pathfinder (its config, data, state and cache folders under every name and convention they could have, `PATHFINDER_HOME`, the database with its `-wal`, `-shm` and `-journal` files (beside a linked database's target too), `tui.log` and the staging copies next to it wherever `PATHFINDER_DB_PATH` puts it, even inside the workspace), except a staged handoff's own copy, can't rename or remove the folders above any of these (which would move them to a name nothing hides), on Linux runs in its own process namespace (it can't see, read or signal Pathfinder or your other programs), and runs without environment variables whose names look like secrets (`*KEY*`, `*TOKEN*`, `*SECRET*`, `*PASSWORD*`). Sandboxed commands run without asking. When a command needs the internet (installing packages) or must leave the sandbox (`git commit`, writing elsewhere), the agent says so with a reason and you're asked first: Allow, Always allow for that program and kind of access, or Deny. When a sandboxed command fails in a way that looks like the sandbox stopped it, the agent is told how to ask. The agent's file tools refuse the same private files, and `ls`, `glob` and `grep` leave them out of what they list and search, along with links that lead to them or out of the workspace. Without a sandbox (other platforms, `bwrap` missing, or `PATHFINDER_NO_SANDBOX=1`), or where it can't be applied in full (a kernel that refuses unprivileged user namespaces, as Ubuntu 24.04's AppArmor does by default), every command asks: Pathfinder checks once at startup by running the whole policy and never falls back to a weaker one. To be asked before every command anyway, set `"ask_every_command": true` in the config. The macOS policy is adapted from OpenAI Codex (Apache-2.0; see NOTICE).

How the loop behaves (it borrows what Codex, Claude Code, opencode, Crush and Gemini CLI do):

- **Context:** the system prompt names the workspace, today's date, the platform and the tool access; agent work also gets the project's `AGENTS.md` or `CLAUDE.md`. Old tool output is cleared once it passes 300 KB (the last three steps stay), and a per-workspace cache key lets the OpenAI routes reuse the cached prompt.
- **Tools:** reads run in parallel; edits and commands in order. `grep` searches PDFs page by page (`slides.pdf p.12: …`); `read` gives numbered lines, at most 2,000 characters each, and says where to continue. `edit` matches typographic quotes, dashes and trailing spaces loosely when there's exactly one such match, keeps Windows line endings, and on a miss shows the closest lines. A missing file gets "did you mean" suggestions.
- **Commands:** output and errors interleaved; long output keeps its start and end (where errors are); output before a timeout is kept.
- **Recovery:** tool calls with broken arguments (often cut off by the output limit) go back to the model as an error instead of ending the run; an empty answer gets one nudge; the same call with the same result a third time is flagged, a fifth time ends tool use; three steps before the limit the model is told to converge. A final answer that fails Pathfinder's format check goes back to the model in the same conversation, up to twice, so it fixes the answer without redoing its reads; only then does the check retry from scratch or move to the main model. Stalled streams (no data for 150 s) and overloaded providers are retried up to five times with backoff.

### Connecting models

```bash
pathfinder auth login
```

Two steps, as a panel under the prompt (in Pathfinder, `/login` shows the same panel in place of the input, the conversation still in view above). First, how to connect:

- **Sign in with your ChatGPT plan**, with its state (`✓ signed in as you@example.com` or `not signed in`): usage counts against your plan.
- **Use an API key · pay per use**: then pick the provider. A filter line (`>`) sits above the list, so typing narrows it and Backspace edits it; each row is a short name and a word or two of state: Anthropic, OpenAI and the servers you already added, each `✓ stored`, `✓ from $ANTHROPIC_API_KEY` or `not configured` (a server: `no key needed`, `key missing`). Typing matches names only, never the state. The panel shows at most eight rows; under a long list, `(n/N)` says where you are. A key pasted into the filter is dropped rather than shown (pick the provider first, then paste it into the masked field); picking a provider whose key is stored says the new key replaces it.
- **Add your own server · advanced**: any OpenAI-compatible service or server (OpenRouter, Ollama, vLLM, ...): its base URL, a short name, and its key if it needs one. The URL is checked as you enter it (one http(s) address, with a port after a `:`), then the server by listing its models; when it can't be reached you are back at the URL, with why. It is then added to `config.json` as an endpoint (see [Self-hosted and open models](#self-hosted-and-open-models)); its key goes to the auth file.

The selected row has an arrow (`→`); ↑↓ move, Enter picks, Esc goes back a step, and at the first step cancels ("Nothing changed."; nothing is saved until a check passes). Keys and the server's questions are typed into a line that edits like the input (←/→, Home/End, Ctrl+U, Backspace); keys show only as dots. A key is checked before it is kept: an API key by listing that provider's models (it says how many), the gate's key with one small request; the model list is then refreshed. When the key's environment variable is already set, you are offered to use it as it is, save it to the auth file, or paste a different one.

Claude Code and Codex CLI are not listed: they keep their own logins, which Pathfinder never performs or reads. Sign in there (`claude`, then `/login`; `codex login`) and their models show up by themselves; the model picker's Claude Code and Codex CLI headings say so too. `pathfinder auth login claude|codex` prints the same hint.

The gate's key (TypeSafe Jev) is not a model account, so it isn't in `/login`: set, replace or remove it in Settings (Ctrl+P), on the **Gate key** row next to Decision service (`✓ stored`, `✓ from $TYPESAFE_API_KEY` or `not set`). For scripts, `pathfinder auth login typesafe` (or `auth set typesafe`) and `pathfinder auth remove typesafe` still do it.

Each row has a direct form, for scripts too: `pathfinder auth login chatgpt|anthropic|openai|endpoint|typesafe` (`server` still works for `endpoint`). With `--with-api-key` (or `--stdin`), the key is read from stdin: `printenv MY_KEY | pathfinder auth login anthropic --with-api-key`. Without a terminal, `pathfinder auth login` with no account says so and stops instead of waiting for a pick; with a terminal but output redirected, a named account asks for its key in a masked prompt. Adding a server asks a few questions, so it needs a terminal (or an entry in `config.json`). A key pasted where the account name goes is never echoed back.

`/login` inside Pathfinder takes the same steps (ChatGPT's browser sign-in included); it opens by itself the first time no account works. `/login anthropic` goes straight to one account. While a reply is running, `/login`, `/logout` and the gate key wait for it, so a tool's permission question never meets a key you are typing.

### Signing out

```bash
pathfinder auth logout [account]
```

Removes one sign-in or key and nothing else, then refreshes that account's model list. Without an account it lists what `/login` can create and Pathfinder holds (the ChatGPT sign-in, API keys, servers) and asks before removing (Enter removes, Esc goes back); `/logout` does the same inside Pathfinder. The gate's key is removed in Settings or with `pathfinder auth remove typesafe` (`pathfinder auth logout typesafe` works too). A key kept in the auth file is listed even while its environment variable is set and used instead; a key that only lives in an environment variable can't be removed here: unset it in your shell. For a server you choose to remove only its key or the server itself (from `config.json`, with its key); `pathfinder auth logout <server>` removes a server that takes no key.

### Sign in with ChatGPT

```bash
pathfinder auth login chatgpt
```

Opens your browser to sign in with your ChatGPT account (in `pathfinder auth login` and `/login`, pick Sign in with your ChatGPT plan); model usage then counts against your ChatGPT plan (Plus users share one five-hour limit across all connected apps; see chatgpt.com/settings/usage). This is OpenAI's official flow for open-source apps: Pathfinder registers itself once, keeps the tokens in its auth file, and refreshes them as needed. `pathfinder auth logout chatgpt` signs out and revokes the token (`--forget` also drops the registration, to switch accounts). Set `PATHFINDER_NO_BROWSER=1` to print the sign-in URL instead of opening a browser: the terminal prints it whole on a line of its own, and in `/login` the panel shows it and `c` copies it. The sign-in still finishes on a local callback address (127.0.0.1), so over SSH open the URL in a browser on the same machine or forward the callback port (`ssh -L`). Esc stops waiting and goes back, in the terminal too.

Claude and Gemini subscriptions can't be used this way: their providers don't allow third-party apps to. Use an API key, or the `claude` route for your Claude Code login.

### API keys

```bash
pathfinder auth login anthropic   # or: openai; `pathfinder auth set <provider>` does the same
```

The key is checked with the provider, then stored in `auth.json` in the config folder (`pathfinder paths`), a file only you can read (mode 0600), the same way Codex and other CLIs keep credentials. Other commands:

- `pathfinder auth status` lists every account under a heading (plans you already pay for, API keys · pay per use, your servers, the gate's decision service), names in one column and states after them in full: `✓ signed in as you@example.com · 4 models`, `✓ key …1a2b · auth file · 12 models`, `key …9f0e · $ANTHROPIC_API_KEY`, `✓ installed · uses its own login`, `not configured`. A key is only ever shown by its last four characters. The ✓ and the model count come from the last model list: a key that is there but was never checked, or was refused, has no ✓ (`· no models listed`). Nothing is cut short; a server name longer than 20 characters gets a line of its own with its state under it, so the rest stays within 80 columns.
- `pathfinder auth remove <provider>` deletes Pathfinder's stored key.
- `pathfinder auth import <provider>` copies an existing `<provider>-api-key` macOS keychain item (e.g. one your scripts already use) into the auth file. This is the only time Pathfinder reads the keychain.

Keys are looked up in this order: `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`, then the auth file. The file holds secrets in plain text, protected by file permissions; use the environment variables if you'd rather keep keys elsewhere.

### Choosing models

With a key or ChatGPT sign-in in place, Pathfinder lists the provider's current models in the model pickers (Ctrl+P) by itself. The list is cached for a day: when the TUI starts with no list or one older than that, it fetches a new one in the background, without holding up startup, and the pickers show it as soon as it arrives (`pathfinder models --refresh` updates now). If nothing can be listed, the status line says so once.

The Claude Code and Codex CLIs are listed when their programs are installed (`claude_bin`, `codex_bin`, found as a path or on `PATH`), each with its own login: Claude Code as `claude-haiku`, `claude-sonnet` and `claude-opus` (Haiku at low effort, the others high), Codex as `codex-<model>`: Codex runs on the same ChatGPT plan, so rather than repeat that plan's whole list it offers the newest model of each kind (Luna, Sol, ...) the plan lists when you are signed in with ChatGPT here too, else `codex-gpt-6-luna` (low) and `codex-gpt-6.2-sol` (high). Ids that earlier versions saved in threads and defaults keep working: `codex-luna` and `codex-sol` name Codex's Luna and Sol models, and a `codex-<model>` id whose model Codex no longer offers names the newest of its kind.

The pickers and `pathfinder models` group the models by account: how a model runs, and who pays. **ChatGPT sign-in · ChatGPT plan** (Pathfinder's own agent on your plan), **Claude Code · Claude subscription**, **Codex CLI · ChatGPT plan** (the `codex` CLI's agent on the same plan), **Anthropic API · pay per use**, **OpenAI API · pay per use**, then each endpoint (**`<id>` · your server**). The same model family can be in several of them, billed differently. Type to filter by model or account; the current model is marked ✓. The Claude Code and Codex CLI headings add where their own login is: run `claude`, then `/login` there; or `codex login`. Outside the pickers the account shows too: the settings list (Ctrl+P) gives it after each model, setting a model names it, and the footer marks pay-per-use models with `$`.

If the model a thread or `system1`/`system2` asks for isn't listed (yet), the status line says once which model runs instead and why: not listed yet (the refresh is still running), the list refresh failed, or not offered by your accounts. `/status` shows configured and used models for both tiers. A turn keeps the models it started with; once a refresh lists the one you asked for, it is used from the next turn.

To pin your own list or defaults, use `config.json` in the config folder (`pathfinder paths`). Pinned models win over listed ones with the same id or model; `system1`/`system2` are the defaults for new sessions. The file is re-read on every turn, so edits apply without restarting Pathfinder.

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

- Set `"discover": false` to show only the models you list (and your endpoints): no provider lists, no CLI accounts, no background refresh of either.
- `model` is the provider's model id (or the CLI's model name; empty uses its default). `effort` is the reasoning effort (`low`, `medium`, `high`, ...).
- On the CLI routes, agent work may edit files and run shell commands in the workspace: Codex in its `workspace-write` sandbox, Claude Code with its sandbox enabled.
- Binaries: `claude_bin`, `codex_bin` (or `PATHFINDER_CLAUDE_BIN`, `PATHFINDER_CODEX_BIN`). `PATHFINDER_DB_PATH` moves the session database.

### Self-hosted and open models

Pathfinder can run entirely on models you host, for example a GPU server at an institute, with nothing sent to a model company. Each server is an **endpoint** in `config.json`:

```json
{
  "endpoints": [
    { "id": "lab", "base_url": "http://gpu-box:8000/v1", "models": ["qwen3.8-27b", "gemma-3-27b-it"] }
  ],
  "decisions": { "endpoint": "lab", "model": "gemma-3-27b-it" },
  "system1": "lab:gemma-3-27b-it",
  "system2": "lab:qwen3.8-27b",
  "local_only": true
}
```

- `base_url` goes up to and including `/v1`. Leave `models` out to offer whatever the server lists (`/models`, refreshed with `pathfinder models --refresh`).
- `key` names an auth file entry for servers that need one: `pathfinder auth set <name>`, or `pathfinder auth import <name>` to copy a keychain item. No `key`, no auth header.
- `extra` is merged into every request, for server-specific options. On OpenRouter, `{"provider": {"require_parameters": true}}` keeps requests on providers that honour log probabilities.
- `decisions` runs the decision service (the gate and step routing) on an endpoint model through **token log probabilities**: each question becomes a one-letter answer, and the letters' probabilities are its distribution. The server must return `logprobs` (vLLM, llama.cpp and many hosted services do), and the model should answer without thinking first. A server that returns none makes the check fail, so the gate withholds the reply rather than guessing.
- `local_only` hides every other route and every hosted decision service, and stops model discovery from contacting them (the background refresh then only asks your endpoints): on-prem means nothing leaves.

Images reach endpoint models that see them (`view` and attachments). PDFs don't go to these servers as files; the model reads them with `read` and `view`. Pathfinder and each person's understanding record stay on their own machine; the server only answers requests.

How good the loop is depends on the models. With 27B-class open models it works end to end (answers grounded in slides, candidates with one real flaw, the gate holding against a paraphrased question, verdicts judged right), but they make more factual slips than the large hosted models and sometimes write LaTeX, which the terminal shows as source. In testing, an 8B decision model read an explanation of the whole part as an ordinary note, while a 27B one got it right, so don't go too small. Run `pathfinder eval` against the models before relying on them.

## In the terminal

```bash
pathfinder ~/projects/thesis   # or: cd there && pathfinder
```

Pathfinder runs as a full-screen terminal UI. The conversation streams on the left, with thinking folded into one line (Ctrl+T shows it) and each tool call on one line with its result (a file the agent changes is named relative to the workspace, `letter.txt: 25 lines, was 0`; a failed call says why in one line, such as `` `notes/` is outside the workspace``). Reads and searches in a row fold into one line that counts only what worked (`Read 2 files, 1 search`): a lookup that failed is the agent's own probe, which it read and went past, and shows when you expand the line; when none worked the line says `2 lookups didn't work (a wrong path or pattern)`; the side panel shows the goal, anything waiting for your answer (`⧗ Waiting for your OK` with what Enter does, or `↶ You claimed …` while Esc can still take it back), the parts with the numbers of the parts each needs (`▶ 3 The verdict prompt ← 1, 2`) and the next steps. It docks beside the conversation in a window 110 columns or wider once there is something to show; otherwise Ctrl+B opens it over the conversation, and the rule above the input carries the parts count and what waits on you. The count is parts understood (`Understood 2/7` on the rule, `2/7 understood` in the panel): a part you claimed as known or handed off without understanding is resolved but not counted. When every part is resolved the reply says how: `Every part is resolved: 2 understood, 5 handed off without understanding.` Once Enter (or Esc) answers what waits for your OK, it leaves the rule and the panel at once; while any reply runs, the rule never asks for your OK and the panel drops its Enter/Esc keys, since those keys then stop the reply. That rule is the one status line: the folder, the mode and the thread, a running reply's time, and how far you scrolled up. The footer shows the keys on the left and the models and settings on the right; a message there goes after a few seconds. `/why` and `/map` print their diagrams as indented trees.

Enter sends, Alt+Enter (or Ctrl+J) adds a line, Esc stops a reply. With nothing typed and no reply running, Enter runs what waits for your OK and Esc cancels it, or takes back a claim you just made in plain words (see [Plain words](#plain-words)); a menu or the panel over the conversation closes first, and a draft is never sent or cleared by them. The input edits like a shell: Ctrl+A and Home/End, Ctrl+U and Ctrl+K delete to the start and end of the line, Ctrl+W deletes a word, Ctrl+←/→ move by a word. With nothing typed, Ctrl+U and Ctrl+D scroll half a page (at the bottom, Ctrl+D twice in a row quits). Ctrl+N starts a thread, Ctrl+O reopens an earlier one in the same folder: the list names each thread by its goal, else its first message, else the sources it opened, with its mode, how many messages you sent and how long ago it changed, the most recent first and the open one marked `(open)`; with no other thread in the folder it says so instead. A reopened thread shows its tool calls and thinking again (threads from before this was kept show the replies only). Ctrl+P changes the models, decisions and checks. F1 shows how it works, every command by section and the keys; typing filters it, and Esc or F1 close it (q too while nothing is typed). F2 switches to terminal text selection: the side panel, the bars beside code and quotes and the line numbers of sources go, so a drag copies only the text. Attach a PDF or image with `/attach <file>`, or drop the file onto the terminal: a path in your message that names an image or PDF is attached. During agent work the TUI asks before each command (once, always for that program, or no). Logs go to `tui.log` (`pathfinder paths` shows where; it keeps one older copy).

### Terminal commands

Typing `/` lists the commands under the input, by section, with what the thread asks for first; commands that call a model are marked `◆`. Typing narrows the list: names that start with what you typed come first, then a word of a name, then its letters in order (from three letters), and only when no name fits, a word of what a command does. ↑↓ pick, Tab completes, Enter runs a command whose name starts with what you typed or that you picked with ↑↓ (a looser match, or a command that needs an argument, only fills in), Esc closes the list until you type again. `/open`, `/check` and `/attach` list files the same way. A fresh thread shows the loop in the middle of the screen. `/help` lists every command with the keys.

| Command | What it does |
|---|---|
| `/help`, `/?` | These commands and the keys |
| `/quit`, `/exit`, `/q` | Quit; the conversation stays in your terminal |
| `/new`, `/clear` | Start a new thread |
| `/resume`, `/threads` | Open an earlier thread in this folder |
| `/model`, `/models`, `/settings` | Models, decisions, checks |
| `/status`, `/version` | Version, models, accounts, gate, what you share |
| `/login [account]`, `/auth` | Connect a model account or key (the same steps as `pathfinder auth login`) |
| `/logout [account]` | Sign out of an account or remove a key |
| `/copy` | Copy the last reply |
| `/thinking` | Show or hide thinking |
| `/join` | Join the beta, or change what you share |
| `/data` | What you share, and how to change it |
| `/update` | Check for a new version |
| `/attach <file>` | Attach a PDF or image |
| `/share [note]`, `/bug [what happened]` | Send this thread or a bug report, shown in full first |

The task commands (`/why`, `/map`, `/handoff` and the rest) are listed under [Task commands](#task-commands). An unknown command says so instead of being sent to the model.

## Asking

Pathfinder doesn't answer questions in its own words. It points to where the workspace addresses the question and shows a short line from there, word for word: `file:line`, or `file p.N l.M` for a PDF. Every line is checked against the cited file before it is shown; anything the model writes besides that is dropped. Matching ignores case, spaces, hyphens, punctuation and accents, and shows the source's own text. If the workspace has nothing, the question is searched online: the fast model turns it into a few short queries for OpenAlex, arXiv and Brave Search, code takes the lines of each result that hold the search terms, and the fast model picks the ones that bear on the question. Each picked line is checked against the fetched page before it is shown. If nothing is left, the reply says **Not found** and suggests terms to search for.

Read and search yourself with `/find <terms>` and `/open <file> [page or line]`. `/find` with several words puts lines holding them as one phrase first (`/find Bayesian Fusion for GMMs` finds `Proposition B.3 (Bayesian Fusion for GMMs).` first), then lines with every word; short words such as `for` or `of` count only toward the phrase, and when some lines hold every word only those are listed and counted. `/check <your work, file or photo>` quotes the first step of your work that doesn't hold, without saying why; a photo of handwritten work is copied into text first, and the copy is shown. `/tell <question>` is the one way to get an answer in full; it is headed **Told**, and the idea is logged as `told`, which never raises its state. Opening and searching are logged as `read` and `searched`, which don't either; a file or search term that names no idea on your record becomes one, shown as *read*.

Checks come at natural moments, read from what you mean rather than from any phrase: when you say something has clicked, one candidate follows straight away, and asking anything else instead drops it with nothing recorded; when you ask to be tested, Pathfinder asks open questions one at a time, grades each answer and goes on until you stop or change the subject. Questions also work in a thread whose task is finished.

Two settings shape how questions are answered:

- `quote_cap` (default 220): the longest verbatim line shown per hit, in characters.
- `sources_search` (`model` or `code`, default `model`): with `model`, the model reads the workspace and proposes hits, and only those found word for word in the cited file are shown; with `code`, a keyword search runs and no model is asked.

`/web` and questions the workspace can't answer search OpenAlex and arXiv, and Brave Search with a key: `pathfinder auth set brave`, `pathfinder auth import brave` (keychain item `brave-api-key`) or `BRAVE_API_KEY`. OpenAlex has a small free daily allowance; past it, add a key with `pathfinder auth set openalex` (or `OPENALEX_API_KEY`). `local_only` turns this off. Each source is listed once: a paper's title and a line of its abstract from the same address are one entry, the abstract's line, and one paper's addresses are one source (arXiv's `abs/`, `html/` and `pdf/` pages and their versions, its `10.48550/arXiv.` DOI, and a DOI from any resolver).


Attach documents, papers or screenshots: images (PNG, JPEG, GIF, WebP) and PDFs are sent to the model as content it can see, on every route, and a screenshot pasted into the chat is saved and attached the same way. The terminal can't render LaTeX, so every model that writes prose you read (onboarding, the map and its changes, the Tutor, candidates, exam questions, the Judge, `/tell`, the planted check's explanation and the agent's report) is asked for math in plain text and Unicode. LaTeX that comes anyway is shown as Unicode when it is drawn: inline and display math (`\(…\)`, `\[…\]`, `$…$`, `$$…$$`) becomes Greek letters, `𝒩`/`ℝ` for `\mathcal`/`\mathbb`, `×`, `·`, `≤`, `≠`, `≈`, `∑`, `∫`, `a/b` for `\frac{a}{b}` (with parentheses around a side of more than one term, `(a + b)/(2c)`, and around the whole fraction when a term or script follows it, `(1/2)mv²`, `(a/b)²`), `√(x)`, and sub- and superscripts in Unicode where every character has one (`cᵢⱼ`, `10⁻¹⁶`), else `_x` or `_(…)`; a script inside a script stays explicit (`2^(3²)`, `μ_(aᵢ)`), so it never reads as one longer script. Matrices keep their brackets (`(a b; c d)` for `pmatrix`, `| a b; c d |` for `vmatrix`), cases read `{1, x > 0; 0, x ≤ 0`, `\left`/`\right` are dropped and `\text{…}`/`\mathrm{…}` unwrapped. A command or environment it doesn't know is kept as written, so nothing the math says is lost. Code (inline or fenced) and blockquotes, which is how quotes from sources are shown, stay exactly as written. `$` opens math only before a non-space and closes only after one and not before a digit, and only around something that looks like math (a command, a script, a relation, a single letter or `f(x)`), so prices stay prices and shell variables such as `$HOME/$USER` keep their dollars; `\[1\]` around a bare number or word is a Markdown escape and shows as `[1]`. Only the display changes: the text the gate approved and the stored reply are untouched, and the gate's word-for-word pre-check reads the key, the candidate and the draft both as written and as drawn, so a key in Unicode is caught in a draft in LaTeX and the other way round.

Open the folder you're working in as the workspace and the model finds things itself: `read` returns a PDF's text page by page (`pages: "3-7"`), and `view` shows it up to four pages as images, for circuits, diagrams and plots that text can't carry. Both use poppler (`brew install poppler`); without it, attach the file instead.

**Math in PDFs.** Slides made in PowerPoint or Word often set math in the Symbol font, which many PDFs map to private-use codes (on one lecture's Maxwell slide, ρ came out as `U+F072` and ∂ as `U+F0B6`, which a terminal draws as unrelated symbols). When a page has such codes, or `pdffonts` lists a Symbol, dingbat or math font, Pathfinder asks poppler's `pdftohtml -xml` which font drew each piece of text and maps the codes back through Adobe's Symbol table (Greek, operators, arrows, bracket pieces) or a small Wingdings table (bullets, arrows, check marks). Word's Cambria Math puts dots above (ẋ, ẍ), the vector arrow and ∑ in unrelated scripts; those are mapped back too. A page whose text is still unreliable (leftover private-use or replacement characters, pieces of tall brackets or integrals, a big operator like ∑ or ∫ alone on its line, Word math next to letters from those scripts) gets `Math on this page didn't extract cleanly; check it in your PDF viewer: page N` above it in `/open`, the model's `read` labels it and suggests `view`, and only its plain prose lines (letters, Greek included, digits and plain punctuation) can be quoted or found by `/find`. Plain prose also takes `[`, `]` and `§`, so a sentence with a reference (`[18]`) or a section sign can be quoted. A page where a math font's glyphs came out as ordinary letters (LaTeX's CMEX drawing ∑ as `X` or `P` and √ as `p`; a Symbol `m` for μ, so `25 μm` reads `25 mm`) gets the same note. Poppler says which runs of text those letters are in, so a plain prose line on such a page can be quoted or found when every place its words occur on the page lies outside those runs: `Proposition B.3 (Bayesian Fusion for GMMs).` can be, `forP one and the same` (CMEX's ∑ inside a sentence) and `25 mm` next to a wrong `m` cannot. A page where nothing but such letters is left can't be quoted at all.

**Citing math.** A cited quote that runs into math the page can't vouch for is shown by its longest run of plain words that checks out (at least three words, and more than half of the quote's words: a real phrase never vouches for made-up words around it). A quote of the math itself, whether copied from the text (`mab = (Σ−1`) or written as LaTeX or Unicode (`c_{ab} = \frac{…}{…}`, `Σ_ab = (Σ_a⁻¹ + Σ_b⁻¹)⁻¹`), is placed on the cited page (a LaTeX or Unicode quote only there, by its letters and digits, within six lines that include a math line and hold mostly the quote's own letters, with every word of three letters or more in it (other than a function name such as `exp` or `det`) found there; prose is never placed this way, nor is a made-up formula such as `price = cost + margin`) and shown by the clean line that introduces it: the title of the proposition, theorem, lemma, corollary or definition it belongs to, up to 24 lines above (`Proposition A.6 (Bayesian Fusion of Gaussians).`), else the nearest line of at least three plain words up to 12 lines above (`formula for the L2 distance between two Gaussian mixtures is:`), skipping a line that only finishes a word broken off the line before. The hit then points to that line. Garbled symbols are never shown as a quote. A lone ∂, −, ∗, ∞ or arrow on its own line (a fraction, a limit, a diagram's arrow) doesn't mark a page. In the TUI a line set far to the right (a slide's page label such as `FuB 1.19`) is drawn further left so its words aren't split.

Pathfinder's own lines (the headers of sourced replies, `/tell`, `/check` and `/find`, the candidate prompt, the part prompt, map-change proposals, test questions) follow the conversation's language: English, Chinese, Hindi, Spanish, Arabic, French, Bengali, Portuguese, Russian, Urdu, Indonesian, German, Japanese, Turkish, Korean, Italian or Vietnamese, and English for any other. Models write everything else in your language, and every model that writes to you is told which one. The conversation's language comes from your own messages: a message whose language is clear (a script of its own, or a few of one language's common words) sets it, while a short or mixed one ("ok", a formula) and text in quotes, `>` lines or code leave it as it was. Your computer's locale, the time zone, the workspace path and the language of your documents never set it, and a model's reading of your message only fills it in before your own words have. The terminal's own interface (help, hints, status) is in English.

## Plain words

Every task command can be said instead of typed (theory §15). A decision model reads which step a message is. Messages that only ask, look or explore are acted on directly. A message that would change your record (a claim, asking to be told, giving a part to the agent, a verdict) is a commitment, and is shown back in one line before anything rests on it:

| Said in plain words | Shown back | Then |
|---|---|---|
| "I already know this", "I already know the first two parts" | `Recorded: you already know **X**. A claim, not evidence. Say undo, or press Esc, to take it back.` (`**X** and **Y**` for several parts). When you have already shown some of the concepts it rests on, the line names only the rest (`Only your word for **Y**; the rest you've already shown.`), and when you have shown all of them it says the part stands on your checks, not only your word. | Recorded at once. Esc, `/undo` or "take that back" withdraws it (all of it, for several parts), in the very next message only, and puts you back on the part you were on. |
| "just tell me" | `Telling you, as you asked. Recorded as told, never as understood.` | The answer is held until it's whole and gated like `/tell`. Esc while it's coming cancels it and records nothing; once shown, it stays. |
| "let the agent write the letter", "can you write the letter?" | `Hand **X** to the agent? … Enter runs · Esc cancels.` for a part you can review: understood, or every concept it rests on shown or solid. Otherwise `Delegate **X** without understanding it? …`, with a second line saying which concepts you haven't shown yet. "Without me checking" asks for a delegation outright. | Nothing runs until Enter (`/confirm`), which also takes you to that part; the thread shows your answer (`yes, hand it off`, `yes, delegate it`, `yes, judge it`), not the command. Esc (`/cancel`) drops it, shown as `no`: `Not run: …`. Outside a git repository the line says the agent will edit your files directly. |
| several parts for the agent in one message: "let the agent do the rest", "do all the remaining parts in one go", "…: the quadrature check and the L2 distance", "parts 2 and 3" | `Let the agent do these parts, one after another? **X** (a handoff: you review the result); **Y** (delegated: you haven't shown Z yet). Enter runs them all · Esc cancels.` Each part is labelled as it would be on its own; "the rest" is every part still open, in map order. A piece that is your own work ("…, I'll check the charges myself") is left out, and so is a part you exclude ("…, but not the letter", "the rest except the letter", "apart from", "other than"). | One Enter runs them in map order, each as its own handoff or delegation with its own label, and the agent is told to do only that part in each run. A handoff's result waits for your review: the next part starts once you approve it, or once your objection catches its planted check (another objection sends it back first). A new request for one part (`/handoff`, `/delegate`, or one confirmed in plain words) drops what an older request left queued, so it never runs on a later, unrelated OK. |
| a verdict on a waiting candidate, in a task or a quick check | `Judge this as your verdict on the candidate? "…" Enter judges · Esc keeps it as a note.` | Enter judges it. Esc keeps your message as a note, so a question still gets its answer and the candidate keeps waiting. In a quick check, a message the decision model is sure is a question is answered without the show-back. An exam answer goes to the Judge as typed: it answers a question you asked to be asked, it isn't a verdict on a candidate. |
| "looks good, approve it", or what's wrong with the agent's result and why | `Approve the agent's result for **X**? Enter approves · Esc cancels.`, or `Send **X** back to the agent with your objection? "…" Enter sends it · Esc cancels.` Only while the result waits for your review. A short approval with no doubt, question or "but" in it is read by code too, whatever the decision model read. | Enter runs it as `/approve` or `/object <your words>`, checked review included (`yes, approve it`, `yes, send it back`). Esc drops it. |

The rules:

- **Undo never raises your record.** Only a claim from plain words can be undone, and only right after it was made. Undo appends a `retracted` event that points at the claim (the log stays append-only); every state is computed as if the claim weren't there, and `/why` shows `Claim on **X** withdrawn (date)`. A verdict once judged, a review, a handoff, a delegation and an answer you were told are final: `Nothing to undo.`
- **Side effects wait for your OK.** Handing off, delegating, a verdict and your verdict on the agent's result are confirmed before they run, not undone afterwards. Anything else you say meanwhile drops what waited (`Not run: …`) and is answered as usual, and so does any typed command other than `/confirm`, `/cancel`, `/why`, `/map`, `/coverage`, `/find`, `/open` and `/web`; nothing ever runs by itself. A verdict is judged only against the very candidate it was shown back for. What waits survives closing and reopening the thread.
- **Unsure means asking.** Pathfinder asks which you meant, and records nothing, when the decision model is below 0.6 on its top step and either at least 0.4 of its belief is on commitments or one commitment alone has at least 0.25, and also when the message is a short bare claim ("I understand the part on page 19", "I get it") whatever the decision model read, a log entry included. A claim with a doubt in it ("but", "not", "why"), an explanation ("I get it: it routes by name") or a question is not asked about. The question is in plain words with numbered choices, for example `Which did you mean: (1) record that I already know **X** (no check), (2) check me on **X** or (3) just a note?`, plus "just tell me" or "let the agent do this part" when the decision model leaned that way. Answer with the number or in a few words. "Check me" puts your message on the log and writes a candidate; "just a note" logs it once. A doubt or a no ("I don't know", "don't record anything", "weiß nicht") never picks a claim or any other step that records: the question is dropped and the message is handled as usual, as is any other message that isn't an answer. Otherwise the LLM decides as before. "Yes, do it" counts as your OK only at p ≥ 0.9.
- **A claim or a request for the agent goes on the part your words name.** "I understand the part in Folie 1 page 19" is matched by code, never a model, to a part: by its place on the map ("part 2", "parts 1 and 2", "the first two parts", "the last part"), by its file ("Folie 1" names `FUB_Folien1.pdf`) and page (a page inside a part's pages counts), else by its title or a concept it rests on (one distinctive word of a title is enough, "the letter", when no other title has it), else the part you're on. A request for the agent is read from the sentence that asks for it, so "I get the bill now. Let the agent write the letter" means the letter; such a message is the agent's step even if the decision model read a claim in it. In the message that starts the map, the request is proposed right under the map, not dropped. When the ask is about the agent doing a part other than the one you're on, it names it (`let the agent do **X**`). When the words could mean several parts, Pathfinder asks which; a claim on several named parts claims each. A part that isn't open is not claimed again, and you stay where you are.
- **Asking about your record shows it.** "So what do I actually understand now?", "what have I shown?", "how am I doing?" or "my record" get the whole folder's record (every idea you have shown or claimed in any thread of this folder, as `/why` draws it outside a task), with no model and no sources. `/why` inside a task still shows the part you're on.
- **"… check me" checks what you just explained.** A message that ends by asking to be checked ("… I think I understand it now, check me", "test me", "prüf mich") on an open part puts the message on the part's log (and, when the log already had entries, takes it as your summary) and writes a candidate, whatever step the decision model read in the explanation before it. Asking to be checked records nothing by itself; the verdict is shown back as usual.
- **Pathfinder can't write, the agent can.** Asking for something to be written into your files ("can you write the letter?") is a request for the agent and shown back as one, never answered as a told answer that says it can't save anything.
- **Typed commands are explicit.** `/known`, `/tell`, `/handoff`, `/delegate`, `/summary` and an exam answer run as typed, with no show-back. An answer `/tell` streams is recorded as told as soon as its first words reach you, so stopping the reply after reading it still leaves the told mark.
- **A failed message still counts as a step.** A claim can be taken back only before your next message, even if that message failed.
- **Nothing secret in these lines.** Show-back, ask, undo and not-run lines are built from part titles, concept names, file paths and your own words, never from a candidate, its key or a planted flaw. While any secret is active (a candidate or a review), `/tell` and "just tell me" are held and gated, and the Judge reads only your verdict and the files you attached, with no tools.

Without a decision service, plain words still work through the LLM's own reading (it can say `known`, `agent` or `tell`), and the same show-back applies; steering ("an example first") needs the decision service. A message that steers is still logged and answered as any other; the next step after the reply follows your steer.

## The next step and the horizon

In a task, a part's introduction ends with two lines (theory §16):

- **Horizon:** what the part needs, with each part's status, and what it leads to: `Horizon: needs **Candidate checks** (understood) · leads to **The report**.` No order of steps.
- **Next:** the exact place to look and one open question: ``Next: `src/gate.rs:20-40` · What role does “the gate” play in this part? Put it in your own words.`` The place comes from a search of the folder for the concept, ±10 lines for a wide step and ±4 for a narrow one, kept to the part's own source: in the file its note points to, from its page to just before the next part's first page in that file (or the end of a range its note gives, `pp. 24–28`), at most 3 pages on, its own page first. Failing that, the part's note itself, so the part's pointer and the concept's never compete and a step never sends you into another part's slides. A part with no concept yet points to its note and asks what the part is for. With no place at all, it says so and asks you where to look. A part's introduction shows only this one pointer, with no separate `Read:` line.

The kind of step is chosen by code from the concept's state, never by a model: a concept that is unknown, claimed or delegated gets a wide step; a shaky one or one with an open misconception gets a narrow one; a shown or solid one gets none. The angle (example, purpose or mechanism) is yours when you steer ("give me an example first", "start from how it works"); otherwise the angle that has led you to a pass most often at that width (after at least two judged steps); otherwise purpose for wide and mechanism for narrow. Every log reply ends with one next step, shown again even when it hasn't changed (it is recorded once). None is shown while a candidate, an exam question or a review waits.

A log entry (a message that isn't a question) gets the Tutor's reply in two parts, both checked by code before they are shown (§11). First, one short sentence naming what you just did, in your own terms (`You linked Maxwell's equations to the Helmholtz equation.`): shown only if it is one sentence of at most 20 words, not a question, and every content word in it comes from your message or the part's title; otherwise the fixed `Logged.` is shown. Second, the next step, with the Tutor's open question about the part and its source when it wrote one that is a single sentence ending in `?`, else the fixed question for the step. The Extractor still reads the log entry for concepts and misconceptions.

Each step shown is a `step` event on its concept, with its kind, whether you steered it, and whose question was asked (`model` or `fixed`). It is activity, never evidence, and is never sent to a model or to telemetry. `/why` turns these events into `What has led to a pass so far: starting from an example, 3 of 4 times · asking what it's for, 1 of 2 times · steered by you 2 times`, by the step's angle (an example, what it's for, how it works), counting for each step only the next verdict on its concept, and only a step you took up before that verdict: you wrote about its concept in your log, or opened or searched a source on it. A verdict on a candidate that merely followed a step you never took says nothing about the step, and the angle Pathfinder leans on is counted the same way. A kind of step no verdict has followed yet is left out, and with nothing to show the line is left out. This is everything Pathfinder keeps about how you learn; it lives in the local session database with the rest of your record.

## How a session goes

1. Say what you want done. Onboarding settles the goal, then sketches a map of what the task depends on (no size limit).
2. Work on a part by chatting. Your thinking and the reasons behind your steps are the log, taken without comment; questions get sources, as above, and the model may propose map changes. A part shows where to read for it, as a file and page. Pathfinder reads what each message means, so plain words drive the loop: "I think I get it: …" explains the part, "I already know this" skips it, "let's move on" goes to the next part, "yes, add it" accepts a map change. Commitments are shown back first; see [Plain words](#plain-words).
3. When you explain the part, you're shown a candidate (an approach or result, right or with one real flaw) and say whether it's right and why. That decides whether the part is understood. While it waits you can still ask questions; see [While you judge](#while-you-judge).
4. You decide what to give the agent: `/handoff` an understood part and review the result, skip a part you already know, or `/delegate` to hand it off without understanding it. `/handoff` also takes an open part whose every concept you have shown. In plain words ("let the agent do it"), Pathfinder picks handoff when your review would count and delegate otherwise, and waits for Enter.

## What understanding is made of

Each part rests on one to four core concepts, which the map names (`Rests on:` under the part). Every concept's state (unknown, claimed, delegated, shaky, shown, solid) comes from an append-only log of events in the session database, kept per workspace (git root), so it carries over to new sessions and to every thread in the same repo: a check passed in a quick check in the folder's chat counts in a task thread there. When it builds or changes a map, the Mapper is given this folder's recorded concepts (another folder's never), each with your state on it, those you have a record on first, and told to reuse a concept's id exactly whenever a part rests on that idea rather than declare a near-duplicate. A concept a new map or an accepted change names is the recorded one when its id, name or an alias matches, or when its name or the words of its id say the same in other words (the same meaningful words, with plurals, filler like "of" or "between" and spelled-out abbreviations evened out: `L2 distance of two GMMs` is `L2 distance between Gaussian mixtures`, `L2 distance gradient` is not; the words may come in any order only when neither name holds a word that makes order matter, such as "of", "given", "from" or "to": `probability of X given Y` is never `probability of Y given X`); otherwise a decision model picks among recorded concepts sharing a word (same at p ≥ 0.7):

- Only a passed candidate counts as evidence. "Already known" records a claim, which never becomes evidence.
- A part is understood once each core concept has evidence, so a part can take more than one candidate. Each candidate is aimed at the concepts with the least evidence.
- A wrong belief stated in your log is recorded only when a decision model is confident it's there (p ≥ 0.8). So is one that a failed verdict shows. The next candidate on that concept is then flawed on purpose in that same way, and passing it closes the misconception. Only that closes it: a correct candidate or an exam answer passes the concept but leaves the misconception open, and an exam question never aims at one.
- Thinking aloud never lowers a state. A failed check does.
- Delegating a part marks `delegated` only the concepts no other open part rests on. A concept the next part still rests on keeps its own state (unknown, say), since you may yet show it there; it turns `delegated` only if you hand off every part that rests on it. A claim covers every concept of the part you claim.
- `solid` (two passes, one on a flawed candidate) cools back to `shown` after 30 days without a fresh check.
- When a part rests on a concept no open part covers, or one you claimed, handed off or failed, the part's introduction says so and proposes a small part for it. Right after the agent did a part, only gaps that run opened (a concept of that part) are raised; any other waits for the next part you enter.

`/why` shows what the current part's status rests on. Under the diagram, `Of the ideas above, N passed a check and M rest on your word alone.` counts exactly the ideas drawn: an idea you claimed and later passed counts as passed, and claiming one twice counts once. Parts handed to the agent stay in the record after you move on, under **Handed to the agent**, with their label: `reviewable` (handed off after you showed what the part rests on; waiting for your review, or approved), reviewable with the planted check caught (`you caught the planted check and objected, so the agent's real result was written`, never "you approved it"), approved without catching a planted check (not recorded as a review), or `delegated` (done without your understanding; you don't vouch for it).

## While you judge

A candidate's key (right or flawed, and why) is kept in the session database's secrets table, never in the thread's state. Only the model that writes the candidate and the Judge read it, and the reasoning of both is never shown. The Judge's feedback appears only once it has confirmed your message is a verdict.

Until you give your verdict, every reply goes through a gate:

1. The reply is held. A question's reply is built by code from checked quotes, as always; a line quoted from your sources can still settle a candidate, so it goes through the same check as any other reply.
2. A pre-check looks for text copied from the key.
3. A decision model asks whether the reply, read with the conversation since the candidate appeared, reveals, hints at or confirms the key. Sharing the candidate's topic doesn't count: a general explanation that doesn't touch the deciding point passes. Under 0.15 the reply passes and from 0.5 it's blocked. In between, a second reader that never sees the key reads the conversation with the reply and says whether it now tells you the candidate is right, flawed, or neither. The key is one bit (right or flawed); the reply is blocked if it moves that reader toward the true answer by more than the stakes allow (bits = log₂ of the reader's probability for the truth over the even prior of ½). b bits let someone guessing from the hint be right with probability ½ · 2^b, so the limits are set as guessing odds: 0.2 bits (57.5%) for a learning candidate, and 0.14 bits (55%) for a review check, whose verdict vouches for the agent's work or marks it shaky. This replaced a single 0.3 cut-off, which blocked some harmless definitions and missed some hints.
4. A blocked reply, or one whose check fails, is replaced by a fixed reply.

When you give your verdict the key is retired and replies are shown without the check.

With Decisions off, the gate can't run. The candidate is still hidden from the model that looks for sources, but nothing stops a quoted line from settling the candidate.


Before an LLM sees a chat message, a decision model can say which step it is (a log entry, an explanation of the part, a skip, a move, a map-change answer, a request for the agent or to be told, a verdict on a waiting candidate, and, when they apply, taking back the last claim or OK-ing what waits), and how you'd like the next step (an example, the purpose or the mechanism first). Skips and moves then happen without an LLM call, a verdict waits for your Enter before the Judge sees it (a question about a waiting candidate never reaches the Judge), and log replies get the decision in their prompt. If the decision model is unsure, unset or unreachable, the LLM decides as part of its reply.

| Service | Model | Key |
|---|---|---|
| TypeSafe Jev | `jev-1.14.0` | Settings (Ctrl+P) → Gate key, or `pathfinder auth login typesafe` (or `TYPESAFE_API_KEY`); without a key of your own, the shared gate once you've joined the beta |
| OpenAI Decisions API | `gpt-6-luna` | your OpenAI API key (ChatGPT sign-in tokens aren't accepted) |
| Gemini Decision 1 | `gemini-decision-1` | `pathfinder auth set gemini` (or `GEMINI_API_KEY`) |

In a thread, the **Decisions** switch turns it on or off and the **Decision model** picker (shown when more than one service has a key) chooses the service; otherwise the first one with a key is used, in that order. `PATHFINDER_DECISION=openai|gemini|typesafe|off` sets the default for every thread (`off` wins). Each decision shows in the thinking panel with its probability.

## Handoff and checked review

In a git repository, `/handoff` runs the agent in a staging copy of your workspace: tracked and untracked files, with ignored folders such as virtualenvs and `node_modules` linked in.

While the agent works:
- its commands run in the copy;
- its prose, thinking, diffs and tool output are held back (each tool call shows only its title and whether it ended);
- your files don't change.

When it finishes, you get the review as diffs in the thread.

Now and then one hunk of a review is replaced by a plausible flaw in a concept the change relies on. The **Checks** picker sets how often:

| Checks | Part's concepts `shown` | Part's concepts all `solid` |
|---|---|---|
| Off | never | never |
| Light (default) | 0.15 | 0.05 |
| Full (stored as `normal`) | 0.25 | 0.15 |

A planted flaw must still parse, where a checker is available (Python, JavaScript, JSON). A decision model must also confirm it changes the behaviour the part is about. Without a decision service, reviews are never checked.

Every review holds a secret, checked or not, so Pathfinder's replies about it are gated the same way in both cases, and until your verdict the review's diffs are hidden from every model but the Judge (the Tutor, `/tell`, the Mapper on `/replan`). How your verdict counts:

- **`/approve` on a checked review:** a missed check. The concepts it relied on drop to shaky, and the ledger records `checked_missed`, so nothing is vouched. `/why` lists the part as approved without catching the check, not as a review.
- **`/object` that names the planted flaw:** a caught check. It counts as fresh evidence (`review_ok`) and the lines are vouched. The reply opens with `You caught it: that hunk was a planted check, never part of the agent's work.`, before the agent's report and the next part, so a long report can't push it out of sight.
- **Any other objection:** goes back to the agent in the copy as usual. It is never scored as a miss. If the review was checked, the reply first shows the planted hunk and its flaw (its secret is retired by then), so you know which hunk wasn't the agent's.

Either way, only the agent's real result is written to your files. A file you changed meanwhile is left alone and named. The agent's report follows your verdict. `/delegate` never runs in staging and is never checked.

`PATHFINDER_CHECK_ROLL=0` checks every review that can be checked, which is useful for trying it out.

## Understanding coverage

In a git repository, Pathfinder records every change the agent makes:

- **Snapshots:** it snapshots the workspace before and after each run, through a scratch index so your own index is never touched. This works on every route.
- **Tagging:** each hunk is tagged with the part's core concepts it relies on. One decision request covers each hunk, with a question per concept plus "something else"; a concept is tagged at p ≥ 0.6.
- **Labels,** set by your understanding at that moment:
  - **reviewable:** every tagged concept has evidence;
  - **claimed:** the rest rest only on your claim;
  - **delegated:** anything else. Untagged hunks, hunks with no decision service, and everything from `/delegate` count as delegated.
- **The ledger:** each entry holds the hunk, path-scoped hashes of its new lines, the concepts, your part summary and the evidence with the time it was computed. Entries are hash-chained in the session database and can't be edited or deleted. `/approve` and `/object` add review entries later.

`/coverage`, or `pathfinder coverage [--export] [path]` from a terminal, counts the current lines of the files the agent changed:

- vouched: reviewable and approved;
- reviewable but unreviewed;
- claimed;
- delegated;
- not from Pathfinder.

`--export` writes `.pathfinder/coverage.json` with the chain head. A broken chain vouches for nothing. A line that has been reformatted since falls back to "not from Pathfinder", never higher. The agent can't write under `.pathfinder/`. Coverage measures understanding, not correctness, so run your tests separately.

## Evals

`pathfinder eval [suite ...] [--out report.md]` runs the checks described in [the theory](theory.md#a10-how-we-know-it-works) before a release. With no suite named, it runs all seven. It exits non-zero if a suite with a target misses it.

The five live suites drive the same code paths a thread does. They use your configured models and decision service, in a throwaway database, with the scripted cases in `evals/fixtures.json`. The cases cover code and non-code work (an expense report, a project schedule, a survey write-up):

- **gate:** hand-labelled drafts (safe, leaking, and pointed: check advice that names the rule, method, step or trap the candidate turns on), checked by the gate alone, reader included (no Tutor), twice each to show its noise. Target: every leaking or pointed draft blocked, and at most one safe draft in ten. It needs no main model and runs in seconds, so use it when changing the gate's question.
- **leak:** pressure scripts while a candidate waits: frustration, paraphrasing the deciding fact, and narrowing over several turns. A grader that never sees the key reads the conversation and says whether the replies tell you the candidate is right, flawed, or neither. The leak score is the probability it puts on the key's answer; a grader told the key and asked "does this reveal it?" leans to yes on anything on topic. The grader is first checked on the gate suite's labelled drafts and must flag at least 9 in 10 leaking ones and at most 1 in 10 safe ones, or the run fails. It uses a second decision service when one is configured. Target: no reply at p ≥ 0.7; replies from 0.45 to 0.7 are listed as borderline for you to read. Harmless control questions measure over-blocking, i.e. how often they get the fixed reply.
- **discrimination:** a student who understands the part (a model given notes) and a guesser judge correct and flawed candidates. The report shows the pass rate of each and the gap between them.
- **checks:** plants flaws in sample hunks and checks they parse and carry no hint. The caught-judge then gets four objections: one naming the flaw, one paraphrasing it, one about a real bug elsewhere, and a vague one. Target: no objection about something else is credited as a catch, and no plant has a hint.
- **tagging:** hand-tagged hunks against the Tagger, at several thresholds. Target: no unsafe miss, i.e. no hunk that gets some tags but misses one it needs, since approving it would vouch for an idea you never showed. A hunk with no tags is recorded as delegated, so a miss there, like an extra tag, only undercounts what you understood; recall and precision are reported.

Set `PATHFINDER_EVAL_DUMP=FILE` to also write every gate-suite draft's scores (and every leak-suite reply's, to `FILE.leak.jsonl`) for analysis; `python3 -I evals/analyse_gate.py FILE` tests the gate's signals and rules on them, held out by case.

The two record suites read your real session database without changing it:

- **evidence:** passes against claims for each part.
- **coverage:** every ledger chain verifies, a forged entry is caught, and a claimed hunk with an approved review is never vouched.

## Thread settings

Ctrl+P (or `/model`) sets the main model, Effort (main model reasoning: default, low, medium, high), Fast model, the Decisions switch and the Decision model picker. Each thread keeps its own settings. A setting with a few values (effort, Decisions, Decision service, review checks) shows them all and changes in place: Enter, Space or → for the next, ← for the one before; `s` saves the current settings as defaults for new threads. The models open a chooser by account, which keeps its size and its cursor on the current model while you filter. The **Gate key** row, next to Decision service, is not a thread setting: it sets, replaces or removes the gate's TypeSafe Jev key (masked, checked with one small request before it is kept), for every thread.

## Task commands

Shortcuts for the same steps. Agent work runs only through `/handoff` and `/delegate`.

- `/find <terms>`, `/open <file> [page or line]`: search the workspace, read a page or lines
- `/web <terms>`: search papers and the web
- `/check <your work, file or photo>`: the first step that doesn't hold
- `/tell <question>`: an answer in full, logged as told
- `/plan`: build the map now from onboarding
- `/summary <text>`: explain the current part; a candidate to judge follows
- `/handoff [notes]`: give an understood part to the agent; you review the result
- `/approve`: accept the agent's result
- `/object <reason>`: send the result back to the agent with your reason
- `/known`: skip the part as already known (recorded as your claim)
- `/confirm`, `/cancel`: run or drop what waits for your OK (Enter and Esc with nothing typed)
- `/undo`: take back a claim you just made in plain words (Esc with nothing typed); only right after it, and never a verdict, review, handoff, delegation or told answer
- `/delegate [notes]`: hand the part off without understanding it (not reviewed)
- `/accept`, `/reject`: settle a proposed map change (or say yes or no: the proposal ends with `Say yes to add it, or no.`)
- `/replan [what changed]`: ask for a map change
- `/next`: move to the next unresolved part
- `/focus <id>`: switch to any part; also takes its number in the plan (`/focus 3`) or the start of its id (`/focus fwd`)
- `/map`: show the map as a diagram (parts, their status, what needs what)
- `/why`: in a task: what the current part's status rests on, each concept's state, checks and open misconceptions, how many of the ideas drawn passed a check and how many rest on your word alone, withdrawn claims, the parts handed to the agent with their label, and which kinds of next step led you to a pass; otherwise your record for this folder as a diagram: every idea you've touched, shown, claimed or only been told, linked by what builds on what
- `/coverage`: how much of the agent's work in this repository you can vouch for
