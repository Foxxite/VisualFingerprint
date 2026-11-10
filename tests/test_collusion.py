"""Colluders blend their copies. At least one must be accused; no innocent may be."""

import shutil

import numpy as np
import pytest

from vfp import pipeline
from vfp.audio.ffio import decode_audio, encode_audio
from vfp.eval.collusion_eval import aud_interleave
from vfp.eval.image_eval import synthetic_image
from vfp.workspace import Workspace
from tests.test_audio import SR, synth

pytestmark = pytest.mark.skipif(shutil.which("ffmpeg") is None, reason="ffmpeg not installed")

NAMES = ["r1", "r2", "r3", "r4", "r5", "r6"]


def make_ws(tmp_path, src, **kw):
    ws = Workspace.init(tmp_path / "ws")
    for n in NAMES:
        ws.add_receiver(n)
    asset, plan = pipeline.add_asset(ws, src, "a", max_receivers=100, n_colluders=3, eps=1e-2, **kw)
    copies = {n: pipeline.mark_for_receiver(ws, asset, n, tmp_path / "copies") for n in NAMES}
    return ws, copies


def check(r, guilty):
    assert set(r.flagged) & set(guilty), f"no colluder accused: {r.ranking[:4]}"
    assert not (set(r.flagged) - set(guilty)), f"innocent accused: {r.flagged}"


def test_image_average_of_three(tmp_path):
    src = tmp_path / "p.png"
    pipeline.write_image(src, synthetic_image(960, 640))
    ws, copies = make_ws(tmp_path, src)
    guilty = ["r2", "r4", "r5"]
    mixed = np.mean([pipeline.read_image(copies[g]) for g in guilty], axis=0)
    leak = tmp_path / "leak.jpg"
    pipeline.write_image(leak, np.rint(mixed).astype(np.uint8))
    check(pipeline.trace_leak(ws, leak, "a"), guilty)


def test_audio_interleave_of_three(tmp_path):
    src = tmp_path / "s.wav"
    encode_audio(synth(40), SR, str(src))
    ws, copies = make_ws(tmp_path, src)
    guilty = ["r1", "r3", "r6"]
    xs = [decode_audio(str(copies[g])) for g in guilty]
    n = min(x.shape[1] for x in xs)
    mixed = aud_interleave([x[:, :n] for x in xs], SR, np.random.default_rng(3))
    leak = tmp_path / "leak.mp3"
    encode_audio(mixed, SR, str(leak), ["-c:a", "libmp3lame", "-b:a", "128k"])
    check(pipeline.trace_leak(ws, leak, "a"), guilty)


def test_audio_average_of_two(tmp_path):
    src = tmp_path / "s.wav"
    encode_audio(synth(20), SR, str(src))
    ws, copies = make_ws(tmp_path, src)
    guilty = ["r2", "r5"]
    xs = [decode_audio(str(copies[g])) for g in guilty]
    n = min(x.shape[1] for x in xs)
    leak = tmp_path / "leak.wav"
    encode_audio(np.mean([x[:, :n] for x in xs], axis=0).astype(np.float32), SR, str(leak))
    check(pipeline.trace_leak(ws, leak, "a"), guilty)
