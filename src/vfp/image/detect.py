"""Read symbols from a leaked image, using the original as reference."""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

from .common import (US, VS, NPOS, ImageParams, block_dct, layout, luma, pad8,
                     strengths)
from .register import register


@dataclass
class ImageDetection:
    z: np.ndarray            # per-slot z-score; sign = bit (+ -> 1)
    bits: np.ndarray         # hard bits
    confidence: np.ndarray   # 0..1 per slot
    valid_fraction: float    # share of blocks that overlapped the original
    info: dict


def detect_image(leaked: np.ndarray, original: np.ndarray, seed: int,
                 params: ImageParams = ImageParams()) -> ImageDetection:
    aligned, valid, info = register(leaked, original)
    H, W = original.shape[:2]

    yo = pad8(luma(original))
    yl = pad8(luma(aligned))
    co, cl = block_dct(yo), block_dct(yl)
    nbh, nbw = co.shape[:2]
    slot, pat = layout(nbh, nbw, seed, params.nslots)
    alpha = strengths(co, params)

    vpad = np.pad(valid, ((0, nbh * 8 - H), (0, nbw * 8 - W)), mode="edge")
    bvalid = vpad.reshape(nbh, 8, nbw, 8).all(axis=(1, 3)).reshape(-1)

    D = (cl - co).reshape(-1, 8, 8)[:, US, VS]                   # (nb, npos)
    proj = (D * pat).sum(axis=1)

    wgt = bvalid * alpha
    num = np.bincount(slot, weights=wgt * proj, minlength=params.nslots)
    den = np.bincount(slot, weights=bvalid * alpha**2, minlength=params.nslots)

    dv = D[bvalid]
    sigma = 1.4826 * np.median(np.abs(dv - np.median(dv))) if dv.size else 1.0
    sigma = max(float(sigma), 1e-3)
    z = num / np.sqrt(sigma**2 * NPOS * np.maximum(den, 1e-9))
    z = np.where(den > 0, z, 0.0)

    return ImageDetection(
        z=z,
        bits=(z > 0).astype(np.uint8),
        confidence=np.clip(np.abs(z) / 3.0, 0.0, 1.0),
        valid_fraction=float(bvalid.mean()),
        info=info,
    )
