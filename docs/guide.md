# Using Pathfinder

Pathfinder is an assistant for real work (code, analysis, research, writing) with one difference: it knows the difference between being told something and understanding it, and keeps an honest record of which is which.

## Open it

In a terminal, go to the folder you're working in and start it:

```sh
cd ~/projects/thesis
pathfinder
```

Pathfinder finds and reads the files there by itself: code, notes, papers, PDFs, figures included. F1 shows the keys. Start a new thread (Ctrl+N) for each topic or task.

In Zed, open the folder as the project and start a new Pathfinder thread in the agent panel. Everything below works the same there.

## Ask, and get told where to look

Ask in your own words, in English or German. Pathfinder doesn't answer: like a librarian with a catalogue, it points to where your material addresses the question and shows a short verbatim line from there.

```
**Where to look:**

- `attention.pdf p.5 l.14`
  > In this work we employ h = 8 parallel attention layers, or heads. For each of these we use
```

Every quote is checked against the file it cites before you see it, and nothing else is shown: no explanation, no summary, no conclusion. You read, calculate and decide. If nothing in the folder addresses the question, it says **Not found** and suggests terms to search for.

Read and search yourself:

- `/find <terms>`: matching lines across the folder's files and PDFs.
- `/open <file> [page or line]` (or `/open notes.md:120`): the page or lines, numbered.
- `/web <terms>`: raw web hits under the same rules (not set up yet).

What you open and search is on your record as *read*; it grows the graph but never counts as understanding.

When you do want an answer, `/tell <question>` gives one. It's marked **Told** and recorded as told, not as understood.

## Where understanding comes from

You don't need to ask for a check. It comes at the moments that matter, and there are no special phrases: Pathfinder reads what you mean.

- **When you say something has clicked**, it shows one short *candidate*: an explanation that is either right or has exactly one real mistake. Say whether it's right and why. If you'd rather move on, just ask your next question; nothing is recorded then.
- **When you want to be tested on a topic**, say so. Pathfinder asks open questions one at a time, grades each answer with what was missing, and goes on until you stop.
- **When you want to do a task yourself** and say so, it splits the task into parts and gives hints, never the result. Think out loud as you work. When you can explain a part, say so and get a candidate for it.
- **When you want the agent to do the work**, `/handoff` gives it a part you understand and shows you the result to approve or object to. `/delegate` hands off a part without understanding it, and the record says so.

Passing a candidate or a test question is what puts an idea on your record as shown. `/why` shows, per idea, what you've shown, claimed or only been told.

## Commands, if you want them

| Command | What it does |
|---|---|
| `/find <terms>`, `/open <file> [page]` | Search the folder; read a page or lines |
| `/tell <question>` | A reasoned answer, recorded as told |
| `/why` | Your record: every idea you've touched here and whether you've shown it, claimed it or were only told |
| `/map` | In a task: its parts as a diagram |
| `/summary <your explanation>` | In a task: explain the current part; a candidate follows |
| `/next`, `/known` | In a task: the next part; skip a part you know (recorded as your claim) |
| `/handoff`, `/delegate` | Give a part to the agent, understood or not |
| `/accept`, `/reject` | Answer a proposed extra part for something a part rests on |
| `/share [note]` | Send this thread (you see it all first) |
| `/bug [what happened]` | Send a bug report (you see it all first) |

In the terminal there are also `/help`, `/quit` (or `/exit`), `/new` (or `/clear`), `/resume`, `/model`, `/status`, `/copy`, `/join`, `/data` and `/update`. Type `/` to see what matches, and Tab to complete. `/help` lists everything, keys included.

## Models

Ctrl+P (the toolbar in Zed) picks the models for a thread: a fast one for quick replies and a main one for candidates, judging and agent work. With a ChatGPT plan, Pathfinder uses OpenAI's models through your plan. With an API key it can use Anthropic's or OpenAI's models, billed per use, or models on your own server. See the [reference](reference.md#models-and-routes).

## If something's off

- **"I can't see the file":** start Pathfinder in the folder that holds it, or attach the file.
- **No models:** run `pathfinder auth status`.
- **A reply was held back** while you judged a candidate: that's the gate. Give your verdict, and the answer follows.
- **Anything else:** `/bug what happened`.
