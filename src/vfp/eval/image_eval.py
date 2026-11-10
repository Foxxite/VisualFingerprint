"""Mark an image for one receiver, attack it, trace it, print a table.

Usage: python -m vfp.eval.image_eval [path-to-image] [--strength 3.0]
Without a path a synthetic natural-looking test image is used.
"""

from __future__ import annotations

import argparse
import time

import cv2
import numpy as np

from ..codes.keys import derive_seed
from ..codes.tardos import TardosCode, accusation_threshold
from ..image.common import ImageParams, usable_slots
from ..image.detect import detect_image
from ..image.embed import embed_image
from .attacks import IMAGE_ATTACKS
from .metrics import psnr, ssim


def synthetic_image(w=1280, h=720, seed=3) -> np.ndarray:
    """1/f noise texture + gradients + sharp shapes. Not a photo, but similar statistics."""
    rng = np.random.default_rng(seed)
    fy = np.fft.fftfreq(h)[:, None]
    fx = np.fft.fftfreq(w)[None, :]
    f = np.sqrt(fx**2 + fy**2)
    f[0, 0] = 1
    chans = []
    for _ in range(3):
        spec = (rng.normal(size=(h, w)) + 1j * rng.normal(size=(h, w))) / f**1.3
        c = np.real(np.fft.ifft2(spec))
        chans.append((c - c.mean()) / c.std())
    base = np.stack(chans, -1)
    mix = 0.7 * base[..., :1] + 0.3 * base
    grad = np.linspace(-1, 1, w)[None, :, None] * 0.8
    im = 128 + 38 * mix + 30 * grad
    img = np.clip(im, 0, 255).astype(np.uint8)
    cv2.rectangle(img, (w // 8, h // 5), (w // 3, h // 2), (40, 90, 200), -1)
    cv2.circle(img, (int(w * 0.7), int(h * 0.55)), h // 5, (200, 180, 60), -1)
    cv2.line(img, (0, h - 80), (w, h - 200), (20, 20, 20), 6)
    img = np.clip(img.astype(np.float32) + rng.normal(0, 1.5, img.shape), 0, 255).astype(np.uint8)
    return img


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("image", nargs="?")
    ap.add_argument("--strength", type=float, default=3.0)
    ap.add_argument("--nslots", type=int, default=256)
    ap.add_argument("--users", type=int, default=50)
    ap.add_argument("--user", type=int, default=7)
    a = ap.parse_args()

    img = cv2.imread(a.image) if a.image else synthetic_image()
    if img is None:
        raise SystemExit("cannot read image")
    slots = usable_slots(*img.shape[:2], wanted=a.nslots)
    if slots < a.nslots:
        print(f"WARNING: image is small, using {slots} slots instead of {a.nslots}. "
              f"Tracing is weaker.")
    a.nslots = slots
    params = ImageParams(nslots=a.nslots, strength=a.strength)
    master = b"eval-master-key-0123456789abcdef"
    code = TardosCode.create(a.users, a.nslots, derive_seed(master, "tardos"), n_colluders=1)
    seed = derive_seed(master, "image-layout")
    word = code.codeword(a.user)
    z_thr = accusation_threshold(a.users, a.nslots, 1e-3)

    t = time.time()
    marked = embed_image(img, word, seed, params)
    print(f"image {img.shape[1]}x{img.shape[0]}  embed {time.time()-t:.2f}s  "
          f"PSNR {psnr(img, marked):.2f} dB  SSIM {ssim(img, marked):.4f}  "
          f"maxdiff {int(np.abs(img.astype(int)-marked.astype(int)).max())}")
    print(f"threshold (false-accuse 1e-3): {z_thr:.1f}\n")
    print(f"{'attack':<24}{'bit acc':>8}{'true score':>12}{'best other':>12}{'rank':>6}  result  (register)")

    for name, atk in IMAGE_ATTACKS.items():
        leaked = atk(marked)
        det = detect_image(leaked, img, seed, params)
        s = code.scores(det.bits, det.confidence)
        order = np.argsort(-s)
        rank = int(np.where(order == a.user)[0][0]) + 1
        other = np.max(np.delete(s, a.user))
        ok = "CAUGHT" if s[a.user] > z_thr and rank == 1 else "miss"
        print(f"{name:<24}{(det.bits == word).mean():>8.3f}{s[a.user]:>12.1f}{other:>12.1f}"
              f"{rank:>6}  {ok:<6}  {det.info.get('method')}")


if __name__ == "__main__":
    main()
