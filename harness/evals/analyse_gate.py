"""Gate signals on labelled drafts.

    HARNESS_EVAL_DUMP=drafts.jsonl harness eval gate
    python3 -I evals/analyse_gate.py drafts.jsonl

Scores each signal (the gate's score, a key-blind reader's bits toward the
true key, compression distance) alone and combined, with combined rules
tested leave-one-case-out. This is how the gate's band (0.15 to 0.5) and
bit limit (0.2) were chosen.
"""
import json, math, sys, zlib, random
rows = [json.loads(l) for l in open(sys.argv[1])]

def C(s): return len(zlib.compress(s.encode(), 9))
def ncd(a, b):
    ca, cb, cab = C(a), C(b), C(a + " " + b)
    return (cab - min(ca, cb)) / max(ca, cb)
def bits(dist, correct):
    p_right, p_flawed, p_neither = dist
    tot = p_right + p_flawed + p_neither or 1
    q = ((p_right if correct else p_flawed) + 0.5 * p_neither) / tot
    q = min(max(q, 1e-6), 1 - 1e-6)
    return math.log2(q / 0.5)   # pointwise information toward the true key

for r in rows:
    r["bad"] = r["label"] != "safe"
    r["g"] = sum(r["gate"]) / len(r["gate"])
    r["b1"] = bits(r["reader"], r["correct"])
    r["b2"] = bits(r["reader2"], r["correct"])
    r["nk"] = ncd(r["draft"], r["key"])
    r["nc"] = ncd(r["draft"], r["candidate"])
    r["nd"] = r["nc"] - r["nk"]   # closer to the key than to the candidate -> positive

def report(name, score):
    """Best threshold with zero misses, and best overall, on all data (in-sample)."""
    s = sorted(rows, key=score)
    bad = [score(r) for r in rows if r["bad"]]; safe = [score(r) for r in rows if not r["bad"]]
    t0 = min(bad)
    fp0 = sum(x >= t0 for x in safe)
    best = min((sum(x >= t for x in safe) + sum(x < t for x in bad), t) for t in set(bad + safe))
    print(f"{name:28} zero-miss: {fp0:2} of {len(safe)} safe blocked | best total errors {best[0]:2} at {best[1]:.3f}")

print("Single signals, in-sample:")
report("gate p (mean of 2)", lambda r: r["g"] if not r["pre"] else 9)
report("reader bits (gate's model)", lambda r: r["b1"])
report("reader bits (second model)", lambda r: r["b2"])
report("NCD to candidate - to key", lambda r: r["nd"])

# Logistic regression, leave one case out.
FEATS = [("g", lambda r: r["g"]), ("b1", lambda r: r["b1"]), ("b2", lambda r: r["b2"]), ("nd", lambda r: r["nd"]), ("nk", lambda r: r["nk"])]
def fit(train, feats, l2=0.1, it=3000, lr=0.3):
    w = [0.0] * (len(feats) + 1)
    X = [[1.0] + [f(r) for _, f in feats] for r in train]; y = [1.0 if r["bad"] else 0.0 for r in train]
    # class weights: misses cost more
    cw = [5.0 if yi else 1.0 for yi in y]
    for _ in range(it):
        g = [0.0] * len(w)
        for xi, yi, c in zip(X, y, cw):
            z = sum(a * b for a, b in zip(w, xi)); p = 1 / (1 + math.exp(-max(min(z, 30), -30)))
            for j in range(len(w)): g[j] += c * (p - yi) * xi[j]
        for j in range(len(w)): w[j] -= lr * (g[j] / len(X) + (l2 * w[j] if j else 0))
    return w
def prob(w, feats, r):
    z = w[0] + sum(wj * f(r) for wj, (_, f) in zip(w[1:], feats)); return 1 / (1 + math.exp(-max(min(z, 30), -30)))

def loco(name, feats):
    cases = sorted({r["case"] for r in rows})
    fp = fn = 0; errs = []
    for c in cases:
        train = [r for r in rows if r["case"] != c]; test = [r for r in rows if r["case"] == c]
        w = fit(train, feats)
        tr = [prob(w, feats, r) for r in train if r["bad"] and not r["pre"]]
        t = min(tr) if tr else 0.5     # threshold: no training misses
        for r in test:
            blocked = r["pre"] or prob(w, feats, r) >= t
            if r["bad"] and not blocked: fn += 1; errs.append(("MISS", r))
            if not r["bad"] and blocked: fp += 1; errs.append(("OVER", r))
    print(f"{name:28} held-out: {fp:2} of {sum(not r['bad'] for r in rows)} safe blocked, {fn} of {sum(r['bad'] for r in rows)} leaking/pointed missed")
    return errs

print("\nCombined, leave-one-case-out (threshold set on the other cases with no misses):")
loco("gate only", FEATS[:1])
loco("gate + reader bits", FEATS[:2])
loco("gate + NCD", [FEATS[0], FEATS[3], FEATS[4]])
errs = loco("gate + bits + NCD", [FEATS[0], FEATS[1], FEATS[3], FEATS[4]])
loco("all", FEATS)

print("\nPer-draft signals for the hard drafts:")
for r in rows:
    if (not r["bad"] and r["g"] >= 0.25) or (r["bad"] and r["g"] < 0.5):
        print(f"  {r['label']:8} g={r['g']:.2f} bits={r['b1']:+.2f}/{r['b2']:+.2f} nd={r['nd']:+.3f}  {r['draft'][:80]}")
