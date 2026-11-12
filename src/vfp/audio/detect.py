"""Read symbols from a leaked audio file, using the original as reference."""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np
from scipy.signal import stft

from .align import find_offset
from .common import (AudioParams, band, capacity, frames_per_seg, hop, seg_layout)
from .ffio import decode_audio, probe_audio


GAIN_GROUP = 96   # bins per local gain estimate


@dataclass
class AudioDetection:
    z: np.ndarray
    bits: np.ndarray
    confidence: np.ndarray
    coverage: float
    info: dict


def detect_audio(leaked: str, original: str, seed: int, p: AudioParams = AudioParams(),
                 m: int | None = None) -> AudioDetection:
    io = probe_audio(original)
    sr = io.sr
    xo = decode_audio(original, channels=1)[0]
    yl = decode_audio(leaked, sr=sr, channels=1)[0]
    return detect_arrays(xo, yl, sr, io.duration, seed, p, m)


def detect_arrays(xo: np.ndarray, yl: np.ndarray, sr: int, duration: float, seed: int,
                  p: AudioParams = AudioParams(), m: int | None = None) -> AudioDetection:
    """Same as `detect_audio`, on mono arrays already at the same sample rate."""
    m = m or capacity(duration, p)

    off, score = find_offset(xo, yl, sr)
    h = hop(p)
    lo = max(0, off)
    lo = -(-lo // h) * h                       # start on a frame boundary of the original
    hi = min(len(xo), off + len(yl))
    if hi - lo < p.nfft * 2:
        z = np.zeros(m)
        return AudioDetection(z, z.astype(np.uint8), z, 0.0, {"offset": off, "align": score})
    xr, yr = xo[lo:hi], yl[lo - off: hi - off]
    gain = float(np.dot(yr, xr) / (np.dot(xr, xr) + 1e-12))
    if abs(gain) > 1e-3:
        yr = yr / gain

    kw = dict(fs=sr, window="hann", nperseg=p.nfft, noverlap=p.nfft - h, boundary=None, padded=False)
    _, _, Zo = stft(xr, **kw)
    _, _, Zl = stft(yr, **kw)
    b0, b1 = band(sr, p)
    Zo, Zl = Zo[b0:b1], Zl[b0:b1]
    nf = Zo.shape[1]
    absz = np.abs(Zo)

    # Undo level changes that vary over time and frequency (compressor, EQ, fades):
    # find the best gain per frame and per group of bins, then divide it out.
    G = GAIN_GROUP
    nb = Zo.shape[0]
    ng = -(-nb // G)
    padw = ((0, ng * G - nb), (0, 0))
    grp = lambda a: np.pad(a, padw).reshape(ng, G, -1).sum(axis=1)
    g = grp(np.real(Zl * np.conj(Zo))) / (grp(absz**2) + 1e-12)
    g = np.repeat(g, G, axis=0)[:nb]
    Zl = Zl / np.where(np.abs(g) > 1e-3, g, 1.0)
    proj = np.real((Zl - Zo) * np.conj(Zo)) / (absz + 1e-9)        # change along the signal
    med = np.median(proj, axis=0)
    sigma = np.maximum(1.4826 * np.median(np.abs(proj - med), axis=0), 1e-7)   # per frame
    wn = (absz * proj) / sigma**2
    wd = absz**2 / sigma**2

    Fs, K = frames_per_seg(sr, p), p.nslots
    t = lo // h + 1 + np.arange(nf)            # frame number on the original's grid
    seg_of = t // Fs
    acc_num = np.zeros(m)
    acc_den = np.zeros(m)
    for seg in np.unique(seg_of):
        if (seg + 1) * K > m:
            continue
        cols = np.where(seg_of == seg)[0]
        w, slot = seg_layout(seed, int(seg), Fs, b1 - b0, K)
        loc = t[cols] - seg * Fs
        sl, ww = slot[loc], w[loc]                                  # (cols, bins)
        acc_num[seg * K:(seg + 1) * K] += np.bincount(sl.ravel(), weights=(wn[:, cols].T * ww).ravel(),
                                                      minlength=K)
        acc_den[seg * K:(seg + 1) * K] += np.bincount(sl.ravel(), weights=wd[:, cols].T.ravel(),
                                                      minlength=K)
    z = np.where(acc_den > 0, acc_num / np.sqrt(np.maximum(acc_den, 1e-12)), 0.0)
    return AudioDetection(z, (z > 0).astype(np.uint8), np.clip(np.abs(z) / 3.0, 0, 1),
                          float((acc_den > 0).mean()),
                          {"offset": off, "align": score, "gain": gain})
