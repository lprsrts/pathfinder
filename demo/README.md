# The demo recordings

There are two: the electricity bill (`pathfinder-demo.*`) and a scientist working through a preprint (`pathfinder-demo-research.*`, see [The research demo](#the-research-demo) below). Both are made the same way.

`collector/public/pathfinder-demo.gif` and `.mp4` show someone who isn't technical working out why their electricity bill went up: Pathfinder points to the lines in the bill, they say what they think, judge a short candidate, let the agent write a letter asking for a real meter reading, look at their record and read the letter. Captions in a band under the terminal say what is going on for viewers who don't know Pathfinder.

- `energy-bill/`: the two documents in the folder, a bill and a tariff sheet from Kestrelby Energy, an invented supplier. The PDFs are built from `energy-bill/source/*.typ` with `typst compile`.
- `record.sh [energy-bill|research]`: sets up a throwaway home under `/tmp/pathfinder-demo` (so the screen shows `~/Documents/Electricity bill`), builds the release binary, starts a shell in a private tmux server and starts VHS on `energy-bill.tape` (or `research.tape`), which records that session.
- `say.sh`: plays the person. It types a message at a steady pace, sends it, waits for the reply and prints the screen.
- `cut.py`: shortens the waits, adds the captions from `captions.txt` (or `$DEMO_CAPTIONS`) and writes the mp4, the gif and a poster frame, named after `$DEMO_NAME` (default `pathfinder-demo`; captions are rendered with `typst`).
- `captions.txt`: the captions, each tied to a step (`3` from when step 3 is typed, `3+` from when its reply has arrived).

The candidate you judge is right or flawed at random, and the plan Pathfinder makes varies, so the lines can't all be scripted ahead: you read each reply and answer it.

## Re-recording

You need `vhs`, `tmux`, `ffmpeg`, `jq`, `typst` and poppler's `pdftotext`, and a Pathfinder home with a login (the demo uses the ChatGPT plan's GPT-6-Luna and GPT-6.2-Sol; set `PATHFINDER_DEMO_FAST` and `PATHFINDER_DEMO_MAIN` for others). Only `auth.json`, `config.json` and the cached model list are copied, into the throwaway home. A sign-in refreshed during the recording is copied back, so the source home stays signed in. Nothing about your own threads is shown.

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

## The research demo

`collector/public/pathfinder-demo-research.gif` and `.mp4` show a senior scientist working through a public preprint: Ali Darijani, Jürgen Beyerer, Zahra Sadat Hajseyed Nasrollah, Luisa Hoffmann and Michael Heizmann, *Comprehensive Description of Uncertainty in Measurement for Representation and Propagation with Scalable Precision*, arXiv:2603.20365 (2026), <https://arxiv.org/abs/2603.20365>. It proposes Gaussian mixture models (GMMs) as the data type for measurement uncertainty. `record.sh research` downloads it from arXiv into `~/Research/mixture-uncertainty` in the throwaway home and makes that folder a git repository with one commit, so the agent's work runs in a staged copy and its review may be checked. The PDF is never kept in this repository.

The reasoning is the scientist's own and stays within what the paper says:

- Bayesian fusion of two mixtures pairs every component (Proposition B.3, eq. 44, p. 29): K·L components, so the count grows exponentially with the number of fusions (§6.8, p. 18).
- The L2 distance between two mixtures is closed-form (eqs. 21–23, pp. 18–19), and its pairwise term c_uv is the same constant as the normalisation of a Gaussian product (eq. 39, p. 27). Scoring a reduced mixture needs only Gaussian products, no numerical integration, so reducing after every fusion is affordable to evaluate; finding the best reduced mixture (eq. 24) is still a non-convex optimisation, which the paper doesn't claim is cheap.

The scientist asks where the paper derives these (Pathfinder cites p. 29 Proposition B.3, p. 18 on the exponential growth and the L2 formula, p. 27 Proposition A.6 and Corollary A.1), searches related work online, states the argument and is checked on it, then asks the agent for a small numerical experiment (`fusion_check.py`, plain Python: fuse random 1-D mixtures with eq. 44, check against numerical integration, count components over five fusions, score a 3-component reduction with the closed-form L2 distance). They delegate the random inputs on trust, explain the Gaussian product before the fusion code, judge that candidate, hand the fusion to the agent, catch the planted check in its review, let the agent do the rest (one part reviewed and approved, the others on trust), ask for the record and run the script.

Review checks were at their highest setting (`normal`, the Ctrl+P **Review checks** row, shown at steps 2-4; the source home's `config.json` already had `"checks": "normal"`), and a caption says so.

These are the lines of the current recording, 27 steps (the numbers are the steps `cut.py` counts, which `research-captions.txt` refers to). Long messages were typed with `DEMO_TYPE_DELAY=0.018` to `0.02`.

```sh
PATHFINDER_DEMO_CREDENTIALS=~/.local/share/pathfinder-dev demo/record.sh research
sleep 3
demo/say.sh --hold 2.5 "pathfinder"                                   #  1
demo/say.sh --hold 2.5 --key C-p                                      #  2 settings
demo/say.sh --hold 1 --key End                                        #  3 Review checks: normal
demo/say.sh --hold 2 --key Escape                                     #  4
demo/say.sh --hold 2 "Where does this paper derive the Bayesian fusion of two Gaussian mixtures, how the number of components grows, and the Gaussian product constant its L2 distance reuses?"
demo/say.sh --hold 4.5 "/web Gaussian mixture reduction closed-form L2 distance"
demo/say.sh --hold 5 "My argument: fusion pairs every component, weight πi·πj·cij, so K·L components (eq. 44) and a chain of fusions grows exponentially (§6.8). But the L2 distance (eqs. 21-23) uses the same cij as a Gaussian product (eq. 39), so scoring a reduction needs no integration. Only the fit (eq. 24) is non-convex. I think I understand it now, check me"
demo/say.sh --hold 7 "Right. cᵢⱼ = N(mᵢ; mⱼ, Σᵢ + Σⱼ) is the integral of each pairwise product, Z is the integral of the whole product so the result is a density, and K·L terms per fusion gives K^T after T fusions"
demo/say.sh --hold 2 --key Enter                                      #  9 judge it: right
demo/say.sh --hold 6 "Let the agent write fusion_check.py in plain Python: fuse two random 1-D mixtures with eq. 44, check the product against numerical integration, print the component count over five fusions and the closed-form L2 distance to a 3-component reduction, and run it"
# 11-12 (cut): Enter on "Delegate Generate seeded random 1-D mixtures ...
# without understanding it?" (the inputs, on trust), then /reject on a
# proposed map change adding parts for Gaussian densities and mixtures
# (step 11's hold, 4.5 s, is how long the plan stays on screen)
demo/say.sh --hold 4.5 "A product of two Gaussians N(x; μa, va)·N(x; μb, vb) is c·N(x; μ, v): precisions add, 1/v = 1/va + 1/vb, the mean is precision-weighted, μ = v·(μa/va + μb/vb), and c = N(μa; μb, va + vb) is their overlap integral. That c is the cᵢⱼ in eq. 44. Check me"
demo/say.sh --hold 7 "Right. v = va·vb/(va + vb), the mean weights μa by vb/s and μb by va/s, which is the precision-weighted mean, the overlap is N(μa; μb, va + vb), and z is the integral of the whole product"
demo/say.sh --hold 2 --key Enter                                      # 15 judge it: right
demo/say.sh --hold 4 "Hand it to the agent"
demo/say.sh --hold 3 --key Enter                                      # 17 the review
demo/say.sh --hold 4 --key C-u                                        # 18 the flawed hunk
# 19 (cut): C-u to the top of the diff
demo/say.sh --hold 4 "/object The overlap divides by √variance, the fused variance va·vb/(va + vb), but c_ij = N(μa; μb, va + vb) needs √variance_sum, as its log term already uses. With the smaller variance every pair weight is wrong"
demo/say.sh --hold 7 "Let the agent do the rest: the quadrature check, the count over five fusions, the closed-form L2 distance to a 3-component reduction, and run it"
demo/say.sh --hold 4 --key Enter                                      # 22 runs them; the chain part comes for review
demo/say.sh --hold 4 "/approve Each fused result is the next prior and nothing is merged, so 2 then 6, 18, 54, 162, 486"
demo/say.sh --hold 5 "So what have I actually shown here?"            # 24
# 25 (cut): C-u to the top of the record
demo/say.sh --hold 5 "/quit"
demo/say.sh --hold 2 "python3 fusion_check.py"                       # 27
demo/say.sh --hold 6 --end
# the mp4 (152 s), gif (2.8 MB) and poster in one cut:
DEMO_DROP="11-12,19,25" DEMO_TYPE_SPEED=3 DEMO_NAME=pathfinder-demo-research \
  DEMO_CAPTIONS=demo/research-captions.txt DEMO_GIF_FPS=2 DEMO_GIF_WIDTH=640 DEMO_GIF_COLORS=10 \
  demo/cut.py collector/public
```

The holds above are the ones the cut uses; most were shortened in `timeline.log` after the take (it is plain text: `hold=S` on each step's line). `DEMO_DROP` leaves steps out (the transcript above still shows what was said in them), `DEMO_TYPE_SPEED` plays typing faster, and `DEMO_GIF_COLORS` (default 32) shrinks the gif. Leave the C-u at step 18 in: it shows the scientist reading the agent's diff before objecting.

This cut still shows bugs of the build it was recorded with, which cutting can't hide: the plan and the record call concepts the scientist had just shown "unknown" and then "delegated" (Gaussian probability densities, Gaussian mixtures and their weights), only because a part resting on them was delegated; the agent's report shows raw LaTeX (`\(c_{ij}=\mathcal N(...)\)`); a heredoc command is collapsed onto one line above the review; the status bar keeps "review ready (/approve · /object)" after `/approve`; `/web` lists arXiv 2306.07309 twice (abs and html); and "Parts 2/7" counts only parts understood or approved. Re-record once these are fixed. The captions point the viewer at the last paragraph of the reply to the objection (where the catch is explained) and say what "Parts 2/7" counts.

In this take the first review was checked. The hunk shown for the fusion part scaled the mean difference by the fused component's standard deviation, `z = (left.mean - right.mean) / math.sqrt(variance)`, while the log term used `variance_sum`; the staged copy, the agent's real work, had `math.sqrt(variance_sum)`. The scientist objected with that reason, Pathfinder answered "You caught it: that hunk was a planted check, never part of the agent's work", wrote the agent's real result and went on to the next part. The record says of that part "in its review you caught the planted check and objected, so the agent's real result was written". The second review (the five-fusion chain) matched the staged copy and was approved.

As with the bill, the candidate is right or flawed at random and a review is checked at random (the **Checks** setting), so read each reply and answer it truthfully as the scientist, and renumber `research-captions.txt` to the steps you keep. In a take where a review is checked, object with the precise reason and add a caption for it; read the diff against the staged copy under the throwaway home's `pathfinder-home/staging/` to be sure what the agent really wrote. A plain verdict waits for Enter in a quick check too, and "let the agent do the rest: …" shows back every part it names in one line.
