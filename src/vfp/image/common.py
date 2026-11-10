"""Shared layout and transform helpers for image embed and detect."""

from __future__ import annotations

from dataclasses import dataclass

import cv2
import numpy as np
from scipy.fft import dctn, idctn

from ..perceptual import block_mask

# Mid/low DCT positions (u, v). Low enough to survive scaling and JPEG,
# high enough to stay out of the most visible bands.
POSITIONS = [(u, v) for u in range(5) for v in range(5) if 2 <= u + v <= 5]
US = np.array([p[0] for p in POSITIONS])
VS = np.array([p[1] for p in POSITIONS])
NPOS = len(POSITIONS)


@dataclass(frozen=True)
class ImageParams:
    nslots: int = 256        # symbols carried by one image
    strength: float = 3.0    # base DCT amplitude per coefficient


MIN_BLOCKS_PER_SLOT = 12
DEFAULT_SLOTS = 1024     # symbols per image when the image is big enough


def usable_slots(height: int, width: int, wanted: int = 256) -> int:
    """Fewest-risk slot count for an image of this size.

    Each slot needs enough blocks to beat the noise. Small images get fewer slots
    (so fewer symbols and weaker collusion tracing). Warn the user when this drops.
    """
    blocks = (height // 8) * (width // 8)
    return int(max(8, min(wanted, blocks // MIN_BLOCKS_PER_SLOT)))


def luma(bgr: np.ndarray) -> np.ndarray:
    return cv2.cvtColor(bgr, cv2.COLOR_BGR2YCrCb)[..., 0].astype(np.float32)


def pad8(y: np.ndarray) -> np.ndarray:
    h, w = y.shape
    return np.pad(y, ((0, (-h) % 8), (0, (-w) % 8)), mode="edge")


def block_dct(y: np.ndarray) -> np.ndarray:
    h, w = y.shape
    b = y.reshape(h // 8, 8, w // 8, 8).transpose(0, 2, 1, 3)
    return dctn(b, axes=(2, 3), norm="ortho")


def block_idct(coef: np.ndarray) -> np.ndarray:
    b = idctn(coef, axes=(2, 3), norm="ortho")
    nbh, nbw = b.shape[:2]
    return b.transpose(0, 2, 1, 3).reshape(nbh * 8, nbw * 8)


def layout(nbh: int, nbw: int, seed: int, nslots: int):
    """Keyed block->slot map and keyed +-1 pattern per block and position."""
    nb = nbh * nbw
    parts = [int(seed)] if np.isscalar(seed) else [int(s) for s in seed]
    rng = np.random.default_rng([*parts, 0x1A6E])
    slot = rng.permutation(nb) % nslots          # spread each slot over the whole image
    w = rng.integers(0, 2, size=(nb, NPOS), dtype=np.int8) * 2 - 1
    return slot, w.astype(np.float32)


def strengths(coef_orig: np.ndarray, params: ImageParams) -> np.ndarray:
    return (params.strength * block_mask(coef_orig)).reshape(-1)
