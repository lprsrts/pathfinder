# Using Pathfinder

Pathfinder is an assistant for real work (code, analysis, research, writing) with one difference: it knows the difference between being told something and understanding it, and keeps an honest record of which is which.

## Open it

In a terminal, go to the folder you're working in and start it:

```sh
cd ~/projects/thesis
pathfinder
```

Pathfinder finds and reads the files there by itself: code, notes, papers, PDFs, figures included. F1 shows the keys. Start a new thread (Ctrl+N) for each topic or task.

## Ask, and get told where to look

Ask in your own words, in any language. Pathfinder doesn't answer: like a librarian with a catalogue, it points to where your material addresses the question and shows a short verbatim line from there.

```
**Where to look:**

- `attention.pdf p.5 l.14`
  > In this work we employ h = 8 parallel attention layers, or heads. For each of these we use
```

Every quote is checked against the file it cites before you see it, and nothing else is shown: no explanation, no summary, no conclusion. You read, calculate and decide. If nothing in the folder addresses the question, it says **Not found** and suggests terms to search for.

Read and search yourself:

- `/find <terms>`: matching lines across the folder's files and PDFs.
- `/open <file> [page or line]` (or `/open notes.md:120`): the page or lines, numbered.
- `/web <terms>`: papers from OpenAlex and arXiv and pages from Brave Search, each with a line checked against what was fetched. When the folder has nothing for a question, this search runs by itself.

Searches don't mind spelling: "k-means", "K means" and "kmeans" find each other, and so do "Schrödinger", "Schroedinger" and "Schrodinger".

What you open and search is on your record as *read*; it grows the graph but never counts as understanding.

