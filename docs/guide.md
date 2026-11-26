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

## Ask anything

Ask in your own words, in English or German. Pathfinder answers from your material:

- "Why does this function hold the lock across the await?"
- "How does the paper get from equation 3 to equation 4?"
- "What would break if we switched the queue to at-most-once delivery?"
- "Here's my derivation, is it right?"

Paste a screenshot or a photo, or attach a PDF (`/attach file.pdf`, or drop it onto the window). Show your own work and ask whether it's right: Pathfinder goes through it step by step and says exactly where it goes wrong.

Answers count as *told*, not as understood.

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
| `/why` | Your record: every idea you've touched here and whether you've shown it, claimed it or were only told |
| `/map` | In a task: its parts as a diagram |
| `/summary <your explanation>` | In a task: explain the current part; a candidate follows |
| `/next`, `/known` | In a task: the next part; skip a part you know (recorded as your claim) |
| `/handoff`, `/delegate` | Give a part to the agent, understood or not |
| `/accept`, `/reject` | Answer a proposed extra part for something a part rests on |
| `/share [note]` | Send this thread (you see it all first) |
| `/bug [what happened]` | Send a bug report (you see it all first) |

The full list is in the [reference](reference.md#slash-commands-in-zed).

## Models

Ctrl+P (the toolbar in Zed) picks the models for a thread: a fast one for quick replies and a main one for candidates, judging and agent work. With a ChatGPT plan, Pathfinder uses OpenAI's models through your plan. With an API key it can use Anthropic's or OpenAI's models, billed per use, or models on your own server. See the [reference](reference.md#models-and-routes).

## If something's off

- **"I can't see the file":** start Pathfinder in the folder that holds it, or attach the file.
- **No models:** run `pathfinder auth status`.
- **A reply was held back** while you judged a candidate: that's the gate. Give your verdict, and the answer follows.
- **Anything else:** `/bug what happened`.
