# Start here

Pathfinder is your assistant for study and research, with one difference: it knows the difference between being told something and understanding it, and keeps an honest record of which is which.

## Open it

1. **Quit and reopen Zed** once, so it starts the newly installed Pathfinder.
2. **Open the folder you're working in** as the project: a course (`~/dev/academic/kit/lectures/eet`), your papers (`~/dev/academic/kit/literature`) or your code (`~/dev/academic/kit/fusion-gmbp`). Pathfinder finds and reads the slides, scripts, exercise sheets, PDFs and code there by itself, figures included.
3. Open the agent panel and start a **new Pathfinder thread**, one per topic or exercise.

Or, without Zed, in any terminal: `harness tui ~/dev/academic/kit/lectures/eet`. Everything below works there too; F1 shows the keys, and you can drop a PDF or a screenshot onto the window to attach it.

## Use it like any assistant

Just ask, in German or English. It answers from your material:

- "Ich check nicht, warum die Asynchronmaschine Schlupf braucht."
- "Wie kommt man im ETI-Skript auf M = k·Φ·I_A?"
- "Was muss ich aus FuB-Foliensatz 4 fürs Tutorium wirklich können?"
- "Gib mir die Lösung für Aufgabe 1a aus dem IEH-Übungsskript."
- "Explain Runnalls' KL-based mixture reduction." / "Wo im Code wird die Reduktion gemacht?"

**Paste a screenshot or a photo** (a slide, a sheet, your handwritten solution) or **attach a PDF** to any message. Show your own work and ask "stimmt das so?": it reviews it step by step and says exactly where it goes wrong.

Answers count as *told*, not as understood.

## Where understanding comes from

You don't need to ask for a check. It comes at the moments that matter:

There are no special phrases: Pathfinder reads what you mean.

- **When you tell it something has clicked**, it shows one short *candidate*: a worked explanation that is either right or has exactly one real mistake. Say whether it's right and why. If you'd rather move on, just ask your next question; nothing is recorded then.
- **When you want to be tested on a topic**, say so however you like, for example that you want to practise for the oral exam. It runs an **exam simulation**: open questions one at a time, the way an oral examiner asks them. Answer as you would in the exam. Each answer is graded with feedback on what was missing from your explanation, then the next question follows. Stop or ask something else whenever you like.
- **When you want to solve an exercise yourself** and say so, it splits the exercise into parts and gives hints, never the result. Think out loud as you work; when you can explain a part, say so and get a candidate for it.

Passing a candidate or an exam question is what puts an idea on your record as shown. `/why` shows, per idea, what you've shown, claimed or only been told.

## Commands, if you want them

| Command | What it does |
|---|---|
| `/why` | Your record as a diagram: every idea you've touched here and whether you've shown it, claimed it or were only told |
| `/map` | In an exercise or exam: its parts as a diagram |
| `/summary <your explanation>` | In an exercise: explain the current part; a candidate follows |
| `/next`, `/known` | In an exercise: next part; skip a part you know (recorded as your claim) |
| `/accept`, `/reject` | Answer a proposed extra part for something a part rests on |

## Models

The toolbar at the top of the thread picks the models. Your defaults are **Haiku** (fast) and **Opus** (main) through Claude Code: thorough, 20 to 70 seconds per answer. Your **ChatGPT** models (GPT-6 Luna, GPT-6.2 Sol) were often quicker in testing. Pick per thread.

## If something's off

- **Math looks like `\frac{…}`:** Zed can't render LaTeX; ask again for plain text.
- **"I can't see the file":** open the folder as the project, or attach the file.
- **No models in the picker:** run `harness auth status` in a terminal.
- Anything else: note what you typed and what you saw.
