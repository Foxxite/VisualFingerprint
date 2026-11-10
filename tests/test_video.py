"""Video round trip on a short generated clip (needs ffmpeg on PATH)."""

import shutil
import subprocess

import numpy as np
import pytest

from vfp.codes.keys import derive_seed
from vfp.codes.tardos import TardosCode, accusation_threshold
from vfp.video.common import VideoParams, capacity
from vfp.video.detect import detect_video
from vfp.video.embed import embed_video
from vfp.video.ffio import probe

pytestmark = pytest.mark.skipif(shutil.which("ffmpeg") is None, reason="ffmpeg not installed")

USERS, USER = 20, 3
SEED = derive_seed(b"v" * 32, "video")
PARAMS = VideoParams(nslots=32)


@pytest.fixture(scope="module")
def clip(tmp_path_factory):
    d = tmp_path_factory.mktemp("vid")
    src = d / "src.mp4"
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-f", "lavfi", "-i",
                    "mandelbrot=size=640x360:rate=24", "-t", "4", "-c:v", "libx264",
                    "-crf", "18", "-pix_fmt", "yuv420p", str(src)], check=True)
    info = probe(str(src))
    m = capacity(info, PARAMS)
    code = TardosCode.create(USERS, m, derive_seed(b"v" * 32, "code"), n_colluders=1)
    word = code.codeword(USER)
    marked = d / "marked.mp4"
    embed_video(str(src), str(marked), word, SEED, PARAMS)
    return d, src, marked, code, m


def trace(clip, leaked):
    d, src, _, code, m = clip
    det = detect_video(str(leaked), str(src), SEED, PARAMS, m=m)
    s = code.scores(det.bits, det.confidence)
    return int(np.argmax(s)), s[USER], accusation_threshold(USERS, m, 1e-3), det


def test_marked_copy_traced(clip):
    who, score, thr, _ = trace(clip, clip[2])
    assert who == USER and score > thr


def test_recompressed_copy_traced(clip):
    d, _, marked, _, _ = clip
    out = d / "crf28.mp4"
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", str(marked), "-c:v", "libx264",
                    "-crf", "28", str(out)], check=True)
    who, score, thr, _ = trace(clip, out)
    assert who == USER and score > thr


def test_trimmed_scaled_copy_traced(clip):
    d, _, marked, _, _ = clip
    out = d / "trim.mp4"
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-ss", "1.0", "-i", str(marked), "-vf",
                    "scale=480:-2", "-c:v", "libx264", "-crf", "24", str(out)], check=True)
    who, score, thr, det = trace(clip, out)
    assert det.info["tau"] == 24
    assert who == USER and score > thr


def test_unmarked_source_accuses_nobody(clip):
    _, src, _, _, _ = clip
    who, score, thr, _ = trace(clip, src)
    assert score < thr
