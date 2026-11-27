#!/usr/bin/env python3
"""Shorten the waits in a demo recording and write the mp4 and gif.

    demo/cut.py [OUT_DIR]

Reads raw.mp4 and timeline.log from $DEMO_ROOT (default /tmp/pathfinder-demo),
both written by demo/record.sh and demo/say.sh. A reply's wait keeps its
first and last moments (you still see "Working 25s"); the time spent
deciding what to type keeps only the --hold given for that step. Typing and
replies arriving play at normal speed. Writes pathfinder-demo.mp4 and
pathfinder-demo.gif to OUT_DIR (default $DEMO_ROOT).

Captions come from $DEMO_CAPTIONS (default demo/captions.txt), one per line:
`STEP text` shows the text from when the person starts step STEP (the
STEPth say.sh action: 1 is typing `pathfinder`), `STEP+ text` from when
that step's reply has arrived, and `0 text` from the start. Each caption
stays until the next. They go in a band under the terminal, rendered with
typst, so nothing Pathfinder shows is covered.
"""
import os, shutil, subprocess, sys, tempfile

ROOT = os.path.realpath(os.environ.get("DEMO_ROOT", "/tmp/pathfinder-demo"))
OUT = sys.argv[1] if len(sys.argv) > 1 else ROOT
RAW = os.path.join(ROOT, "raw.mp4")
KEEP_BUSY_START, KEEP_BUSY_END = 1.6, 0.6  # seconds of a wait kept
GIF_FPS = int(os.environ.get("DEMO_GIF_FPS", 6))
GIF_WIDTH = int(os.environ.get("DEMO_GIF_WIDTH", 820))
CAPTIONS = os.environ.get("DEMO_CAPTIONS",
                          os.path.join(os.path.dirname(os.path.abspath(__file__)), "captions.txt"))
BAND = 54  # caption band height, px

def duration(path):
    out = subprocess.check_output(["ffprobe", "-v", "error", "-show_entries",
                                   "format=duration", "-of", "csv=p=0", path])
    return float(out)

events = []
for line in open(os.path.join(ROOT, "timeline.log")):
    parts = line.split()
    if len(parts) >= 2:
        hold = next((float(p[5:]) for p in parts[2:] if p.startswith("hold=")), None)
        events.append((float(parts[0]), parts[1], hold))
events.sort()

# Map wall time to video time on two anchors: the first character typed
# (found in the video: the first change on the prompt line right of the
# cursor) and the end (the server is told to quit 0.5 s after the end
# event, VHS sees it within a moment and records 0.3 s more). VHS's frame
# rate can lag the wall clock a little, so the scale isn't quite 1.
end = next(t for t, k, _ in events if k == "end")
first_type = next(t for t, k, _ in events if k == "type")
total = duration(RAW)

