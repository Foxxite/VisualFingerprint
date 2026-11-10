"""Collusion test on real media.

Marks copies for N receivers, lets random groups of colluders combine their copies,
then traces the result through the normal pipeline.

Success = at least one colluder is accused AND no innocent receiver is accused.

Usage: python -m vfp.eval.collusion_eval {image|audio|video} FILE [--receivers 12]
                                         [--trials 6] [--sizes 2,3] [--work DIR]
"""

from __future__ import annotations

import argparse
import shutil
import tempfile
import time
from pathlib import Path

import cv2
import numpy as np

from .. import pipeline
from ..audio.ffio import decode_audio, encode_audio, probe_audio
from ..video.ffio import Encoder, decode_yuv, probe
from ..workspace import Workspace


# ------------------------------------------------------------------ image
def img_pixel_pick(arrs, rng):
    pick = rng.integers(0, len(arrs), size=arrs[0].shape[:2])
    st = np.stack(arrs)
    return np.take_along_axis(st, pick[None, ..., None], 0)[0]


def img_tiles(arrs, rng, tile=32):
    out = arrs[0].copy()
    h, w = out.shape[:2]
    for y in range(0, h, tile):
        for x in range(0, w, tile):
            out[y:y + tile, x:x + tile] = arrs[rng.integers(0, len(arrs))][y:y + tile, x:x + tile]
    return out


IMAGE_STRATEGIES = {
    "average": lambda a, r: np.mean(a, axis=0),
    "min": lambda a, r: np.min(a, axis=0),
    "max": lambda a, r: np.max(a, axis=0),
    "median": lambda a, r: np.median(a, axis=0),
    "pixel-mix": img_pixel_pick,
    "tile-mix 32px": img_tiles,
}


def attack_image(paths, strategy, rng, out: Path):
    arrs = [pipeline.read_image(p) for p in paths]
    mixed = np.clip(np.rint(IMAGE_STRATEGIES[strategy](arrs, rng)), 0, 255).astype(np.uint8)
    # the colluders then save it as an ordinary JPEG
    pipeline.write_image(out, mixed)
    return out


# ------------------------------------------------------------------ audio
def aud_interleave(xs, sr, rng):
    n = xs[0].shape[1]
    out = np.zeros_like(xs[0])
    pos, fade = 0, int(0.02 * sr)
    prev = None
    while pos < n:
        ln = int(rng.uniform(0.25, 3.0) * sr)
        k = int(rng.integers(0, len(xs)))
        seg = xs[k][:, pos:pos + ln].copy()
        if prev is not None and seg.shape[1] > fade:                     # short crossfade
            ramp = np.linspace(0, 1, fade, dtype=np.float32)
            seg[:, :fade] = prev * (1 - ramp) + seg[:, :fade] * ramp
        out[:, pos:pos + seg.shape[1]] = seg
        prev = xs[k][:, pos + ln: pos + ln + fade] if pos + ln + fade <= n else None
        if prev is not None and prev.shape[1] < fade:
            prev = None
        pos += ln
    return out


def aud_min_abs(xs, sr, rng):
    st = np.stack(xs)
    idx = np.argmin(np.abs(st), axis=0)
    return np.take_along_axis(st, idx[None], 0)[0]


AUDIO_STRATEGIES = {
    "average": lambda xs, sr, r: np.mean(xs, axis=0),
    "interleave 0.25-3s": aud_interleave,
    "min-abs": aud_min_abs,
}


def attack_audio(paths, strategy, rng, out: Path):
    sr = probe_audio(str(paths[0])).sr
    xs = [decode_audio(str(p)) for p in paths]
    n = min(x.shape[1] for x in xs)
    xs = [x[:, :n] for x in xs]
    mixed = AUDIO_STRATEGIES[strategy](xs, sr, rng).astype(np.float32)
    encode_audio(mixed, sr, str(out))                                   # colluders save as 128k mp3
    return out


# ------------------------------------------------------------------ video
def attack_video(paths, strategy, rng, out: Path):
    info = probe(str(paths[0]))
    gens = [decode_yuv(probe(str(p))) for p in paths]
    c = len(paths)
    schedule = None
    with Encoder(str(out), info, crf=23, preset="fast") as enc:
        n = 0
        cur, left = 0, 0
        for frames in zip(*gens):
            if strategy == "average":
                planes = [np.mean([f[i].astype(np.float32) for f in frames], axis=0) for i in range(3)]
                enc.write(*[np.clip(np.rint(p), 0, 255).astype(np.uint8) for p in planes])
            else:
                if strategy == "frame-mix":
                    cur = int(rng.integers(0, c))
                elif left <= 0:                                         # interleave: random chunks
                    cur, left = int(rng.integers(0, c)), int(rng.integers(8, 45))
                left -= 1
                enc.write(*frames[cur])
            n += 1
    return out


