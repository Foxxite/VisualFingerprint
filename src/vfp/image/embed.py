"""Embed one receiver's symbols into an image."""

from __future__ import annotations

import cv2
import numpy as np

from .common import (US, VS, ImageParams, block_dct, block_idct, layout, luma,
                     pad8, strengths)


def embed_image(bgr: np.ndarray, symbols: np.ndarray, seed: int,
                params: ImageParams = ImageParams()) -> np.ndarray:
    """symbols: array of nslots bits. Returns a marked uint8 BGR image."""
    symbols = np.asarray(symbols)
    if len(symbols) != params.nslots:
        raise ValueError(f"need {params.nslots} symbols, got {len(symbols)}")
    h, w = bgr.shape[:2]
    y = pad8(luma(bgr))
    coef = block_dct(y)
    nbh, nbw = coef.shape[:2]
    slot, pat = layout(nbh, nbw, seed, params.nslots)
    alpha = strengths(coef, params)
    sign = (2 * symbols.astype(np.float32) - 1)[slot]
    delta = alpha[:, None] * sign[:, None] * pat                  # (nb, npos)

    flat = coef.reshape(-1, 8, 8).copy()
    flat[:, US, VS] += delta
    y2 = block_idct(flat.reshape(nbh, nbw, 8, 8))[:h, :w]

    ycc = cv2.cvtColor(bgr, cv2.COLOR_BGR2YCrCb)
    ycc[..., 0] = np.clip(np.rint(y2), 0, 255).astype(np.uint8)
    return cv2.cvtColor(ycc, cv2.COLOR_YCrCb2BGR)
