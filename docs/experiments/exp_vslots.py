"""Symbols per segment vs reliability on a short video clip."""
import json, shutil, subprocess, sys
from pathlib import Path
import numpy as np

from vfp import pipeline
from vfp.eval.collusion_eval import attack_video
from vfp.eval.video_eval import VIDEO_ATTACKS
from vfp.video.common import VideoParams, capacity
from vfp.video.detect import detect_video
from vfp.video.ffio import probe
from vfp.codes.tardos import TardosCode
from vfp.workspace import Workspace

work = Path(sys.argv[1]); shutil.rmtree(work, ignore_errors=True); work.mkdir(parents=True)
SRC = work / "clip5.mp4"
subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", r"test-media\Video-60486.mp4",
                "-t", "5", "-c", "copy", str(SRC)], check=True)
N_USERS, TRIALS, EPS, MAXR = 8, 4, 1e-3, 1000
SINGLE = ["x264 crf34", "scale 480p + crf26", "screen-rec sim", "scale 0.5 + crf30"]
KS = [int(k) for k in sys.argv[2].split(",")] if len(sys.argv) > 2 else [64, 128, 256, 512]

print(f"{'K/0.5s':>7}{'sym/s':>7}{'symbols':>9}  {'single leaker':<44}{'bit acc':>8}  coll c=2   c=3", flush=True)
for K in KS:
    d = work / f"k{K}"
    ws = Workspace.init(d / "ws")
    names = [f"R{i:02d}" for i in range(1, N_USERS + 1)]
    for n in names:
        ws.add_receiver(n)
    a0, _ = pipeline.add_asset(ws, SRC, "a", MAXR, 3, EPS)
    params = {**a0.params, "nslots": K}
    m = capacity(probe(str(SRC)), VideoParams(**params))
    ws.db.execute("UPDATE assets SET params=?, m=? WHERE id=?", (json.dumps(params), m, a0.id)); ws.db.commit()
    asset = ws.asset(a0.id)
    copy = {n: pipeline.mark_for_receiver(ws, asset, n, d / "copies") for n in names}
    code = TardosCode.create(N_USERS, m, ws.tardos_seed(asset.id), n_colluders=3)
    word = code.codeword(0)

    ok, accs, tags = 0, [], []
    for name in SINGLE:
        pre, post, ext = VIDEO_ATTACKS[name]
        out = d / f"s_{abs(hash(name)) % 9999}.{ext}"
        subprocess.run(["ffmpeg", "-y", "-v", "error", *pre, "-i", str(copy["R01"]), "-an", *post, str(out)], check=True)
        det = detect_video(str(out), asset.orig_path, ws.layout_seed(asset.id), VideoParams(**params), m=m)
        have = det.confidence > 0
        accs.append((det.bits == word)[have].mean())
        r = pipeline.trace_leak(ws, out, "a")
        ok += r.flagged == ["R01"]
        tags.append(f"{name.split()[0][:5]}:{accs[-1]:.2f}")
    res = {2: [0, 0], 3: [0, 0]}
    for strat in ("average", "interleave 8-45 frames", "frame-mix"):
        for c in (2, 3):
            for t in range(TRIALS):
                rng = np.random.default_rng([5, c, t, len(strat)])
                guilty = [names[i] for i in rng.choice(N_USERS, size=c, replace=False)]
                out = d / "leak.mp4"
                attack_video([copy[g] for g in guilty], strat, rng, out)
                r = pipeline.trace_leak(ws, out, "a")
                good = bool(set(r.flagged) & set(guilty)) and not (set(r.flagged) - set(guilty))
                res[c][0] += 1; res[c][1] += good
    print(f"{K:>7}{K*2:>7}{m:>9}  {ok}/4 ok, {' '.join(tags):<35}{np.mean(accs):>8.3f}  "
          f"{100*res[2][1]/res[2][0]:>6.0f}%  {100*res[3][1]/res[3][0]:>5.0f}%", flush=True)
print("DONE")
