"""End to end through the command line: init, receivers, asset, mark, trace."""

import json
import shutil
import subprocess

import numpy as np
import pytest

from vfp import cli
from vfp.eval.image_eval import synthetic_image
from vfp.pipeline import write_image
from tests.test_audio import SR, synth
from vfp.audio.ffio import encode_audio

pytestmark = pytest.mark.skipif(shutil.which("ffmpeg") is None, reason="ffmpeg not installed")


def run(home, *args):
    return cli.main(["--home", str(home), *map(str, args)])


def trace_json(home, capsys, leaked, *extra):
    capsys.readouterr()
    assert run(home, "trace", leaked, "--json", *extra) == 0
    return json.loads(capsys.readouterr().out)


@pytest.fixture(scope="module")
def ws(tmp_path_factory):
    d = tmp_path_factory.mktemp("cli")
    home = d / "home"
    assert run(home, "init") == 0
    assert run(home, "receivers", "add", "alice", "bob", "carol") == 0
    write_image(d / "pic.png", synthetic_image(800, 600))
    encode_audio(synth(16), SR, str(d / "song.wav"))
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-f", "lavfi", "-i", "mandelbrot=size=640x360:rate=24",
                    "-t", "3", "-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p", str(d / "vid.mp4")],
                   check=True)
    for f in ("pic.png", "song.wav", "vid.mp4"):
        assert run(home, "asset", "add", d / f, "--name", f.split(".")[0]) == 0
    assert run(home, "mark", "pic", "--all", "--out", d / "out") == 0
    assert run(home, "mark", "song", "--all", "--out", d / "out") == 0
    assert run(home, "mark", "vid", "--to", "bob", "--out", d / "out") == 0
    return d, home


def test_image_leak(ws, capsys):
    d, home = ws
    leak = d / "leak.jpg"
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", d / "out" / "pic__carol.png",
                    "-vf", "scale=iw*0.75:-2", "-q:v", "5", leak], check=True)
    r = trace_json(home, capsys, leak)
    assert r["asset"] == "pic" and r["flagged"] == ["carol"]


def test_audio_leak_excerpt(ws, capsys):
    d, home = ws
    leak = d / "leak.mp3"
    # A 9-second excerpt of a 16-second song, saved as 128k MP3. The secret key is random in
    # every run, so the test must not sit near the accusation threshold.
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-ss", "2", "-t", "9", "-i", d / "out" / "song__alice.wav",
                    "-c:a", "libmp3lame", "-b:a", "128k", leak], check=True)
    r = trace_json(home, capsys, leak)
    assert r["asset"] == "song" and r["flagged"] == ["alice"]


def test_video_leak(ws, capsys):
    d, home = ws
    leak = d / "leak.mp4"
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", d / "out" / "vid__bob.mp4", "-vf", "scale=480:-2",
                    "-c:v", "libx264", "-crf", "26", "-an", leak], check=True)
    r = trace_json(home, capsys, leak, "--asset", "vid")
    assert r["flagged"] == ["bob"]


def test_unmarked_file_is_inconclusive(ws, capsys):
    d, home = ws
    r = trace_json(home, capsys, d / "pic.png", "--asset", "pic")
    assert r["flagged"] == [] and r["verdict"].startswith("INCONCLUSIVE")


def test_unknown_asset_is_an_error(ws):
    _, home = ws
    assert run(home, "mark", "nope", "--all") == 1
