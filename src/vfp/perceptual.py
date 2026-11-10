"""Perceptual masking: how much mark each 8x8 block can hide."""

from __future__ import annotations

import numpy as np


def block_mask(coef: np.ndarray, lo: float = 0.5, hi: float = 2.0) -> np.ndarray:
    """Per-block strength multiplier from the ORIGINAL block DCT.

    coef: (nbh, nbw, 8, 8) orthonormal DCT coefficients of luma.
    Textured blocks hide more mark. Flat and very dark blocks hide less.
    The detector calls this on the original too, so both sides agree.
    """
    dc = coef[..., 0, 0]
    ac_energy = (coef**2).sum(axis=(-1, -2)) - dc**2
    std = np.sqrt(np.maximum(ac_energy, 0.0)) / 8.0          # block pixel std
    texture = np.sqrt(std / 8.0)
    mean = dc / 8.0                                          # block pixel mean
    luma = np.clip(mean / 60.0, 0.6, 1.0)                    # dark areas: weaker
    return np.clip(texture * luma, lo, hi)
