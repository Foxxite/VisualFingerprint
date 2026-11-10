"""Embed one receiver's symbols into a video."""

from __future__ import annotations

from typing import Callable

import cv2
import numpy as np

from ..image.plane import embed_plane, make_layout
from .common import (VideoParams, canon_size, capacity, effective_strength, seg_frames,
                     segment_seed)
from .ffio import Encoder, decode_yuv, probe


def embed_video(src: str, dst: str, symbols: np.ndarray, seed: int,
                params: VideoParams = VideoParams(), max_seconds: float | None = None,
                progress: Callable[[int, int], None] | None = None) -> dict:
    """Write a marked copy of `src` to `dst`. symbols: bits, length >= capacity()."""
    info = probe(src)
    need = capacity(info, params, max_seconds)
    symbols = np.asarray(symbols)
    if len(symbols) < need:
        raise ValueError(f"need {need} symbols for this video, got {len(symbols)}")
    sf, K = seg_frames(info, params), params.nslots
    W, H = info.width, info.height
    cw, ch = canon_size(W, H)
    scaled = (cw, ch) != (W, H)
    strength = effective_strength(W, H, params)
    total = info.nframes if not max_seconds else min(info.nframes, int(max_seconds * float(info.fps)))

    cur_seg, slot, pat, sym = -1, None, None, None
    n = 0
    with Encoder(dst, info, params.crf, params.preset, audio_from=src, max_seconds=max_seconds) as enc:
        for y, u, v in decode_yuv(info, max_seconds):
            seg = n // sf
            if seg != cur_seg:
                cur_seg = seg
                slot, pat = make_layout(ch, cw, segment_seed(seed, seg), K)
                sym = 2.0 * symbols[seg * K:(seg + 1) * K].astype(np.float32) - 1.0
            if scaled:
                yc = cv2.resize(y, (cw, ch), interpolation=cv2.INTER_AREA).astype(np.float32)
                delta = embed_plane(yc, sym, slot, pat, strength) - yc
                y2 = y.astype(np.float32) + cv2.resize(delta, (W, H), interpolation=cv2.INTER_CUBIC)
            else:
                y2 = embed_plane(y.astype(np.float32), sym, slot, pat, strength)
            enc.write(np.clip(np.rint(y2), 0, 255).astype(np.uint8), u, v)
            n += 1
            if progress and n % 10 == 0:
                progress(n, total)
    return {"frames": n, "symbols": need, "segments": cur_seg + 1, "canon": (cw, ch)}
