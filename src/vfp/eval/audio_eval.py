"""Mark an audio file for one receiver, attack it with FFmpeg, trace it, print a table.

Usage: python -m vfp.eval.audio_eval FILE [--seconds 30] [--alpha 0.05] [--work DIR]
"""

from __future__ import annotations

import argparse
import re
import subprocess
import tempfile
import time
from pathlib import Path

import numpy as np

from ..audio.common import AudioParams, capacity
from ..audio.detect import detect_audio
from ..audio.embed import embed_audio
from ..audio.ffio import decode_audio, probe_audio
from ..codes.keys import derive_seed
from ..codes.tardos import TardosCode, accusation_threshold

MP3 = lambda b: ["-c:a", "libmp3lame", "-b:a", b]
AAC = lambda b: ["-c:a", "aac", "-b:a", b]

# name -> (input options, output options, extension)
AUDIO_ATTACKS = {
    "none (marked wav)": ([], ["-c:a", "pcm_s16le"], "wav"),
    "mp3 192k": ([], MP3("192k"), "mp3"),
    "mp3 128k": ([], MP3("128k"), "mp3"),
    "mp3 64k": ([], MP3("64k"), "mp3"),
    "aac 96k": ([], AAC("96k"), "m4a"),
    "aac 48k mono": ([], ["-ac", "1"] + AAC("48k"), "m4a"),
    "opus 64k": ([], ["-c:a", "libopus", "-b:a", "64k"], "ogg"),
    "resample 22k + mp3 96k": ([], ["-ar", "22050"] + MP3("96k"), "mp3"),
    "resample 16k mono + aac 32k": ([], ["-ar", "16000", "-ac", "1"] + AAC("32k"), "m4a"),
    "volume -8 dB + mp3 128k": ([], ["-af", "volume=-8dB"] + MP3("128k"), "mp3"),
    "lowpass 6k + mp3 128k": ([], ["-af", "lowpass=f=6000"] + MP3("128k"), "mp3"),
    "compressor+EQ + mp3 128k": ([], ["-af", "acompressor=threshold=-24dB:ratio=4,"
                                              "equalizer=f=3000:t=q:w=1:g=4"] + MP3("128k"), "mp3"),
    "echo/reverb + mp3 128k": ([], ["-af", "aecho=0.8:0.6:30|50:0.3|0.2"] + MP3("128k"), "mp3"),
    "trim start 3.7s + mp3 128k": (["-ss", "3.7"], MP3("128k"), "mp3"),
    "speed x1.03 (known weak)": ([], ["-af", "atempo=1.03"] + MP3("128k"), "mp3"),
}


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-y", "-v", "error", *map(str, args)], check=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("audio")
    ap.add_argument("--seconds", type=float, default=30.0)
    ap.add_argument("--alpha", type=float, default=0.05)
    ap.add_argument("--nslots", type=int, default=32)
    ap.add_argument("--users", type=int, default=50)
    ap.add_argument("--user", type=int, default=7)
    ap.add_argument("--work", default=None)
    ap.add_argument("--only", default=None)
    a = ap.parse_args()

    work = Path(a.work or tempfile.mkdtemp(prefix="vfp_aeval_"))
    work.mkdir(parents=True, exist_ok=True)
    p = AudioParams(alpha=a.alpha, nslots=a.nslots)
    info = probe_audio(a.audio)
    dur = min(info.duration, a.seconds)
    m = capacity(dur, p)
    master = b"eval-master-key-0123456789abcdef"
    code = TardosCode.create(a.users, m, derive_seed(master, "tardos-audio"), n_colluders=3)
    seed = derive_seed(master, "audio-layout")
    word = code.codeword(a.user)
    thr = accusation_threshold(a.users, m, 1e-3)

    print(f"{Path(a.audio).name}: {info.sr} Hz, {info.channels} ch, marking {dur:.1f}s -> {m} symbols; "
          f"accuse threshold {thr:.1f}")
    marked = str(work / "marked.wav")
    t = time.time()
    embed_audio(a.audio, marked, word, seed, p, max_seconds=a.seconds)
    print(f"embed {time.time()-t:.1f}s")

    x = decode_audio(a.audio, max_seconds=a.seconds)
    y = decode_audio(marked)[:, : x.shape[1]]
    n = min(x.shape[1], y.shape[1])
    err = y[:, :n] - x[:, :n]
    snr = 10 * np.log10(np.sum(x[:, :n] ** 2) / np.sum(err**2))
    print(f"SNR of mark: {snr:.1f} dB   peak change: {np.abs(err).max():.4f}\n")

    print(f"{'attack':<30}{'bit acc':>8}{'cover':>7}{'true':>9}{'best other':>11}{'rank':>6}  result  align/offset")
    for name, (pre, post, ext) in AUDIO_ATTACKS.items():
        if a.only and not any(s.strip().lower() in name.lower() for s in a.only.split(",")):
            continue
        out = str(work / f"atk_{re.sub('[^a-z0-9]+', '_', name.lower())}.{ext}")
        ffmpeg(*pre, "-i", marked, *post, out)
        det = detect_audio(out, a.audio, seed, p, m=m)
        s = code.scores(det.bits, det.confidence)
        order = np.argsort(-s)
        rank = int(np.where(order == a.user)[0][0]) + 1
        other = float(np.max(np.delete(s, a.user)))
        have = det.confidence > 0
        acc = (det.bits == word)[have].mean() if have.any() else float("nan")
        ok = "CAUGHT" if s[a.user] > thr and rank == 1 else "miss"
        print(f"{name:<30}{acc:>8.3f}{det.coverage:>7.2f}{s[a.user]:>9.1f}{other:>11.1f}{rank:>6}  "
              f"{ok:<6}  c={det.info['align']:.2f} off={det.info['offset']}")


if __name__ == "__main__":
    main()
