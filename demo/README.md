# The demo recording

`collector/public/pathfinder-demo.gif` and `.mp4` show someone who isn't technical working out why their electricity bill went up: Pathfinder points to the lines in the bill, they say what they think, judge a short candidate, let the agent write a letter asking for a real meter reading, look at their record and read the letter. Captions in a band under the terminal say what is going on for viewers who don't know Pathfinder.

- `energy-bill/`: the two documents in the folder, a bill and a tariff sheet from Kestrelby Energy, an invented supplier. The PDFs are built from `energy-bill/source/*.typ` with `typst compile`.
- `record.sh`: sets up a throwaway home under `/tmp/pathfinder-demo` (so the screen shows `~/Documents/Electricity bill`), builds the release binary, starts a shell in a private tmux server and starts VHS on `energy-bill.tape`, which records that session.
- `say.sh`: plays the person. It types a message at a steady pace, sends it, waits for the reply and prints the screen.
- `cut.py`: shortens the waits, adds the captions from `captions.txt` and writes the mp4 and gif (captions are rendered with `typst`).
- `captions.txt`: the captions, each tied to a step (`3` from when step 3 is typed, `3+` from when its reply has arrived).

The candidate you judge is right or flawed at random, and the plan Pathfinder makes varies, so the lines can't all be scripted ahead: you read each reply and answer it.

## Re-recording

You need `vhs`, `tmux`, `ffmpeg`, `jq`, `typst` and poppler's `pdftotext`, and a Pathfinder home with a login (the demo uses the ChatGPT plan's GPT-6-Luna and GPT-6.2-Sol; set `PATHFINDER_DEMO_FAST` and `PATHFINDER_DEMO_MAIN` for others). Only `auth.json`, `config.json` and the cached model list are copied, into the throwaway home. Nothing about your own threads is shown.

```sh
PATHFINDER_DEMO_CREDENTIALS=~/.local/share/pathfinder-dev demo/record.sh
sleep 3
demo/say.sh --hold 3.5 "pathfinder"
demo/say.sh --hold 2 "My electricity bill went up a lot this month and I don't understand why"
demo/say.sh --hold 7 "ok so the price went up from 29p to 34p, and they guessed my meter reading. 320 kWh is way more than I use. I think I get it, check me"
demo/say.sh --hold 9 "right. the price per unit went up but the daily charge didn't, and 320 is only their guess so I can send a real reading"
demo/say.sh --hold 5 "great. let the agent write them a letter asking for a real meter reading and a corrected bill, and save it here"
demo/say.sh --hold 6 "I already know this part, we just did it"
demo/say.sh --hold 5 "ok let the agent write the letter"
demo/say.sh --hold 4 --key Enter
demo/say.sh --hold 6 "/why"
demo/say.sh --hold 4.5 --end
# when VHS has finished writing /tmp/pathfinder-demo/raw.mp4:
demo/cut.py collector/public
```

The current recording's last scene, the letter opened with `/open letter.txt`, was recorded afterwards in a fresh Pathfinder over the same folder, holding the letter the take wrote, and appended; next time add `demo/say.sh --hold 6 "/open letter.txt"` before `--end` instead, and a caption for that step.

These are the lines of the current recording. Change them to fit what Pathfinder actually says: the verdict has to match the candidate you get. `--hold S` is how long the video keeps the screen before that action, the time a viewer needs to read the last reply. `cut.py` drops the rest of the time you spent deciding (or holds the screen still if you were quicker), and keeps only the first and last moments of each wait for a reply, so `Working 25s` still shows how long it took.

`cut.py` prints where each step lands in the cut. `DEMO_GIF_FPS` and `DEMO_GIF_WIDTH` (default 6 and 820) trade the gif's size for smoothness and sharpness.
