# Using Pathfinder

This is one session, from start to finish. Everything here works in plain words. Commands are shortcuts if you want them.

## Open it in your folder

Go to the folder you're working in and start Pathfinder:

```sh
cd ~/projects/thesis
pathfinder
```

The model reads the code, notes and papers there as it needs them. PDFs are quoted page by page if poppler is installed (`brew install poppler`). Each thread (Ctrl+N) is its own conversation; your record belongs to the folder.

## Say what you want to understand

Ask in your own words. You won't get an answer. You get where to look: a file and line, or a PDF page, with a short quote that was checked against the file.

```
- `attention.pdf p.5 l.14`
  > In this work we employ h = 8 parallel attention layers, or heads. For each of these we use
```

If your folder has nothing on it, Pathfinder searches papers online the same way, or says **Not found** and suggests terms.

## Read where it points

Open the place and read it. You do the thinking. If you're stuck, ask again more narrowly. When you'd rather just be told, say "just tell me" or type `/tell`. You get an answer, and the record marks it as told, not understood.

To find a mistake in your own written working, `/check` quotes the first wrong step. It doesn't say why.

## Say what you think

If you're working on a task, write out your thinking as you go. Pathfinder keeps it with the thread and replies with one next step: the exact place to look and one question to take there. Say "give me an example first" and the next step changes.

## Judge an explanation when it clicks

When it clicks, explain it in your own words. You then see a short explanation that is either right or has exactly one real mistake. Say which, and why. Only passing goes on your record as understood.

While you judge, the gate holds back replies that would give the answer away. It needs a decision key or the beta's shared gate. If you'd rather not judge now, just ask your next question. Nothing about the explanation is recorded until you judge it.

You can also say "I already know this". That goes on the record as your claim, not as understanding.

## Hand parts to the agent

When you understand a part, hand it off: say "let the agent do this one", or type `/handoff`. The agent does the work and you review the result with `/approve` or `/object`.

A part you don't understand can go too (`/delegate`). That's fine, but the record says you couldn't judge it.

When you say these things in plain words, Pathfinder shows back in one line what it will record. A claim is recorded at once, and Esc takes it back. A handoff or a verdict waits for Enter, and Esc cancels it. Typed commands act at once.

## Look at your record

`/why` shows your record: the ideas you've touched, and whether you showed, claimed or were only told each one. Inside a part, it shows what that part rests on. The record is meant to be honest, so a verdict, a handoff or an answer you've seen can't be undone.

## Worth knowing

- F1: the keys
- Ctrl+N: new thread
- Ctrl+P: settings and models
- `/find <terms>`, `/open <file> [page]`: search and read yourself
- `/web <terms>`: search papers online (and the web, with a Brave key)
- `/tell`, `/check`, `/handoff`, `/delegate`, `/why`: as above
- `/login`: connect models
- `/bug what happened`: send a bug report (you see it all first)

Everything else is in the [reference](reference.md).
