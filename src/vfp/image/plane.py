"""Luma-plane embed and detect primitives shared by images and video frames."""

from __future__ import annotations

import numpy as np

from ..perceptual import block_mask
from .common import NPOS, US, VS, block_dct, block_idct, layout, pad8


def grid(h: int, w: int):
    return (h + 7) // 8, (w + 7) // 8


def make_layout(h: int, w: int, seed, nslots: int):
    nbh, nbw = grid(h, w)
    return layout(nbh, nbw, seed, nslots)


def embed_plane(y: np.ndarray, sym_pm1: np.ndarray, slot, pat, strength: float) -> np.ndarray:
    """y: float32 (H,W). sym_pm1: +-1 per slot. Returns the marked float plane."""
    h, w = y.shape
    coef = block_dct(pad8(y))
    nbh, nbw = coef.shape[:2]
    alpha = (strength * block_mask(coef)).reshape(-1)
    delta = alpha[:, None] * sym_pm1.astype(np.float32)[slot][:, None] * pat
    flat = coef.reshape(-1, 8, 8).copy()
    flat[:, US, VS] += delta
    return block_idct(flat.reshape(nbh, nbw, 8, 8))[:h, :w]


def block_valid(valid: np.ndarray) -> np.ndarray:
    """Pixel valid mask (H,W) -> per-block bool (nb,). A block counts only if all pixels do."""
    h, w = valid.shape
    nbh, nbw = grid(h, w)
    v = np.pad(valid, ((0, nbh * 8 - h), (0, nbw * 8 - w)), mode="edge")
    return v.reshape(nbh, 8, nbw, 8).all(axis=(1, 3)).reshape(-1)


def plane_stats(y_orig: np.ndarray, y_leak: np.ndarray, bvalid: np.ndarray,
                slot, pat, strength: float, nslots: int):
    """Correlate (leaked - original) with the key pattern, per slot.

    Returns (num, den, sigma). Per-slot z = num / (sigma * sqrt(NPOS * den)).
    Frames add up: sum num/sigma^2 and den/sigma^2 over frames.
    """
    co, cl = block_dct(pad8(y_orig)), block_dct(pad8(y_leak))
    alpha = (strength * block_mask(co)).reshape(-1)
    D = (cl - co).reshape(-1, 8, 8)[:, US, VS]
    proj = (D * pat).sum(axis=1)
    wv = bvalid * alpha
    num = np.bincount(slot, weights=wv * proj, minlength=nslots)
    den = np.bincount(slot, weights=bvalid * alpha**2, minlength=nslots)
    dv = D[bvalid]
    sigma = 1.4826 * np.median(np.abs(dv - np.median(dv))) if dv.size else 1.0
    return num, den, max(float(sigma), 1e-3)