Math on PDF slides (Greek letters and symbols from the Symbol font, Word's equations) is turned back into the right characters where Pathfinder can tell which font drew it. A page where it still can't gets a line above it in `/open` saying its math didn't extract cleanly and to check it in your PDF viewer, and its garbled lines are never quoted as the source's words.

To check your own work, `/check <your work>`, `/check derivation.md` or `/check photo.jpg` quotes the first step that doesn't hold. It doesn't say why or how to fix it. A photo of handwritten work is copied into text first and the copy is shown, so you can see it read your steps right.

When you do want an answer, `/tell <question>` gives one. It's marked **Told** and recorded as told, not as understood.

## Where understanding comes from

You don't need to ask for a check. It comes at the moments that matter, and there are no special phrases: Pathfinder reads what you mean.

- **When you say something has clicked**, it shows one short *candidate*: an explanation that is either right or has exactly one real mistake. Say whether it's right and why. If you'd rather move on, just ask your next question; nothing is recorded then.
- **When you want to be tested on a topic**, say so. Pathfinder asks open questions one at a time, grades each answer with what was missing, and goes on until you stop.
- **When you want to do a task yourself** and say so, it splits the task into parts, each with where to read for it, never the result. Think out loud as you work; questions get sources, the rest is logged. When you can explain a part, say so and get a candidate for it.
- **When you want the agent to do the work**, `/handoff` gives it a part you understand and shows you the result to approve or object to. `/delegate` hands off a part without understanding it, and the record says so.

Passing a candidate or a test question is what puts an idea on your record as shown. `/why` shows, per idea, what you've shown, claimed or only been told.

## Say it in plain words

Everything above works without a command. "I already know this part", "just tell me", "let the agent do this one" and "it's right, because …" are read like the commands they stand for. When one of them would change your record, Pathfinder shows it back in one line before anything rests on it:

- **"I already know this"** is recorded at once as your claim and shown back. Esc (or saying "undo", or "no, take that back") withdraws it, but only right away, before your next message. The record keeps it as withdrawn.
- **"Just tell me"** is shown back first, then answered and recorded as told. Esc while the answer is still coming cancels it and records nothing; once you've seen the answer, it stays on your record.
- **"Let the agent do it"** waits for your OK: Enter hands the part off (or delegates it, if you haven't understood it yet), Esc cancels. Nothing runs before you press Enter.
- **A verdict on a candidate** in your own words waits for Enter too, since a judged verdict is final. Esc keeps your message as a note, so a question about the candidate still gets its answer.

A verdict, a review, a handoff, a delegation and an answer you were told can never be undone: taking them back would make your record look better than it is. When Pathfinder can't tell whether you meant one of these, or you say something as short as "I get it" or "I understand the part on page 19", it asks which you meant in plain words, with numbered choices such as "record that I already know it (no check)", "check me on it" and "just a note", and records nothing until you say. Answer with the number or in a few words; "I don't know" or "don't record it" records nothing and goes on as an ordinary message. A claim goes on the part your words name (by title, idea, file or page), and taking it back returns you to where you were. Typed commands (`/known`, `/tell`, `/handoff`, `/delegate`) are already explicit and run as they are. A typed command that changes things (such as `/summary` or `/plan`) drops what waited for your OK, and says so.

## The next step

In a task, each part ends with where to go next: the exact place (`Next: src/gate.rs:30-50`) and one open question to take there, and the part's horizon: what it needs and what it leads to, never the order to get there. A concept you haven't touched gets a wide step; a shaky one gets a narrow one. Say "give me an example first" or "start from what it's for" and the next step changes. The place is always inside the part's own source (its file, and its own pages of a slide deck), never another part's. When you write out your thinking, the reply names what you did in your own words, adding nothing, and ends with that one next step; if the acknowledgement would add anything of its own, you get "Logged." instead. While a candidate or a review waits, no next step is shown.

Over time Pathfinder leans on the kinds of step that led you to a pass. That is the only thing it keeps about how you learn: `/why` shows it in full ("How you got here: wide·example 3 of 4 led to a pass …"), and it lives in the same local database as the rest of your record.

## Commands, if you want them

| Command | What it does |
|---|---|
| `/find <terms>`, `/open <file> [page]` | Search the folder; read a page or lines |
| `/web <terms>` | Search papers and the web |
| `/check <your work>` | The first step that doesn't hold, without the why |
| `/tell <question>` | A reasoned answer, recorded as told |
| `/why` | Your record: every idea you've touched here and whether you've shown it, claimed it or were only told, claims you withdrew, and which kinds of next step led you to a pass |
| `/map` | In a task: its parts as a diagram |
| `/summary <your explanation>` | In a task: explain the current part; a candidate follows |
| `/next`, `/known` | In a task: the next part; skip a part you know (recorded as your claim) |
| `/confirm`, `/cancel`, `/undo` | Run or drop what waits for your OK (Enter, Esc); take back a claim you just made in plain words (Esc) |
| `/handoff`, `/delegate` | Give a part to the agent, understood or not |
| `/accept`, `/reject` | Answer a proposed extra part for something a part rests on |
| `/share [note]` | Send this thread (you see it all first) |
| `/bug [what happened]` | Send a bug report (you see it all first) |

In the terminal there are also `/help`, `/quit` (or `/exit`), `/new` (or `/clear`), `/resume`, `/model`, `/status`, `/login`, `/logout`, `/copy`, `/join`, `/data` and `/update`. Type `/` to see what matches, and Tab to complete. `/help` lists everything, keys included.

## Models

`pathfinder auth login` connects models in two steps: how to connect (your ChatGPT plan, an API key you pay per use, or your own server), then, for a key, which provider (type to filter, Esc goes back). Each row shows its state, such as `✓ signed in as …` or `✓ stored`. `/login` shows the same panel inside Pathfinder, in place of the input, and opens by itself the first time no account works. Claude Code and Codex keep their own logins (`claude`, then `/login`; `codex login`); the gate's key is in Settings (Ctrl+P). Ctrl+P picks models for a thread: a fast one for quick replies and a main one for candidates, judging and agent work. See the [reference](reference.md#models-and-routes).

## If something's off

- **"I can't see the file":** start Pathfinder in the folder that holds it, or attach the file.
- **No models:** `/login` (or `pathfinder auth login`), or `pathfinder auth status` to see what's connected.
- **A reply was held back** while you judged a candidate: that's the gate. Give your verdict, and the answer follows.
- **Anything else:** `/bug what happened`.
