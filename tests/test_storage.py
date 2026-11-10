"""Where originals live: deleting the source or moving the workspace must not break anything."""

import shutil

import pytest

from vfp import cli
from vfp.eval.image_eval import synthetic_image
from vfp.pipeline import write_image


def run(*args):
    return cli.main([str(a) for a in args])


def test_originals_survive_deleting_the_source_and_moving_the_workspace(tmp_path, monkeypatch, capsys):
    work = tmp_path / "work"
    work.mkdir()
    monkeypatch.chdir(work)                              # default workspace: ./.vfp (a relative path)
    write_image(work / "orig.png", synthetic_image(960, 640))
    assert run("init") == 0
    assert run("receivers", "add", "alice", "bob") == 0
    assert run("asset", "add", "orig.png", "--name", "pic") == 0

    (work / "orig.png").unlink()                         # the user deletes the file they registered
    assert run("mark", "pic", "--all", "--out", "out") == 0
    assert (work / "out" / "pic__bob.png").exists()

    # run from a different folder, workspace given as an absolute path
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    monkeypatch.chdir(elsewhere)
    capsys.readouterr()
    assert run("--home", work / ".vfp", "trace", work / "out" / "pic__bob.png", "--json") == 0
    assert '"LEAKER: bob"' in capsys.readouterr().out

    # move the whole workspace and the copies
    moved = tmp_path / "moved"
    shutil.move(str(work), str(moved))
    capsys.readouterr()
    assert run("--home", moved / ".vfp", "trace", moved / "out" / "pic__bob.png", "--json") == 0
    assert '"LEAKER: bob"' in capsys.readouterr().out


def test_old_workspaces_with_stored_absolute_paths_still_open(tmp_path):
    """Older versions stored the full path. Opening must still find the copy by name."""
    from vfp.workspace import Workspace
    ws = Workspace.init(tmp_path / "ws")
    (ws.home / "originals" / "1_x.png").write_bytes(b"data")
    assert ws.original_path(r"D:\somewhere\old\ws\originals\1_x.png") == (ws.home / "originals" / "1_x.png").resolve()
    assert ws.original_path(r".vfp\originals\1_x.png") == (ws.home / "originals" / "1_x.png").resolve()
    assert ws.original_path("originals/1_x.png") == (ws.home / "originals" / "1_x.png").resolve()
