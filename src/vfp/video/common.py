from __future__ import annotations

from dataclasses import dataclass
from math import ceil

from .ffio import VideoInfo


@dataclass(frozen=True)
class VideoParams:
    seg_seconds: float = 0.5   # one group of symbols per segment
    nslots: int = 64           # symbols per segment (spatial slots)
    strength: float = 3.0
    crf: int = 16              # x264 quality of the marked copy
    preset: str = "medium"
    frame_stride: int = 1      # detect: use every n-th frame


def seg_frames(info: VideoInfo, p: VideoParams) -> int:
    return max(1, round(p.seg_seconds * float(info.fps)))


def capacity(info: VideoInfo, p: VideoParams, max_seconds: float | None = None) -> int:
    """Number of Tardos symbols (code length m) this video can carry."""
    n = info.nframes
    if max_seconds:
        n = min(n, int(max_seconds * float(info.fps)) + 1)
    nseg = ceil(n / seg_frames(info, p)) + 1      # +1 absorbs frame-count estimate errors
    return nseg * p.nslots


CANON_AREA = 1280 * 720


def canon_size(width: int, height: int) -> tuple[int, int]:
    """Working size of the mark. Big frames are marked at about 720p and scaled up,
    so a later big downscale still meets a coarse mark."""
    s = min(1.0, (CANON_AREA / (width * height)) ** 0.5)
    if s > 0.97:
        return width, height
    return max(16, round(width * s)), max(16, round(height * s))


def effective_strength(width: int, height: int, p: VideoParams) -> float:
    """A coarser mark is more visible at equal amplitude, so big frames get less."""
    cw, _ = canon_size(width, height)
    return p.strength * (cw / width) ** 0.5


def segment_seed(seed: int, seg: int):
    return (int(seed), 0x7E6, seg)
