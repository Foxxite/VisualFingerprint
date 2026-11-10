from __future__ import annotations

from dataclasses import dataclass
from math import ceil

import numpy as np


@dataclass(frozen=True)
class AudioParams:
    seg_seconds: float = 1.0   # one group of symbols per segment
    nslots: int = 32           # symbols per segment
    alpha: float = 0.05        # relative size of the mark (0.05 = about -26 dB under the signal)
    nfft: int = 2048
    f_lo: float = 300.0
    f_hi: float = 8000.0


def hop(p: AudioParams) -> int:
    return p.nfft // 2


def band(sr: int, p: AudioParams) -> tuple[int, int]:
    b0 = int(ceil(p.f_lo * p.nfft / sr))
    b1 = min(int(p.f_hi * p.nfft / sr), p.nfft // 2)
    return b0, b1


def frames_per_seg(sr: int, p: AudioParams) -> int:
    return max(1, round(p.seg_seconds * sr / hop(p)))


def capacity(duration: float, p: AudioParams) -> int:
    """Number of Tardos symbols (code length m) for audio of this length."""
    return (ceil(duration / p.seg_seconds) + 1) * p.nslots


def seg_layout(seed: int, seg: int, nframes: int, nbins: int, nslots: int):
    """Keyed +-1 pattern and keyed time-frequency cell -> slot map for one segment."""
    rng = np.random.default_rng([int(seed), 0xA0D10, seg])
    w = (rng.integers(0, 2, size=(nframes, nbins), dtype=np.int8) * 2 - 1).astype(np.float32)
    slot = rng.permutation(nframes * nbins).reshape(nframes, nbins) % nslots
    return w, slot
