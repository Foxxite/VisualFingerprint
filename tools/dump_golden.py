"""Dump golden vectors from the Python reference code.

The Rust port (vfp-core) must reproduce every value in tests/golden/golden.json.
Run from the repo root:

    python tools/dump_golden.py

The master key below is a fixed test value. It is not a real secret.
Do not change the inputs: the Rust tests assume them. Add new cases instead.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np

from vfp.audio.common import AudioParams, auto_nslots as audio_auto_nslots, band, capacity as audio_capacity, seg_layout
from vfp.codes.keys import derive_seed
from vfp.codes.tardos import TardosCode, accusation_threshold, required_length
from vfp.image.common import ImageParams, block_dct, layout, usable_slots
from vfp.image.embed import embed_image
from vfp.perceptual import block_mask
from vfp.pipeline import supported_colluders

OUT = Path(__file__).resolve().parent.parent / "tests" / "golden" / "golden.json"
MASTER = bytes.fromhex("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff")


def h(a: np.ndarray) -> str:
    return hashlib.sha256(np.ascontiguousarray(a).tobytes()).hexdigest()


def synthetic_bgr(height: int, width: int) -> np.ndarray:
    """Deterministic test image. No RNG, so Rust can build the same pixels."""
    y, x = np.mgrid[0:height, 0:width].astype(np.int64)
    img = np.empty((height, width, 3), np.uint8)
    img[..., 0] = (x * 7 + y * 13 + ((x * y) % 31) * 5) % 256
    img[..., 1] = (x * 3 + y * 5 + ((x + y) % 17) * 11) % 256
    img[..., 2] = (x * 11 + y * 2 + ((x ^ y) % 23) * 7) % 256
    return img


def main() -> None:
    g: dict = {"master_key_hex": MASTER.hex()}

    # ---- key derivation
    g["derive_seed"] = [
        {"labels": labels, "seed": str(derive_seed(MASTER, *labels))}   # str: u64 may exceed JSON int range
        for labels in (["asset", "1", "tardos"], ["asset", "1", "layout"], ["asset", "2", "tardos"], ["x"])
    ]

    # ---- Tardos code
    tardos = []
    for seed, n_users, length, c in ((12345, 5, 64, 3), (2**63 + 99, 12, 300, 2)):
        code = TardosCode.create(n_users, length, seed, n_colluders=c)
        words = code.all_codewords()
        y = words[1].copy()                       # a leak that equals receiver 1
        conf = np.linspace(0.2, 1.0, length)
        tardos.append({
            "seed": str(seed), "n_users": n_users, "length": length, "n_colluders": c,
            "p": code.p.tolist(),
            "codewords": words.tolist(),
            "y": y.tolist(), "confidence": conf.tolist(),
            "scores_plain": code.scores(y).tolist(),
            "scores_weighted": code.scores(y, conf).tolist(),
            "threshold_plain": accusation_threshold(n_users, length, 1e-3),
            "threshold_weighted": accusation_threshold(n_users, length, 1e-3, weights=conf),
        })
    g["tardos"] = tardos
    g["required_length"] = [
        {"n": n, "c": c, "eps": e, "m": required_length(n, c, e)}
        for n, c, e in ((1000, 3, 1e-3), (10, 2, 1e-2), (1, 1, 1e-3))
    ]
    g["supported_colluders"] = [
        {"m": m, "n": n, "eps": e, "c": supported_colluders(m, n, e)}
        for m, n, e in ((2455, 1000, 1e-3), (500, 1000, 1e-3), (10000, 10, 1e-2))
    ]

    # ---- image layout, mask, DCT, full embed
    lay = []
    for nbh, nbw, seed, nslots in ((6, 8, 777, 4), (20, 30, 2**62 + 5, 50)):
        slot, w = layout(nbh, nbw, seed, nslots)
        lay.append({"nbh": nbh, "nbw": nbw, "seed": str(seed), "nslots": nslots,
                    "slot": slot.tolist(), "w_first_rows": w[:4].tolist(), "w_sha": h(w)})
    g["image_layout"] = lay
    g["usable_slots"] = [{"h": hh, "w": ww, "wanted": wd, "n": usable_slots(hh, ww, wd)}
                         for hh, ww, wd in ((480, 640, 1024), (64, 96, 256), (40, 40, 256))]

    img = synthetic_bgr(64, 96)
    from vfp.image.common import luma, pad8
    coef = block_dct(pad8(luma(img)))
    g["image_dct"] = {"h": 64, "w": 96, "coef_sha": h(coef.astype(np.float32)),
                      "coef_block00": coef[0, 0].tolist(),
                      "mask": block_mask(coef).tolist()}
    p = ImageParams(nslots=8, strength=3.0)
    sym = np.array([1, 0, 0, 1, 1, 0, 1, 0], np.uint8)
    marked = embed_image(img, sym, 4242, p)
    g["image_embed"] = {"h": 64, "w": 96, "seed": "4242", "nslots": 8, "strength": 3.0,
                        "symbols": sym.tolist(), "marked_sha": h(marked),
                        "marked_first_row": marked[0, :8].tolist(),
                        "image": img.reshape(-1).tolist(), "marked": marked.reshape(-1).tolist()}
    from vfp.image.detect import detect_image
    det = detect_image(marked, img, 4242, p)
    g["image_detect"] = {"z": det.z.tolist(), "bits": det.bits.tolist(),
                         "confidence": det.confidence.tolist(), "valid_fraction": det.valid_fraction}

    # ---- audio layout
    ap = AudioParams()
    alay = []
    for seed, seg, nframes, nbins, nslots in ((99, 0, 4, 10, 8), (2**61 + 3, 7, 21, 33, 32)):
        w, slot = seg_layout(seed, seg, nframes, nbins, nslots)
        alay.append({"seed": str(seed), "seg": seg, "nframes": nframes, "nbins": nbins, "nslots": nslots,
                     "w": w.tolist(), "slot": slot.tolist()})
    g["audio_layout"] = alay
    g["audio_misc"] = {
        "band": {str(sr): list(band(sr, ap)) for sr in (44100, 48000, 22050)},
        "capacity": [{"dur": d, "nslots": n, "m": audio_capacity(d, AudioParams(nslots=n))}
                     for d, n in ((10.0, 32), (173.4, 32), (10.0, 256))],
        "auto_nslots": [{"dur": d, "target": t, "n": audio_auto_nslots(d, ap, t)}
                        for d, t in ((10.0, 2455), (173.0, 2455), (3.0, 2455), (600.0, 2455))],
    }

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(g, indent=1))
    print(f"wrote {OUT} ({OUT.stat().st_size // 1024} KiB)")


if __name__ == "__main__":
    main()
