"""Find where a leaked audio file sits inside the original (sample-accurate)."""

from __future__ import annotations

import numpy as np
from scipy.signal import butter, fftconvolve, lfilter, sosfiltfilt

BAND_HZ = 6000.0


def _prep(a: np.ndarray, sr: int) -> np.ndarray:
    """Whiten, then keep only the band every copy still has.

    Whitening (pre-emphasis) stops steady low notes from making many equal correlation
    peaks one period apart. The lowpass drops the top octave, which codecs, resampling
    and lowpass filters remove, so it cannot match anyway.
    """
    e = lfilter([1.0, -0.97], [1.0], a.astype(np.float64))
    sos = butter(6, min(BAND_HZ, sr * 0.45), btype="low", fs=sr, output="sos")
    return sosfiltfilt(sos, e)


def find_offset(orig: np.ndarray, leak: np.ndarray, sr: int):
    """orig, leak: mono float arrays at the same sample rate.

    Returns (off, score). Leaked sample i matches original sample i + off.
    score is the normalized correlation at the best place (1.0 = perfect).
    """
    D = max(1, int(sr // (2.5 * BAND_HZ)))
    of, lf = _prep(orig, sr), _prep(leak, sr)
    o, l = of[::D], lf[::D]
    # Leave a margin at the end: codecs pad the end of a file, and the window must still
    # fit inside the original when the copy was cut at the end.
    margin = int(0.3 * sr / D) if len(l) > int(2 * sr / D) else 0
    w = min(len(l) - margin, int(12 * sr / D))
    # take the loudest 12 s window from the first 90 s of the leaked file
    step = max(1, int(3 * sr / D))
    starts = range(0, max(1, min(len(l) - margin, int(90 * sr / D)) - w + 1), step)
    st = max(starts, key=lambda s: float(np.sum(l[s:s + w] ** 2)))
    chunk = l[st:st + w]
    if len(o) < w:
        return 0, 0.0
    c = fftconvolve(o, chunk[::-1], mode="valid")                  # k = chunk start in o
    cs = np.concatenate([[0.0], np.cumsum(o ** 2)])
    ew = cs[w:w + len(c)] - cs[:len(c)]
    score = c / (np.sqrt(ew * np.sum(chunk ** 2)) + 1e-12)
    k0 = int(np.argmax(score))
    off = (k0 - st) * D

    # refine at full rate
    L = lf[st * D: st * D + w * D]
    best, best_off = -2.0, off
    for cand in range(off - D - 2, off + D + 3):
        a = st * D + cand
        if a < 0 or a + len(L) > len(orig):
            continue
        seg = of[a:a + len(L)]
        r = float(np.dot(L, seg) / (np.linalg.norm(L) * np.linalg.norm(seg) + 1e-12))
        if r > best:
            best, best_off = r, cand
    return best_off, best
