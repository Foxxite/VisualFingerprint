"""Read symbols from a leaked video, using the original as reference."""

from __future__ import annotations

from dataclasses import dataclass
from itertools import chain, islice
from typing import Callable

import cv2
import numpy as np

from ..image.common import NPOS
from ..image.plane import block_valid, make_layout, plane_stats
from ..image.register import apply_warp, estimate_warp
from .common import (VideoParams, canon_size, capacity, effective_strength, seg_frames,
                     segment_seed)
from .ffio import decode_thumbs, decode_yuv, probe
from .sync import find_offsets

HEAD = 60                      # leaked frames buffered for sync and warp estimation
SAMPLE = tuple(range(0, 60, 2))   # leaked frames used to confirm the time offset.
# Spread over several segments, so a wrong offset lands some frames in the wrong segment.


@dataclass
class VideoDetection:
    z: np.ndarray            # per-symbol z-score; sign = bit
    bits: np.ndarray
    confidence: np.ndarray
    coverage: float          # share of symbols that had any evidence
    info: dict


def _canon(y: np.ndarray, cw: int, ch: int) -> np.ndarray:
    if y.shape == (ch, cw):
        return y
    return cv2.resize(y, (cw, ch), interpolation=cv2.INTER_AREA)


class _Cursor:
    """Walk the original frames forward from `start`. Frame numbers must not decrease."""

    def __init__(self, info, start: int):
        self.idx = start - 1
        fps = float(info.fps)
        self.gen = decode_yuv(info, start_seconds=max(0.0, (start - 0.25) / fps) if start > 0 else 0.0)
        self.cur = None

    def get(self, j: int):
        while self.idx < j:
            nxt = next(self.gen, None)
            if nxt is None:
                return None
            self.cur, self.idx = nxt[0], self.idx + 1
        return self.cur if self.idx == j else None


def _grab(info, js, cw, ch) -> dict:
    """Decode a few original frames (by number) at working size."""
    js = sorted({j for j in js if j >= 0})
    if not js:
        return {}
    want, out = set(js), {}
    start, end = js[0], js[-1]
    fps = float(info.fps)
    gen = decode_yuv(info, start_seconds=max(0.0, (start - 0.25) / fps) if start > 0 else 0.0,
                     max_frames=end - start + 1)
    for k, (y, _, _) in enumerate(gen):
        if start + k in want:
            out[start + k] = _canon(y, cw, ch)
    return out


def _prescale(yl: np.ndarray, cw: int) -> np.ndarray:
    """Shrink a big leaked frame to about the working scale before warping."""
    h, w = yl.shape
    f = cw / w
    if f < 0.75:
        return cv2.resize(yl, (round(w * f), round(h * f)), interpolation=cv2.INTER_AREA)
    return yl


def detect_video(leaked: str, original: str, seed: int, params: VideoParams = VideoParams(),
                 m: int | None = None, progress: Callable[[int, int], None] | None = None
                 ) -> VideoDetection:
    io, il = probe(original), probe(leaked)
    sf, K = seg_frames(io, params), params.nslots
    m = m or capacity(io, params)
    cw, ch = canon_size(io.width, io.height)
    fps_o, fps_l = float(io.fps), float(il.fps)
    strength = effective_strength(io.width, io.height, params)

    # 1. candidate time offsets from thumbnails
    to, tl = decode_thumbs(io), decode_thumbs(il)
    cands = find_offsets(to, fps_o, tl, fps_l)
    ratio = fps_o / fps_l

    leaked_frames = ((i, _prescale(f[0], cw)) for i, f in enumerate(decode_yuv(il)))
    head = list(islice(leaked_frames, HEAD))
    sample = [head[k] for k in SAMPLE if k < len(head)] or head[:1]

    # 2. confirm the offset with the watermark: the right one shows strong correlation
    layouts = {}

    def layout_for(seg):
        if seg not in layouts:
            layouts[seg] = make_layout(ch, cw, segment_seed(seed, seg), K)
        return layouts[seg]

    results = []
    tried = []
    for tau, dist in cands:
        js = [int(round(i * ratio)) + tau for i, _ in sample]
        orig_frames = _grab(io, js, cw, ch)
        pairs = [(yl, orig_frames.get(j), j) for (i, yl), j in zip(sample, js)]
        pairs = [p for p in pairs if p[1] is not None]
        if not pairs:
            continue
        M, winfo = estimate_warp(pairs[0][0], pairs[0][1])
        energy = []
        for yl, yo, j in pairs:
            seg = j // sf
            if (seg + 1) * K > m:
                continue
            al, valid = apply_warp(yl, M, (cw, ch))
            bv = block_valid(valid)
            # Score = energy with the real pattern minus energy with a control pattern
            # (a segment that does not exist). Picture mismatch raises both equally;
            # only a real mark raises the first.
            e = []
            for sg in (seg, 100000 + seg):
                slot, pat = layout_for(sg)
                num, den, sig = plane_stats(yo.astype(np.float32), al.astype(np.float32),
                                            bv, slot, pat, strength, K)
                ok = den > 0
                e.append(float(np.mean((num[ok] / (sig * np.sqrt(NPOS * den[ok]))) ** 2)) if ok.any() else 0.0)
            energy.append(e[0] - e[1])
        score = float(np.mean(energy)) if energy else 0.0
        tried.append((tau, round(score, 1), len(energy)))
        results.append((score, tau, dist, M, winfo))
    if results:
        top = max(r[0] for r in results)
        # Prior: most leaks are not trimmed. If "no shift" scores close to the best,
        # take it. A real trim wins by a wide margin and is not affected.
        best = max(results, key=lambda r: r[0])
        zero = [r for r in results if r[1] == 0 and r[0] >= 0.6 * top]
        if zero:
            best = zero[0]
    else:
        best = (0.0, 0, 1.0, None, {"method": "none", "err": 0.0})
    score, tau, dist, M, winfo = best
    jmap = lambda i: int(round(i * ratio)) + tau

    # 3. accumulate evidence per symbol over all frames
    orig = _Cursor(io, max(0, jmap(0)))
    acc_num = np.zeros(m)
    acc_den = np.zeros(m)
    n_done = 0
    for i, yl in chain(head, leaked_frames):
        if i % params.frame_stride:
            continue
        j = jmap(i)
        if j < 0:
            continue
        seg = j // sf
        if (seg + 1) * K > m:
            break
        yo = orig.get(j)
        if yo is None:
            continue
        slot, pat = layout_for(seg)
        al, valid = apply_warp(yl, M, (cw, ch))
        num, den, sig = plane_stats(_canon(yo, cw, ch).astype(np.float32), al.astype(np.float32),
                                    block_valid(valid), slot, pat, strength, K)
        acc_num[seg * K:(seg + 1) * K] += num / sig**2
        acc_den[seg * K:(seg + 1) * K] += den / sig**2
        n_done += 1
        if progress and n_done % 10 == 0:
            progress(n_done, len(tl))
        if len(layouts) > 4:
            layouts.pop(min(layouts))

    z = np.where(acc_den > 0, acc_num / np.sqrt(NPOS * np.maximum(acc_den, 1e-12)), 0.0)
    return VideoDetection(
        z=z, bits=(z > 0).astype(np.uint8), confidence=np.clip(np.abs(z) / 3.0, 0, 1),
        coverage=float((acc_den > 0).mean()),
        info={"tau": tau, "sync_quality": dist, "sync_energy": score, "tried": tried,
              "warp": winfo.get("method"), "warp_err": winfo.get("err"), "frames_used": n_done},
    )
