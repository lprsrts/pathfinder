# Start here

Pathfinder makes sure you understand what you work on. Ask it anything, and when something matters, let it check you.

## Open it

1. **Quit and reopen Zed** once, so it starts the newly installed Pathfinder.
2. **Open your course folder** as the project, e.g. `~/dev/academic/kit/lectures/eet`. Pathfinder can then find and read your slides, scripts and exercise sheets by itself.
3. Open the agent panel and start a **new Pathfinder thread** (one thread per topic or exercise keeps things clear).

## Three ways to use it

**Ask.** Type a question in German or English and get a full answer from your material:

> Wie funktioniert ein Transformator? Erklär es mir anhand der IEH-Folien.

Nothing is recorded as understood. You were told, not shown.

**Check yourself.** After an answer, say **„prüf mich“** (or "check me"). You explain the topic in your own words, then Pathfinder shows you a *candidate*, a short worked answer that is either right or has exactly one real mistake, and you say whether it's right and why. Pass and it counts as understood. While you judge, you can still ask questions; Pathfinder won't give the answer away.

**Work an exercise.** Point it at the exercise and say you want to do it yourself:

> Ich will Aufgabe 1 aus dem IEH-Übungsskript selbst lösen. Hilf mir, ohne die Lösung zu verraten.

It splits the exercise into parts and gives hints, never the result. Think out loud as you work; when you can explain a part, say so (or `/summary …`) and get a candidate for it. Ask about figures ("Was ist in Abbildung 1a zu sehen?"): it opens the page and looks.

You can also **paste a screenshot** or **attach a PDF** (a photo of a sheet, a slide) to any message.

## Commands you'll actually use

| Command | What it does |
|---|---|
| `/summary <your explanation>` | Explain the current part; a candidate follows |
| `/next` | Move to the next part |
| `/known` | Skip a part you already know (recorded as your claim, not as shown) |
| `/why` | What you've shown, claimed or only been told, per idea |
| `/delegate` | Hand a part to the agent without understanding it (recorded as such) |
| `/accept`, `/reject` | Answer a proposed map change. Pathfinder proposes a small extra part when something a part rests on is missing or only claimed. |

Plain words work too: "weiter", "das kann ich schon", "ich glaube, ich hab's: …".

## Models

The toolbar at the top of the thread picks the models. Your defaults are **Haiku** (fast) and **Opus** (main) through Claude Code. Broad answers take 30 to 60 seconds there. Your **ChatGPT** plan is also listed (e.g. GPT-6 Luna fast, GPT-6.2 Sol main) and was quicker in testing, 13 to 30 seconds. Pick per thread.

## If something's off

- **Math looks like `\frac{…}`:** Zed can't render LaTeX; ask again for plain text.
- **"I can't see the file":** make sure the folder is open as the project, or attach the file.
- **No models in the picker:** `harness auth status` in a terminal shows what's signed in.
- Anything else: note what you typed and what you saw; that's what gets fixed next.