VIDEO_STRATEGIES = ["average", "interleave 8-45 frames", "frame-mix"]
KINDS = {
    "image": (IMAGE_STRATEGIES, attack_image, ".jpg"),
    "audio": (AUDIO_STRATEGIES, attack_audio, ".mp3"),
    "video": ({k: None for k in VIDEO_STRATEGIES}, attack_video, ".mp4"),
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("kind", choices=KINDS)
    ap.add_argument("file")
    ap.add_argument("--receivers", type=int, default=12)
    ap.add_argument("--trials", type=int, default=6)
    ap.add_argument("--sizes", default="2,3")
    ap.add_argument("--strategies", default=None, help="comma list of substrings")
    ap.add_argument("--max-receivers", type=int, default=1000)
    ap.add_argument("--eps", type=float, default=1e-3)
    ap.add_argument("--work", default=None)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--slots", type=int, default=None, help="image only: symbols per image (default 256)")
    a = ap.parse_args()

    work = Path(a.work or tempfile.mkdtemp(prefix="vfp_coll_"))
    for sub in ("ws", "copies"):                      # start clean: these are this script's own folders
        shutil.rmtree(work / sub, ignore_errors=True)
    (work / "ws").mkdir(parents=True, exist_ok=True)
    ws = Workspace.init(work / "ws")
    names = [f"R{i:02d}" for i in range(1, a.receivers + 1)]
    for n in names:
        ws.add_receiver(n)
    asset, plan = pipeline.add_asset(ws, a.file, "asset", a.max_receivers, 3, a.eps, slots=a.slots)
    print(f"{Path(a.file).name}: {plan.kind}, {plan.m} symbols, guaranteed colluder group size "
          f"{plan.guaranteed_colluders or 'none (single leaker only)'} "
          f"(for {a.max_receivers} receivers, false-accusation {a.eps:g})")

    t = time.time()
    copies_dir = work / "copies"
    copy = {n: pipeline.mark_for_receiver(ws, asset, n, copies_dir) for n in names}
    print(f"made {len(copy)} marked copies in {time.time()-t:.0f}s\n")

    strategies, attack, ext = KINDS[a.kind]
    if a.strategies:
        wanted = [s.strip().lower() for s in a.strategies.split(",")]
        strategies = {k: v for k, v in strategies.items() if any(w in k.lower() for w in wanted)}
    sizes = [int(s) for s in a.sizes.split(",")]

    print(f"{'strategy':<24}{'group':>6}{'trials':>7}{'>=1 caught':>11}{'all caught':>11}{'avg caught':>11}{'false acc':>10}{'success':>9}")
    total = {"ok": 0, "n": 0, "fa": 0}
    for strat in strategies:
        for c in sizes:
            ok = caught_any = caught_all = fa = 0
            caught_sum = 0
            for tr in range(a.trials):
                rng = np.random.default_rng([a.seed, c, tr, abs(hash(strat)) % 1000])
                guilty = list(rng.choice(names, size=c, replace=False))
                out = work / f"leak_{strat[:6].replace(' ', '')}_{c}_{tr}{ext}"
                attack([copy[g] for g in guilty], strat, rng, out)
                r = pipeline.trace_leak(ws, out, "asset")
                hit = [n for n in r.flagged if n in guilty]
                wrong = [n for n in r.flagged if n not in guilty]
                caught_any += bool(hit)
                caught_all += len(hit) == c
                caught_sum += len(hit)
                fa += bool(wrong)
                ok += bool(hit) and not wrong
            n = a.trials
            print(f"{strat:<24}{c:>6}{n:>7}{caught_any:>8}/{n:<2}{caught_all:>8}/{n:<2}{caught_sum/n:>11.2f}"
                  f"{fa:>7}/{n:<2}{100*ok/n:>8.0f}%", flush=True)
            total["ok"] += ok
            total["n"] += n
            total["fa"] += fa
    print(f"\noverall: success {total['ok']}/{total['n']}, trials with a false accusation: {total['fa']}")


if __name__ == "__main__":
    main()
