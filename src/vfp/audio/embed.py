"""Embed one receiver's symbols into audio.

Each time-frequency cell of the STFT is scaled by (1 + alpha * symbol * pattern).
The mark follows the signal, so quiet parts get a quiet mark.
"""

from __future__ import annotations

from math import ceil

import numpy as np
from scipy.signal import istft, stft

from .common import (AudioParams, band, capacity, frames_per_seg, seg_layout)
from .ffio import decode_audio, encode_audio, probe_audio


def embed_audio_array(x: np.ndarray, sr: int, symbols: np.ndarray, seed: int,
                      p: AudioParams = AudioParams()) -> np.ndarray:
    """x: float32 (channels, n). Returns the marked samples, same shape."""
    n = x.shape[-1]
    need = capacity(n / sr, p)
    symbols = np.asarray(symbols)
    if len(symbols) < need:
        raise ValueError(f"need {need} symbols for this audio, got {len(symbols)}")
    _, _, Z = stft(x, sr, window="hann", nperseg=p.nfft, noverlap=p.nfft // 2,
                   boundary="zeros", padded=True)
    b0, b1 = band(sr, p)
    Fs, K = frames_per_seg(sr, p), p.nslots
    nt = Z.shape[-1]
    for seg in range(ceil(nt / Fs)):
        c0, c1 = seg * Fs, min(nt, (seg + 1) * Fs)
        w, slot = seg_layout(seed, seg, Fs, b1 - b0, K)
        s = 2.0 * symbols[seg * K:(seg + 1) * K].astype(np.float32) - 1.0
        mult = 1.0 + p.alpha * s[slot[: c1 - c0]] * w[: c1 - c0]       # (frames, bins)
        Z[:, b0:b1, c0:c1] *= mult.T[None].astype(np.float32)
    _, y = istft(Z, sr, window="hann", nperseg=p.nfft, noverlap=p.nfft // 2, boundary=True)
    return np.clip(y[..., :n], -1.0, 1.0).astype(np.float32)


def embed_audio(src: str, dst: str, symbols: np.ndarray, seed: int,
                p: AudioParams = AudioParams(), max_seconds: float | None = None) -> dict:
    """Mark an audio file. Output codec follows the extension of `dst`."""
    info = probe_audio(src)
    if info.channels > 2:
        raise ValueError("only mono and stereo are supported")
    x = decode_audio(src, max_seconds=max_seconds)
    y = embed_audio_array(x, info.sr, symbols, seed, p)
    encode_audio(y, info.sr, dst)
    return {"samples": x.shape[-1], "sr": info.sr, "symbols": capacity(x.shape[-1] / info.sr, p)}
