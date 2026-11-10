"""v1.1: exact accusation threshold and symbol density that adapts to clip length."""

import shutil
import subprocess

import numpy as np
import pytest

from vfp import pipeline
from vfp.audio.common import AUDIO_NSLOTS_MAX, AudioParams
from vfp.audio.ffio import encode_audio
from vfp.codes.tardos import TardosCode, accusation_threshold, required_length
from vfp.eval.image_eval import synthetic_image
from vfp.video.common import VIDEO_NSLOTS_MAX, VideoParams
from tests.test_audio import SR, synth

needs_ffmpeg = pytest.mark.skipif(shutil.which("ffmpeg") is None, reason="ffmpeg not installed")


# ---------------------------------------------------------------- exact threshold
def test_threshold_shrinks_when_symbols_are_lost():
    full = accusation_threshold(1000, 400, 1e-3)
    half = accusation_threshold(1000, 400, 1e-3, weights=np.r_[np.ones(200), np.zeros(200)])
    assert half == pytest.approx(full / np.sqrt(2), rel=1e-9)
    assert accusation_threshold(1000, 400, 1e-3, weights=np.ones(400)) == pytest.approx(full)


def test_threshold_has_a_floor_when_there_is_almost_no_evidence():
    from scipy.stats import norm
    floor = accusation_threshold(1000, 400, 1e-3, weights=np.zeros(400))
    assert floor == pytest.approx(5 * norm.isf(1e-3 / 1000), rel=1e-9)     # sqrt(25) = 5


def test_exact_threshold_keeps_the_error_rate():
    """Innocent receivers must pass the exact threshold no more often than promised."""
    rng = np.random.default_rng(3)
    m, n = 600, 20000
    code = TardosCode.create(n, m, seed=11, n_colluders=3)
    # unreliable symbols: random weights, many near zero, as in a damaged leak
    w = np.clip(rng.exponential(0.4, m), 0, 1)
    y = rng.integers(0, 2, m)
    sc = code.scores(y, w)
    eps_user = 1e-2                                       # per-receiver level (n_users=1)
    thr = accusation_threshold(1, m, eps_user, weights=w)
    assert (sc > thr).mean() <= eps_user * 1.3


# ---------------------------------------------------------------- plan: density
def test_short_audio_packs_more_symbols(tmp_path):
    f = tmp_path / "short.wav"
    encode_audio(synth(10), SR, str(f))
    plan = pipeline.plan_file(f, max_receivers=1000, eps=1e-3, n_colluders=3)
    n = plan.params["nslots"]
    assert AudioParams().nslots < n <= AUDIO_NSLOTS_MAX
    assert plan.m >= required_length(1000, 3, 1e-3) or n == AUDIO_NSLOTS_MAX
    assert any("packing" in x for x in plan.notes)


def test_short_audio_dense_code_survives_damage_end_to_end(tmp_path):
    """Dense code on a 10 s clip: 2 colluders blend, then a 64k MP3 re-encode. Must still trace."""
    from vfp.audio.ffio import decode_audio
    from vfp.workspace import Workspace
    src = tmp_path / "short.wav"
    encode_audio(synth(10), SR, str(src))
    ws = Workspace.init(tmp_path / "ws")
    names = ["a", "b", "c", "d"]
    for n in names:
        ws.add_receiver(n)
    asset, plan = pipeline.add_asset(ws, src, "s", max_receivers=1000, n_colluders=3, eps=1e-3)
    assert plan.params["nslots"] > AudioParams().nslots
    copies = {n: pipeline.mark_for_receiver(ws, asset, n, tmp_path / "out") for n in names}

    # one leaker, harsh re-encode
    leak = tmp_path / "one.mp3"
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", str(copies["c"]), "-c:a", "libmp3lame",
                    "-b:a", "64k", str(leak)], check=True)
    assert pipeline.trace_leak(ws, leak, "s").flagged == ["c"]

    # two colluders average, then re-encode
    xs = [decode_audio(str(copies[n])) for n in ("b", "d")]
    n_ = min(x.shape[1] for x in xs)
    blend = tmp_path / "blend.mp3"
    encode_audio(np.mean([x[:, :n_] for x in xs], axis=0).astype(np.float32), SR, str(blend),
                 ["-c:a", "libmp3lame", "-b:a", "96k"])
    r = pipeline.trace_leak(ws, blend, "s")
    assert set(r.flagged) & {"b", "d"} and not (set(r.flagged) - {"b", "d"})


def test_long_audio_keeps_the_default(tmp_path):
    f = tmp_path / "long.wav"
    encode_audio(np.zeros((2, SR * 200), np.float32) + 1e-3, SR, str(f))
    plan = pipeline.plan_file(f, max_receivers=1000, eps=1e-3, n_colluders=3)
    assert plan.params["nslots"] == AudioParams().nslots


def test_manual_slots_win(tmp_path):
    f = tmp_path / "short.wav"
    encode_audio(synth(10), SR, str(f))
    assert pipeline.plan_file(f, slots=40).params["nslots"] == 40


def test_fewer_planned_receivers_need_fewer_symbols(tmp_path):
    f = tmp_path / "short.wav"
    encode_audio(synth(10), SR, str(f))
    small = pipeline.plan_file(f, max_receivers=20, eps=1e-2, n_colluders=2).params["nslots"]
    big = pipeline.plan_file(f, max_receivers=100000, eps=1e-6, n_colluders=4).params["nslots"]
    assert small <= big


def test_image_plan_uses_what_fits(tmp_path):
    f = tmp_path / "p.png"
    pipeline.write_image(f, synthetic_image(960, 640))            # 9600 blocks
    plan = pipeline.plan_file(f, max_receivers=1000, eps=1e-3, n_colluders=3)
    assert plan.m == 9600 // 12 == plan.params["nslots"]


@needs_ffmpeg
def test_short_video_packs_more_symbols(tmp_path):
    f = tmp_path / "v.mp4"
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-f", "lavfi", "-i", "mandelbrot=size=640x360:rate=24",
                    "-t", "3", "-c:v", "libx264", "-pix_fmt", "yuv420p", str(f)], check=True)
    plan = pipeline.plan_file(f, max_receivers=1000, eps=1e-3, n_colluders=3)
    assert VideoParams().nslots < plan.params["nslots"] <= VIDEO_NSLOTS_MAX
