"""Audio round trip on generated audio (needs ffmpeg on PATH)."""

import shutil
import subprocess

import numpy as np
import pytest

from vfp.audio.common import AudioParams, capacity
from vfp.audio.detect import detect_audio
from vfp.audio.embed import embed_audio
from vfp.audio.ffio import encode_audio, probe_audio
from vfp.codes.keys import derive_seed
from vfp.codes.tardos import TardosCode, accusation_threshold

pytestmark = pytest.mark.skipif(shutil.which("ffmpeg") is None, reason="ffmpeg not installed")

USERS, USER = 20, 5
SEED = derive_seed(b"a" * 32, "audio")
P = AudioParams(nslots=24)
SR = 44100


def synth(seconds=12, seed=1):
    """Pink-ish noise with a slow amplitude beat and a few tones: music-like enough."""
    rng = np.random.default_rng(seed)
    n = SR * seconds
    spec = (rng.normal(size=n // 2 + 1) + 1j * rng.normal(size=n // 2 + 1))
    f = np.maximum(np.arange(n // 2 + 1), 1.0)
    pink = np.fft.irfft(spec / np.sqrt(f), n)
    pink /= np.abs(pink).max()
    t = np.arange(n) / SR
    # Tones drift in pitch, so the signal never repeats exactly (a perfectly periodic test
    # signal would match itself at many shifts, which real recordings do not).
    drift = np.cumsum(rng.normal(0, 0.02, n)) / SR * 200
    tones = sum(np.sin(2 * np.pi * (fr * t + drift * k)) for k, fr in enumerate((220, 330, 1250, 2970), 1)) / 30
    # Random note-like bursts give the signal sharp structure in time.
    bursts = np.convolve(rng.normal(size=n) * (rng.random(n) < 0.0005), np.hanning(400), "same") * 40
    env = 0.6 + 0.4 * np.sin(2 * np.pi * 0.7 * t)
    x = 0.5 * (0.6 * pink + tones + 0.3 * bursts) * env
    return np.stack([x, 0.9 * x + 0.05 * np.roll(x, 7)]).astype(np.float32)


@pytest.fixture(scope="module")
def setup(tmp_path_factory):
    d = tmp_path_factory.mktemp("aud")
    src = d / "src.wav"
    encode_audio(synth(), SR, str(src))
    m = capacity(probe_audio(str(src)).duration, P)
    code = TardosCode.create(USERS, m, derive_seed(b"a" * 32, "code"), n_colluders=1)
    marked = d / "marked.wav"
    embed_audio(str(src), str(marked), code.codeword(USER), SEED, P)
    return d, src, marked, code, m


def trace(setup, leaked):
    _, src, _, code, m = setup
    det = detect_audio(str(leaked), str(src), SEED, P, m=m)
    s = code.scores(det.bits, det.confidence)
    return int(np.argmax(s)), s[USER], accusation_threshold(USERS, m, 1e-3), det


def ff(*args):
    subprocess.run(["ffmpeg", "-y", "-v", "error", *map(str, args)], check=True)


def test_mark_is_quiet(setup):
    from vfp.audio.ffio import decode_audio
    _, src, marked, _, _ = setup
    x, y = decode_audio(str(src)), decode_audio(str(marked))
    n = min(x.shape[1], y.shape[1])
    snr = 10 * np.log10(np.sum(x[:, :n] ** 2) / np.sum((y[:, :n] - x[:, :n]) ** 2))
    assert snr > 28


def test_marked_traced(setup):
    who, score, thr, _ = trace(setup, setup[2])
    assert who == USER and score > thr


def test_mp3_traced(setup):
    d, _, marked, _, _ = setup
    out = d / "a.mp3"
    ff("-i", marked, "-c:a", "libmp3lame", "-b:a", "96k", out)
    who, score, thr, _ = trace(setup, out)
    assert who == USER and score > thr


def test_trimmed_resampled_traced(setup):
    d, _, marked, _, _ = setup
    out = d / "b.m4a"
    ff("-ss", "2.5", "-i", marked, "-ar", "22050", "-c:a", "aac", "-b:a", "64k", out)
    who, score, thr, det = trace(setup, out)
    # AAC leaves a little encoder delay, so allow a couple of thousand samples.
    assert abs(det.info["offset"] - int(2.5 * SR)) < 2048
    assert who == USER and score > thr


def test_unmarked_accuses_nobody(setup):
    _, src, _, _, _ = setup
    who, score, thr, _ = trace(setup, src)
    assert score < thr
