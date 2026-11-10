"""Find which original frame each leaked frame came from (time offset, frame rate)."""

from __future__ import annotations

import numpy as np


def _norm(t: np.ndarray) -> np.ndarray:
    t = t - t.mean(axis=1, keepdims=True)
    return t / (t.std(axis=1, keepdims=True) + 1.0)


def find_offsets(sig_o: np.ndarray, fps_o: float, sig_l: np.ndarray, fps_l: float,
                 n_samples: int = 64, top: int = 4, min_gap: int = 4):
    """Candidate offsets, best first: list of (tau, distance).

    Leaked frame i maps to original frame round(i / fps_l * fps_o) + tau.
    Thumbnails can be fooled by slow scenes or heavy edits, so the caller should
    confirm the winner with the watermark itself.
    """
    So, Sl = _norm(sig_o), _norm(sig_l)
    No, Nl = len(So), len(Sl)
    idx = np.unique(np.linspace(0, Nl - 1, min(Nl, n_samples)).astype(int))
    base = idx / fps_l * fps_o
    Sl_s = Sl[idx]
    lo, hi = int(-base.max()), int(No - base.min())
    taus, dists = [], []
    for tau in range(lo, hi):
        j = np.rint(base + tau).astype(int)
        ok = (j >= 0) & (j < No)
        if ok.mean() < 0.6:
            continue
        taus.append(tau)
        dists.append(np.mean((Sl_s[ok] - So[j[ok]]) ** 2))
    if not taus:
        return [(0, 1.0)]
    taus, dists = np.array(taus), np.array(dists)
    med = np.median(dists) + 1e-9
    out = []
    for k in np.argsort(dists):
        if all(abs(int(taus[k]) - t) >= min_gap for t, _ in out):
            out.append((int(taus[k]), float(dists[k] / med)))
        if len(out) >= top:
            break
    if 0 in taus and all(t != 0 for t, _ in out):
        out.append((0, float(dists[int(np.where(taus == 0)[0][0])] / med)))
    return out
