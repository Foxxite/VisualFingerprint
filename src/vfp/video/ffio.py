"""FFmpeg / ffprobe pipes: probe, decode raw frames, encode raw frames."""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from fractions import Fraction
from typing import Iterator

import numpy as np


@dataclass
class VideoInfo:
    path: str
    width: int           # even, after rotation
    height: int
    fps: Fraction
    nframes: int         # estimate when the container has no count
    duration: float
    has_audio: bool
    audio_codec: str | None
    color: dict          # colorspace / primaries / trc / range if known


def _run_json(cmd):
    out = subprocess.run(cmd, capture_output=True, check=True).stdout
    return json.loads(out)


def probe(path: str) -> VideoInfo:
    j = _run_json(["ffprobe", "-v", "error", "-show_streams", "-show_format", "-of", "json", str(path)])
    v = next(s for s in j["streams"] if s["codec_type"] == "video")
    a = next((s for s in j["streams"] if s["codec_type"] == "audio"), None)
    w, h = int(v["width"]), int(v["height"])
    rot = 0
    for sd in v.get("side_data_list", []) or []:
        if "rotation" in sd:
            rot = int(sd["rotation"])
    if abs(rot) % 180 == 90:
        w, h = h, w
    fps = Fraction(v.get("avg_frame_rate") or v["r_frame_rate"])
    if fps == 0:
        fps = Fraction(v["r_frame_rate"])
    dur = None
    for src in (v.get("duration"), j.get("format", {}).get("duration")):
        if src not in (None, "N/A"):
            dur = float(src)
            break
    nb = v.get("nb_frames")
    if nb not in (None, "N/A"):
        n = int(nb)
        dur = dur or n / float(fps)
    else:
        n = int(round((dur or 0) * float(fps)))
    color = {}
    for key, opt in (("color_space", "colorspace"), ("color_primaries", "color_primaries"),
                     ("color_transfer", "color_trc"), ("color_range", "color_range")):
        val = v.get(key)
        if val and val != "unknown":
            color[opt] = val
    return VideoInfo(str(path), w - w % 2, h - h % 2, fps, n, dur or 0.0,
                     a is not None, a["codec_name"] if a else None, color)


def _read_exact(f, n: int) -> bytes | None:
    buf = bytearray()
    while len(buf) < n:
        chunk = f.read(n - len(buf))
        if not chunk:
            return None
        buf += chunk
    return bytes(buf)


def decode_yuv(info: VideoInfo, max_seconds: float | None = None,
               start_seconds: float = 0.0, max_frames: int | None = None) -> Iterator[tuple]:
    """Yield (y, u, v) uint8 arrays per frame. y is (H,W); u, v are (H/2,W/2)."""
    w, h = info.width, info.height
    cmd = ["ffmpeg", "-v", "error"]
    if start_seconds > 0:
        cmd += ["-ss", f"{start_seconds:.6f}"]
    cmd += ["-i", info.path]
    if max_seconds:
        cmd += ["-t", str(max_seconds)]
    if max_frames:
        cmd += ["-frames:v", str(max_frames)]
    cmd += ["-an", "-vf", f"crop={w}:{h}:0:0", "-fps_mode", "passthrough",
            "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"]
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE)
    ysz, csz = w * h, (w // 2) * (h // 2)
    try:
        while True:
            raw = _read_exact(p.stdout, ysz + 2 * csz)
            if raw is None:
                break
            a = np.frombuffer(raw, np.uint8)
            yield (a[:ysz].reshape(h, w), a[ysz:ysz + csz].reshape(h // 2, w // 2),
                   a[ysz + csz:].reshape(h // 2, w // 2))
    finally:
        p.stdout.close()
        p.kill()
        p.wait()


def decode_thumbs(info: VideoInfo, size=(32, 18), max_seconds: float | None = None) -> np.ndarray:
    """All frames as small gray thumbnails, shape (N, h*w), float32. For time sync."""
    tw, th = size
    cmd = ["ffmpeg", "-v", "error", "-i", info.path]
    if max_seconds:
        cmd += ["-t", str(max_seconds)]
    cmd += ["-an", "-vf", f"scale={tw}:{th}:flags=area", "-fps_mode", "passthrough",
            "-f", "rawvideo", "-pix_fmt", "gray", "-"]
    raw = subprocess.run(cmd, capture_output=True, check=True).stdout
    n = len(raw) // (tw * th)
    return np.frombuffer(raw[: n * tw * th], np.uint8).reshape(n, tw * th).astype(np.float32)


class Encoder:
    """Pipe raw yuv420p frames into x264. Copies (or re-encodes) the audio of `audio_from`."""

    def __init__(self, dst: str, info: VideoInfo, crf: int = 16, preset: str = "medium",
                 audio_from: str | None = None, max_seconds: float | None = None):
        w, h = info.width, info.height
        cmd = ["ffmpeg", "-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "yuv420p",
               "-s", f"{w}x{h}", "-r", str(info.fps), "-i", "-"]
        use_audio = audio_from is not None and info.has_audio
        if use_audio:
            cmd += ["-i", audio_from, "-map", "0:v", "-map", "1:a:0"]
        cmd += ["-c:v", "libx264", "-crf", str(crf), "-preset", preset, "-pix_fmt", "yuv420p"]
        for k, v in info.color.items():
            cmd += [f"-{k}", v]
        if use_audio:
            copy_ok = info.audio_codec in ("aac", "mp3", "ac3")
            cmd += ["-c:a", "copy"] if copy_ok else ["-c:a", "aac", "-b:a", "192k"]
        if max_seconds:
            cmd += ["-t", str(max_seconds)]
        cmd += ["-movflags", "+faststart", str(dst)]
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE)

    def write(self, y: np.ndarray, u: np.ndarray, v: np.ndarray):
        self.p.stdin.write(np.ascontiguousarray(y).tobytes())
        self.p.stdin.write(np.ascontiguousarray(u).tobytes())
        self.p.stdin.write(np.ascontiguousarray(v).tobytes())

    def close(self):
        self.p.stdin.close()
        rc = self.p.wait()
        if rc:
            raise RuntimeError(f"ffmpeg encode failed (exit {rc})")

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        if exc[0] is None:
            self.close()
        else:
            self.p.kill()
            self.p.wait()
