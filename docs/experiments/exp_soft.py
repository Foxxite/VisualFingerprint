"""Does proper soft scoring help short clips? Collect detector outputs once, then compare scorers."""
import shutil, sys
from pathlib import Path
import numpy as np
from scipy.stats import norm

from vfp import pipeline
from vfp.audio.common import AudioParams
from vfp.audio.detect import detect_audio
from vfp.codes.tardos import TardosCode
from vfp.eval.collusion_eval import AUDIO_STRATEGIES, attack_audio
from vfp.workspace import Workspace

SRC = r"test-media\01KK7JAA5378JAJH24MEV4ERCK.mp3"
work = Path(sys.argv[1]); shutil.rmtree(work, ignore_errors=True); work.mkdir(parents=True)
N_USERS, TRIALS, EPS, MAXR = 12, 18, 1e-3, 1000

ws = Workspace.init(work / "ws")
names = [f"R{i:02d}" for i in range(1, N_USERS + 1)]
for n in names:
    ws.add_receiver(n)
asset, plan = pipeline.add_asset(ws, SRC, "a", MAXR, 3, EPS)
copy = {n: pipeline.mark_for_receiver(ws, asset, n, work / "copies") for n in names}
code = TardosCode.create(N_USERS, asset.m, ws.tardos_seed(asset.id), n_colluders=3)
X = code.all_codewords().astype(float)
p = code.p
gp, gm = np.sqrt((1 - p) / p), np.sqrt(p / (1 - p))
W = X * gp - (1 - X) * gm                       # (users, m): score weight if u = +1

trials = []
for strat in AUDIO_STRATEGIES:
    for c in (2, 3):
        for t in range(TRIALS):
            rng = np.random.default_rng([7, c, t, len(strat)])
            guilty = sorted(rng.choice(N_USERS, size=c, replace=False))
            out = work / "leak.mp3"
            attack_audio([copy[names[g]] for g in guilty], strat, rng, out)
            det = detect_audio(str(out), asset.orig_path, ws.layout_seed(asset.id),
                               AudioParams(**asset.params), m=asset.m)
            trials.append((strat, c, guilty, det.z.copy()))
print("collected", len(trials), "trials; m =", asset.m)

zthr = norm.isf(EPS / MAXR)
def run(name, ufun, tight):
    res = {2: [0, 0, 0, 0, 0], 3: [0, 0, 0, 0, 0]}      # n, ok, any, caught_sum, false
    for strat, c, guilty, z in trials:
        u = ufun(z)
        S = W @ u
        sigma = np.sqrt((u ** 2).sum()) if tight else np.sqrt(asset.m)
        acc = set(np.flatnonzero(S > zthr * sigma))
        hit, wrong = acc & set(guilty), acc - set(guilty)
        r = res[c]
        r[0] += 1; r[1] += bool(hit) and not wrong; r[2] += bool(hit); r[3] += len(hit); r[4] += bool(wrong)
    row = "  ".join(f"c={c}: ok {100*r[1]/r[0]:3.0f}%  caught {r[3]/r[0]:.2f}/{c}  false {r[4]}" for c, r in res.items())
    print(f"{name:<34}{row}")

sgn = np.sign
zs = np.concatenate([np.abs(t[3]) for t in trials])
print(f"|z| quantiles 10/50/90%: {np.quantile(zs[zs>0], [.1,.5,.9]).round(1)}\n")
run("current (clip |z|/3, thr sqrt(m))", lambda z: sgn(z) * np.clip(np.abs(z) / 3, 0, 1), False)
run("same u, exact threshold", lambda z: sgn(z) * np.clip(np.abs(z) / 3, 0, 1), True)
run("hard bits, exact threshold", lambda z: sgn(z), True)
for q in (24, 48, 96, 200, 400):
    run(f"linear clip(z/{q}), exact thr", lambda z, q=q: np.clip(z / q, -1, 1), True)
for s in (16, 32, 64, 128, 256):
    run(f"tanh(z/{s}), exact thr", lambda z, s=s: np.tanh(z / s), True)
run("raw z (no clip), exact thr", lambda z: z, True)
run("sign(z)*sqrt|z|, exact thr", lambda z: sgn(z) * np.sqrt(np.abs(z)), True)
# per-file scale: divide by the median |z| of that file
run("tanh(z/median|z|), exact thr", lambda z: np.tanh(z / max(np.median(np.abs(z[z != 0])), 1e-9)), True)
run("tanh(z/(2*median|z|)), exact thr", lambda z: np.tanh(z / (2 * max(np.median(np.abs(z[z != 0])), 1e-9))), True)

# null check: with exact threshold, innocents must exceed it no more often than promised
rng = np.random.default_rng(0)
u = np.tanh(trials[0][3] / 8)
x = (rng.random((200000, asset.m)) < p).astype(float)
S = (x * gp - (1 - x) * gm) @ u / np.sqrt((u ** 2).sum())
print("\nnull check (innocent normalised score): P(>2.33)=%.4f (want .0100)  P(>3.09)=%.5f (want .00100)  std=%.3f"
      % ((S > 2.33).mean(), (S > 3.09).mean(), S.std()))
