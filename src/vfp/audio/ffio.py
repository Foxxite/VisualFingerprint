"""FFmpeg pipes for audio."""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass

import numpy as np


@dataclass
class AudioInfo:
    path: str
    sr: int
    channels: int
    duration: float


def probe_audio(path: str) -> AudioInfo:
    out = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries",
                          "stream=sample_rate,channels,duration:format=duration", "-of", "json",
                          str(path)], capture_output=True, check=True).stdout
    j = json.loads(out)
    s = j["streams"][0]
    dur = s.get("duration")
    if dur in (None, "N/A"):
        dur = j["format"]["duration"]
    return AudioInfo(str(path), int(s["sample_rate"]), int(s["channels"]), float(dur))


def decode_audio(path: str, sr: int | None = None, channels: int | None = None,
                 max_seconds: float | None = None) -> np.ndarray:
    """Return float32 samples, shape (channels, n)."""
    info = probe_audio(path)
    ch = channels or info.channels
    cmd = ["ffmpeg", "-v", "error", "-i", str(path), "-vn"]
    if max_seconds:
        cmd += ["-t", str(max_seconds)]
    cmd += ["-ac", str(ch)]
    if sr:
        cmd += ["-ar", str(sr)]
    cmd += ["-f", "f32le", "-acodec", "pcm_f32le", "-"]
    raw = subprocess.run(cmd, capture_output=True, check=True).stdout
    a = np.frombuffer(raw[: len(raw) // (4 * ch) * 4 * ch], np.float32)
    return a.reshape(-1, ch).T.copy()


# output extension -> ffmpeg codec options
ENCODERS = {
    ".wav": ["-c:a", "pcm_s24le"],
    ".flac": ["-c:a", "flac"],
    ".mp3": ["-c:a", "libmp3lame", "-b:a", "320k"],
    ".m4a": ["-c:a", "aac", "-b:a", "256k"],
    ".ogg": ["-c:a", "libopus", "-b:a", "160k"],
}


def encode_audio(x: np.ndarray, sr: int, dst: str, extra: list[str] | None = None):
    """Write float32 samples (channels, n) to `dst`. Codec follows the file extension."""
    import os
    ext = os.path.splitext(str(dst))[1].lower()
    codec = extra if extra is not None else ENCODERS[ext]
    ch = x.shape[0]
    cmd = ["ffmpeg", "-y", "-v", "error", "-f", "f32le", "-ar", str(sr), "-ac", str(ch),
           "-i", "-", *codec, str(dst)]
    p = subprocess.run(cmd, input=np.ascontiguousarray(x.T, dtype=np.float32).tobytes(),
                       capture_output=True)
    if p.returncode:
        raise RuntimeError(p.stderr.decode(errors="replace"))
