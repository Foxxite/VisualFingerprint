"""Symbols per second vs reliability on a short audio clip (10 s)."""
import json, shutil, subprocess, sys
from pathlib import Path
import numpy as np
from scipy.stats import norm

from vfp import pipeline
from vfp.audio.common import AudioParams, capacity
from vfp.audio.detect import detect_audio
from vfp.audio.ffio import probe_audio
from vfp.codes.tardos import TardosCode
from vfp.eval.collusion_eval import AUDIO_STRATEGIES, attack_audio
from vfp.workspace import Workspace

SRC = r"test-media\01KK7JAA5378JAJH24MEV4ERCK.mp3"
work = Path(sys.argv[1]); shutil.rmtree(work, ignore_errors=True); work.mkdir(parents=True)
N_USERS, TRIALS, EPS, MAXR = 12, 8, 1e-3, 1000
zthr = norm.isf(EPS / MAXR)
SINGLE = {
    "mp3 64k": ["-c:a", "libmp3lame", "-b:a", "64k"],
    "aac 48k mono": ["-ac", "1", "-c:a", "aac", "-b:a", "48k"],
    "16k mono + aac 32k": ["-ar", "16000", "-ac", "1", "-c:a", "aac", "-b:a", "32k"],
}

print(f"{'slots/s':>8}{'symbols':>9}  {'single leaker (3 hard attacks)':<34}{'bit acc*':>9}  collusion c=2   c=3")
for K in (32, 64, 128, 256):
    d = work / f"k{K}"
    ws = Workspace.init(d / "ws")
    names = [f"R{i:02d}" for i in range(1, N_USERS + 1)]
    for n in names:
        ws.add_receiver(n)
    a0, _ = pipeline.add_asset(ws, SRC, "a", MAXR, 3, EPS)
    params = {**a0.params, "nslots": K}
    m = capacity(probe_audio(SRC).duration, AudioParams(**params))
    ws.db.execute("UPDATE assets SET params=?, m=? WHERE id=?", (json.dumps(params), m, a0.id)); ws.db.commit()
    asset = ws.asset(a0.id)
    copy = {n: pipeline.mark_for_receiver(ws, asset, n, d / "copies") for n in names}
    code = TardosCode.create(N_USERS, m, ws.tardos_seed(asset.id), n_colluders=3)
    word = code.codeword(0)

    # single leaker under hard attacks: ok count and bit accuracy
    ok, accs = 0, []
    for name, opts in SINGLE.items():
        out = d / ("s_" + name.replace(" ", "_") + (".mp3" if "mp3" in opts[1] else ".m4a"))
        subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", str(copy["R01"]), *opts, str(out)], check=True)
        det = detect_audio(str(out), asset.orig_path, ws.layout_seed(asset.id), AudioParams(**params), m=m)
        have = det.confidence > 0
        accs.append((det.bits == word)[have].mean())
        r = pipeline.trace_leak(ws, out, "a")
        ok += r.flagged == ["R01"]

    res = {2: [0, 0], 3: [0, 0]}
    for strat in AUDIO_STRATEGIES:
        for c in (2, 3):
            for t in range(TRIALS):
                rng = np.random.default_rng([5, c, t, len(strat)])
                guilty = [names[i] for i in rng.choice(N_USERS, size=c, replace=False)]
                out = d / "leak.mp3"
                attack_audio([copy[g] for g in guilty], strat, rng, out)
                r = pipeline.trace_leak(ws, out, "a")
                good = bool(set(r.flagged) & set(guilty)) and not (set(r.flagged) - set(guilty))
                res[c][0] += 1
                res[c][1] += good
    print(f"{K:>8}{m:>9}  {ok}/3 correct, acc {', '.join(f'{x:.2f}' for x in accs):<22}"
          f"{np.mean(accs):>9.3f}  {100*res[2][1]/res[2][0]:>5.0f}%  {100*res[3][1]/res[3][0]:>5.0f}%", flush=True)
print("DONE")
