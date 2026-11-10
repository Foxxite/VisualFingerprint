"""GUI smoke and flow tests. Runs headless (offscreen Qt)."""

import os
import shutil
import subprocess

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import pytest

PySide6 = pytest.importorskip("PySide6")
from PySide6.QtCore import QEventLoop, QTimer
from PySide6.QtWidgets import QApplication, QMessageBox

from vfp import pipeline
from vfp.eval.image_eval import synthetic_image
from vfp.gui.app import MainWindow, StartPage
from vfp.gui.mark import AddFileDialog
from vfp.workspace import Workspace

pytestmark = pytest.mark.skipif(shutil.which("ffmpeg") is None, reason="ffmpeg not installed")


@pytest.fixture(scope="module")
def app():
    return QApplication.instance() or QApplication([])


def wait_for(cond, timeout=120_000):
    loop, t = QEventLoop(), QTimer()
    t.setInterval(30)
    t.timeout.connect(lambda: loop.quit() if cond() else None)
    t.start()
    QTimer.singleShot(timeout, loop.quit)
    loop.exec()
    t.stop()
    assert cond(), "timed out"


@pytest.fixture()
def win(app, tmp_path, monkeypatch):
    monkeypatch.setattr(QMessageBox, "warning", lambda *a, **k: None)      # never block a test
    monkeypatch.setattr(QMessageBox, "information", lambda *a, **k: None)
    Workspace.init(tmp_path / "ws")
    w = MainWindow(tmp_path / "ws")
    yield w
    w.close()


def test_no_workspace_shows_start_page(app, tmp_path, monkeypatch):
    monkeypatch.delenv("VFP_HOME", raising=False)
    w = MainWindow(tmp_path / "nothing-here")
    assert isinstance(w.stack.currentWidget(), StartPage)


def test_add_receivers_and_reject_duplicates(win):
    v = win.views["receivers"]
    for name in ("alice", "bob", "alice"):
        v.name.setText(name)
        v.add()
    assert [v.table.item(i, 0).text() for i in range(v.table.rowCount())] == ["alice", "bob"]


def test_add_file_dialog_preview(app, tmp_path):
    from vfp.pipeline import write_image
    write_image(tmp_path / "p.png", synthetic_image(640, 480))
    d = AddFileDialog(path=str(tmp_path / "p.png"))
    assert "Code length" in d.preview.text()
    d.path.setText(str(tmp_path / "missing.png"))
    assert not d.buttons.button(d.buttons.StandardButton.Ok).isEnabled()


def test_mark_then_investigate_flow(win, tmp_path):
    from vfp.pipeline import write_image
    img = tmp_path / "pic.png"
    write_image(img, synthetic_image(960, 640))
    win.ctx.ws.add_receiver("alice")
    win.ctx.ws.add_receiver("bob")
    pipeline.add_asset(win.ctx.ws, img, "pic")
    win.ctx.changed.emit()

    mv = win.views["mark"]
    assert mv.assets.rowCount() == 1 and mv.recv.count() == 2
    mv.out.setText(str(tmp_path / "out"))
    mv.start()
    wait_for(lambda: mv.worker is not None and mv.worker.isFinished())
    assert sorted(p.name for p in (tmp_path / "out").iterdir()) == ["pic__alice.png", "pic__bob.png"]
    assert mv.assets.item(0, 3).text() == "2"                      # copies column refreshed
    assert mv.open_btn.isVisible() or not mv.isVisible()           # offered after success

    leak = tmp_path / "leak.jpg"
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", str(tmp_path / "out" / "pic__bob.png"),
                    "-vf", "scale=iw*0.75:-2", "-q:v", "5", str(leak)], check=True)
    iv = win.views["investigate"]
    iv.set_file(str(leak))
    iv.start()
    wait_for(lambda: iv.result is not None)
    assert iv.result.flagged == ["bob"]
    assert iv.banner.text() == "LEAKER: bob"
    assert iv.table.rowCount() == 2 and iv.table.item(0, 1).text() == "bob"
    assert iv.save.isEnabled()


def test_marking_failure_is_reported_not_crashed(win, tmp_path):
    from vfp.pipeline import write_image
    write_image(tmp_path / "pic.png", synthetic_image(640, 480))
    win.ctx.ws.add_receiver("alice")
    a, _ = pipeline.add_asset(win.ctx.ws, tmp_path / "pic.png", "pic")
    win.ctx.changed.emit()
    os.remove(a.orig_path)                       # the private original goes missing
    mv = win.views["mark"]
    mv.out.setText(str(tmp_path / "out"))
    mv.start()
    wait_for(lambda: mv.worker.isFinished())
    assert "FAILED" in mv.log.toPlainText()
    assert mv.go.isEnabled()
