"""Mark a video for one receiver, attack it with FFmpeg, trace it, print a table.

Usage: python -m vfp.eval.video_eval VIDEO [--seconds 8] [--strength 3.0] [--work DIR]
                                     [--only "crf23,scale 480p"]
"""

from __future__ import annotations

import argparse
import re
import subprocess
import tempfile
import time
from pathlib import Path

import numpy as np

from ..codes.keys import derive_seed
from ..codes.tardos import TardosCode, accusation_threshold
from ..video.common import VideoParams, capacity
from ..video.detect import detect_video
from ..video.embed import embed_video
from ..video.ffio import probe

X264 = ["-c:v", "libx264", "-preset", "fast"]

# name -> (input options, output options, extension)
VIDEO_ATTACKS = {
    "none (marked copy)": ([], ["-c:v", "copy"], "mp4"),
    "x264 crf23": ([], X264 + ["-crf", "23"], "mp4"),
    "x264 crf28": ([], X264 + ["-crf", "28"], "mp4"),
    "x264 crf34": ([], X264 + ["-crf", "34"], "mp4"),
    "x265 crf28": ([], ["-c:v", "libx265", "-preset", "fast", "-crf", "28"], "mp4"),
    "vp9 crf38": ([], ["-c:v", "libvpx-vp9", "-crf", "38", "-b:v", "0", "-deadline", "realtime",
                       "-cpu-used", "8"], "webm"),
    "scale 480p + crf26": ([], ["-vf", "scale=-2:480"] + X264 + ["-crf", "26"], "mp4"),
    "scale 0.5 + crf30": ([], ["-vf", "scale=iw/2:ih/2"] + X264 + ["-crf", "30"], "mp4"),
    "crop 90% + crf26": ([], ["-vf", "crop=iw*0.9:ih*0.9"] + X264 + ["-crf", "26"], "mp4"),
    "fps 15 + crf26": ([], ["-vf", "fps=15"] + X264 + ["-crf", "26"], "mp4"),
    "blur + crf26": ([], ["-vf", "gblur=sigma=1"] + X264 + ["-crf", "26"], "mp4"),
    "noise + crf26": ([], ["-vf", "noise=alls=8:allf=t"] + X264 + ["-crf", "26"], "mp4"),
    "bright/contrast + crf26": ([], ["-vf", "eq=contrast=1.1:brightness=-0.04"] + X264 + ["-crf", "26"], "mp4"),
    "trim start 1.3s + crf26": (["-ss", "1.3"], X264 + ["-crf", "26"], "mp4"),
    "screen-rec sim": ([], ["-vf", "rotate=0.02:c=black:ow=iw:oh=ih,scale=iw*0.8:ih*0.8"] + X264 + ["-crf", "30"], "mp4"),
}


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-y", "-v", "error", *map(str, args)], check=True)


def vmaf(distorted: str, reference: str, seconds: float) -> float | None:
    r = subprocess.run(["ffmpeg", "-v", "info", "-t", str(seconds), "-i", distorted,
                        "-t", str(seconds), "-i", reference, "-lavfi",
                        "[0:v][1:v]libvmaf=n_threads=8", "-f", "null", "-"],
                       capture_output=True, text=True)
    m = re.search(r"VMAF score:\s*([\d.]+)", r.stderr)
    return float(m.group(1)) if m else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("video")
    ap.add_argument("--seconds", type=float, default=8.0)
    ap.add_argument("--strength", type=float, default=3.0)
    ap.add_argument("--nslots", type=int, default=64)
    ap.add_argument("--users", type=int, default=50)
    ap.add_argument("--user", type=int, default=7)
    ap.add_argument("--work", default=None)
    ap.add_argument("--only", default=None, help="comma list of substrings of attack names")
    ap.add_argument("--no-quality", action="store_true")
    a = ap.parse_args()

    work = Path(a.work or tempfile.mkdtemp(prefix="vfp_eval_"))
    work.mkdir(parents=True, exist_ok=True)
    params = VideoParams(nslots=a.nslots, strength=a.strength)
    info = probe(a.video)
    m = capacity(info, params, a.seconds)
    master = b"eval-master-key-0123456789abcdef"
    code = TardosCode.create(a.users, m, derive_seed(master, "tardos-video"), n_colluders=3)
    seed = derive_seed(master, "video-layout")
    word = code.codeword(a.user)
    thr = accusation_threshold(a.users, m, 1e-3)

    print(f"{Path(a.video).name}: {info.width}x{info.height} @ {float(info.fps):.2f} fps, "
          f"marking first {a.seconds}s -> {m} symbols; accuse threshold {thr:.1f}")
    marked = str(work / "marked.mp4")
    t = time.time()
    r = embed_video(a.video, marked, word, seed, params, max_seconds=a.seconds)
    dt = time.time() - t
    print(f"embed: {r['frames']} frames in {dt:.1f}s ({r['frames']/dt:.1f} fps)")

    if not a.no_quality:
        base = str(work / "baseline.mp4")
        embed_video(a.video, base, word, seed, VideoParams(nslots=a.nslots, strength=0.0),
                    max_seconds=a.seconds)
        vm, vb = vmaf(marked, a.video, a.seconds), vmaf(base, a.video, a.seconds)
        print(f"VMAF vs original: marked {vm}  | re-encode with no mark {vb}  "
              f"(cost of the mark: {None if vm is None or vb is None else round(vb - vm, 2)})")

    print(f"\n{'attack':<26}{'bit acc':>8}{'cover':>7}{'true':>9}{'best other':>11}{'rank':>6}  result  sync/warp")
    for name, (pre, post, ext) in VIDEO_ATTACKS.items():
        if a.only and not any(s.strip().lower() in name.lower() for s in a.only.split(",")):
            continue
        out = str(work / f"atk_{re.sub('[^a-z0-9]+', '_', name.lower())}.{ext}")
        ffmpeg(*pre, "-i", marked, "-an", *post, out)
        det = detect_video(out, a.video, seed, params, m=m)
        s = code.scores(det.bits, det.confidence)
        order = np.argsort(-s)
        rank = int(np.where(order == a.user)[0][0]) + 1
        other = float(np.max(np.delete(s, a.user)))
        ok = "CAUGHT" if s[a.user] > thr and rank == 1 else "miss"
        print(f"{name:<26}{(det.bits == word)[det.confidence > 0].mean():>8.3f}{det.coverage:>7.2f}"
              f"{s[a.user]:>9.1f}{other:>11.1f}{rank:>6}  {ok:<6}  "
              f"tau={det.info['tau']} E={det.info['sync_energy']:.0f} {det.info['warp']}")


if __name__ == "__main__":
    main()