def first_typed_char():
    w, h = 1200, 720
    raw = subprocess.check_output(["ffmpeg", "-v", "error", "-t", "30", "-i", RAW, "-vf",
        f"scale={w}:{h},format=gray", "-f", "rawvideo", "-"])
    size = w * h
    # The prompt line: the top 40 px, from just right of the cursor.
    def strip(i):
        f = raw[i * size:(i + 1) * size]
        return [f[y * w + x] for y in range(18, 44, 2) for x in range(175, 500, 2)]
    base = strip(0)
    for i in range(1, len(raw) // size):
        cur = strip(i)
        if sum(abs(a - b) for a, b in zip(base, cur)) > 2000:
            return i / 25.0
    return 1.5

v_first = float(os.environ.get("DEMO_FIRST_CHAR", 0)) or first_typed_char()
v_end = total - 1.0
scale = (v_end - v_first) / (end - (first_type + 0.04))
print(f"first character at {v_first:.2f}s, video/wall scale {scale:.4f}")
def vt(t):
    return max(0.0, min(total, v_first + (t - first_type - 0.04) * scale))

cuts = []    # (start, end) in video time, removed
freezes = []  # (at, seconds): hold the frame before `at` this much longer
busy_start = None
last_idle = 0.0
for t, kind, hold in events:
    if kind == "busy":
        busy_start = vt(t)
    elif kind == "idle" and busy_start is not None:
        b0, b1 = busy_start, vt(t)
        if b1 - b0 > KEEP_BUSY_START + KEEP_BUSY_END + 0.5:
            cuts.append((b0 + KEEP_BUSY_START, b1 - KEEP_BUSY_END))
        busy_start = None
        last_idle = b1
    elif kind in ("type", "key", "end"):
        # The screen stays `hold` seconds between settling and this action,
        # shortened or held still as needed.
        h = hold if hold is not None else 3.0
        a = vt(t)
        if a - last_idle > h:
            cuts.append((last_idle + h, a))
        elif h - (a - last_idle) > 0.2:
            freezes.append((a, h - (a - last_idle)))
        if kind == "end":
            cuts.append((a + 0.4, total))
    if kind in ("send", "key"):
        last_idle = vt(t)

cuts = sorted((max(0, a), min(total, b)) for a, b in cuts if b > a)
keep, pos = [], 0.0
for a, b in cuts:
    if a > pos:
        keep.append((pos, a))
    pos = max(pos, b)
if pos < total:
    keep.append((pos, total))
# Split pieces where a freeze goes, so it can be padded on the piece's end.
pieces = []  # (start, end, pad)
for a, b in keep:
    for at, sec in sorted(freezes):
        if a < at < b:
            pieces.append((a, at, sec))
            a = at
    pieces.append((a, b, 0.0))

def out_time(v):
    """Where video time v lands in the cut."""
    t = 0.0
    for a, b, pad in pieces:
        if v < a:
            break
        t += min(v, b) - a + (pad if v >= b else 0.0)
    return t

for t, kind, _ in events:
    if kind in ("type", "key", "busy", "idle"):
        print(f"  {out_time(vt(t)):6.1f}s  {kind}")
length = sum(b - a + p for a, b, p in pieces)
print(f"raw {total:.1f}s -> {length:.1f}s in {len(pieces)} pieces")

# Captions: where each step starts and where its reply has arrived, in the cut.
steps = []  # (typed, replied) per step, in cut time
for i, (t, kind, _) in enumerate(events):
    if kind not in ("type", "key"):
        continue
    sent = t if kind == "key" else next((u for u, k, _ in events[i:] if k == "send"), t)
    replied, waited = sent + 0.3, False
    for u, k, _ in events[i + 1:]:
        if k in ("type", "key", "end"):
            break
        if k == "busy":
            waited = True
        if k == "idle" and waited:
            replied = u
            break
    steps.append((out_time(vt(t)), out_time(vt(replied))))
captions = []
if os.path.exists(CAPTIONS):
    for line in open(CAPTIONS, encoding="utf-8"):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        key, text = line.split(" ", 1)
        n = int(key.rstrip("+"))
        at = 0.0 if n == 0 else steps[n - 1][1 if key.endswith("+") else 0]
        captions.append((at, text.strip()))
captions.sort()
spans = [(a, captions[i + 1][0] if i + 1 < len(captions) else length, text)
         for i, (a, text) in enumerate(captions)]
for a, b, text in spans:
    print(f"  caption {a:6.1f}-{b:6.1f}s  {text}")

tmp = tempfile.mkdtemp(prefix="pathfinder-captions-")
pngs = []
for n, (_, _, text) in enumerate(spans):
    typ = os.path.join(tmp, f"c{n}.typ")
    esc = text.replace("\\", "\\\\").replace("\"", "\\\"")
    with open(typ, "w", encoding="utf-8") as f:
        f.write(f"""#set page(width: 1200pt, height: {BAND}pt, margin: 0pt, fill: rgb("#11111b"))
#set text(font: ("Avenir Next", "Helvetica Neue", "Arial"), size: 21pt, fill: rgb("#f5e0dc"), weight: "medium")
#align(center + horizon)[#str("{esc}")]
""")
    png = os.path.join(tmp, f"c{n}.png")
    subprocess.run(["typst", "compile", "--ppi", "72", typ, png], check=True, stderr=subprocess.DEVNULL)
    pngs.append(png)
chains = []
for n, (a, b, pad) in enumerate(pieces):
    f = f"[0:v]trim={a:.3f}:{b:.3f},setpts=PTS-STARTPTS"
    if pad:
        f += f",tpad=stop_mode=clone:stop_duration={pad:.3f}"
    chains.append(f + f"[v{n}]")
graph = ";".join(chains) + ";" + "".join(f"[v{n}]" for n in range(len(pieces))) \
    + f"concat=n={len(pieces)}:v=1:a=0"
if spans:
    graph += f",pad=iw:ih+{BAND}:0:0:color=#11111b[c0]"
    for n, (a, b, _) in enumerate(spans):
        graph += (f";[c{n}][{n + 1}:v]overlay=0:H-{BAND}:"
                  f"enable='between(t,{a:.2f},{b - 0.01:.2f})'[c{n + 1}]")
    graph += f";[c{len(spans)}]null[out]"
else:
    graph += "[out]"
inputs = ["-i", RAW]
for png in pngs:
    inputs += ["-loop", "1", "-t", f"{length + 1:.2f}", "-i", png]
mp4 = os.path.join(OUT, "pathfinder-demo.mp4")
gif = os.path.join(OUT, "pathfinder-demo.gif")
os.makedirs(OUT, exist_ok=True)
subprocess.run(["ffmpeg", "-v", "error", "-y", *inputs, "-filter_complex", graph,
                "-map", "[out]", "-an", "-c:v", "libx264", "-preset", "slow", "-crf", "26",
                "-tune", "stillimage", "-pix_fmt", "yuv420p", "-movflags", "+faststart", mp4],
               check=True)
subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", mp4, "-vf",
                f"fps={GIF_FPS},scale={GIF_WIDTH}:-1:flags=lanczos,split[a][b];"
                "[a]palettegen=max_colors=32:stats_mode=diff[p];"
                "[b][p]paletteuse=dither=none:diff_mode=rectangle", gif], check=True)
for f in (mp4, gif):
    print(f, f"{os.path.getsize(f) / 1e6:.2f} MB")
shutil.rmtree(tmp, ignore_errors=True)
