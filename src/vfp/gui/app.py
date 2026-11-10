"""VisualFingerprint desktop app."""

from __future__ import annotations

import os
import sys
from pathlib import Path

from PySide6.QtCore import QSettings, QUrl, Qt
from PySide6.QtGui import QAction, QDesktopServices
from PySide6.QtWidgets import (QApplication, QFileDialog, QLabel, QMainWindow, QMessageBox,
                               QPushButton, QStackedWidget, QTabWidget, QVBoxLayout, QWidget)

from .. import __version__
from ..workspace import Workspace
from .common import Context
from .investigate import InvestigateView
from .mark import MarkView
from .receivers import ReceiversView


class StartPage(QWidget):
    def __init__(self, window: "MainWindow"):
        super().__init__()
        lay = QVBoxLayout(self)
        lay.addStretch(1)
        title = QLabel("VisualFingerprint")
        title.setStyleSheet("font-size:26px; font-weight:700;")
        title.setAlignment(Qt.AlignmentFlag.AlignCenter)
        sub = QLabel("Give each receiver a copy with a hidden code.\nIf a copy leaks, find out whose it was.")
        sub.setAlignment(Qt.AlignmentFlag.AlignCenter)
        new_b, open_b = QPushButton("Create a new workspace…"), QPushButton("Open an existing workspace…")
        new_b.clicked.connect(window.new_workspace)
        open_b.clicked.connect(window.open_workspace_dialog)
        for w in (title, sub, new_b, open_b):
            lay.addWidget(w, alignment=Qt.AlignmentFlag.AlignCenter)
        lay.addStretch(2)


class SettingsView(QWidget):
    def __init__(self, ctx: Context):
        super().__init__()
        lay = QVBoxLayout(self)
        info = QLabel(
            f"<b>Workspace</b><br>{ctx.home.resolve()}<br><br>"
            f"<b>Secret key</b><br>{(ctx.home / 'master.key').resolve()}<br><br>"
            "<b>Back up the secret key.</b> Every hidden code comes from it. Without it you cannot "
            "trace a leak. Never share it, and keep it away from the marked copies.<br><br>"
            "The folder <i>originals</i> holds private copies of your unmarked files. Tracing needs them. "
            f"Keep them safe too.<br><br>VisualFingerprint {__version__}")
        info.setWordWrap(True)
        info.setTextInteractionFlags(Qt.TextInteractionFlag.TextSelectableByMouse)
        btn = QPushButton("Open workspace folder")
        btn.clicked.connect(lambda: QDesktopServices.openUrl(QUrl.fromLocalFile(str(ctx.home.resolve()))))
        lay.addWidget(info)
        lay.addWidget(btn, alignment=Qt.AlignmentFlag.AlignLeft)
        lay.addStretch(1)


class MainWindow(QMainWindow):
    def __init__(self, home: str | Path | None = None):
        super().__init__()
        self.setWindowTitle("VisualFingerprint")
        self.resize(1040, 700)
        self.settings = QSettings("VisualFingerprint", "vfp")
        self.ctx: Context | None = None
        self.stack = QStackedWidget()
        self.setCentralWidget(self.stack)
        self.stack.addWidget(StartPage(self))

        m = self.menuBar().addMenu("&Workspace")
        for text, fn in (("New workspace…", self.new_workspace), ("Open workspace…", self.open_workspace_dialog)):
            act = QAction(text, self)
            act.triggered.connect(fn)
            m.addAction(act)

        home = home or os.environ.get("VFP_HOME") or self.settings.value("last_home")
        if home and (Path(home) / "vfp.db").exists():
            self.open_workspace(home)

    def new_workspace(self):
        d = QFileDialog.getExistingDirectory(self, "Choose an empty folder for the new workspace")
        if not d:
            return
        if (Path(d) / "vfp.db").exists():
            QMessageBox.information(self, "Already a workspace", "That folder already holds a workspace. Opening it.")
        else:
            Workspace.init(d).close()
        self.open_workspace(d)

    def open_workspace_dialog(self):
        d = QFileDialog.getExistingDirectory(self, "Open a workspace folder")
        if d:
            if not (Path(d) / "vfp.db").exists():
                QMessageBox.warning(self, "Not a workspace", "That folder has no workspace (vfp.db) in it.")
                return
            self.open_workspace(d)

    def open_workspace(self, home):
        self.ctx = Context(Path(home))
        self.settings.setValue("last_home", str(Path(home).resolve()))
        tabs = QTabWidget()
        self.views = dict(receivers=ReceiversView(self.ctx), mark=MarkView(self.ctx),
                          investigate=InvestigateView(self.ctx), settings=SettingsView(self.ctx))
        tabs.addTab(self.views["receivers"], "Receivers")
        tabs.addTab(self.views["mark"], "Make copies")
        tabs.addTab(self.views["investigate"], "Investigate a leak")
        tabs.addTab(self.views["settings"], "Workspace")
        self.tabs = tabs
        while self.stack.count() > 1:
            w = self.stack.widget(1)
            self.stack.removeWidget(w)
            w.deleteLater()
        self.stack.addWidget(tabs)
        self.stack.setCurrentWidget(tabs)
        self.setWindowTitle(f"VisualFingerprint — {Path(home).resolve().name}")


def main(argv=None) -> int:
    app = QApplication(argv or sys.argv)
    win = MainWindow()
    win.show()
    return app.exec()


if __name__ == "__main__":
    sys.exit(main())
