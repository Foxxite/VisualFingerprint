"""Shared GUI pieces: app context and the background worker."""

from __future__ import annotations

from pathlib import Path

from PySide6.QtCore import QObject, QThread, Signal

from ..workspace import Workspace


class Context(QObject):
    """The open workspace. Views listen to `changed` and refresh."""
    changed = Signal()

    def __init__(self, home: Path):
        super().__init__()
        self.home = Path(home)
        self.ws = Workspace(self.home)       # for the GUI thread only

    def new_workspace_handle(self) -> Workspace:
        """Workers need their own database connection."""
        return Workspace(self.home)


class Worker(QThread):
    """Run a function in a thread. fn(worker) may call worker.log(text) and worker.step(i, n)."""
    logged = Signal(str)
    stepped = Signal(int, int)
    succeeded = Signal(object)
    failed = Signal(str)

    def __init__(self, fn, parent=None):
        super().__init__(parent)
        self.fn = fn

    def log(self, text: str):
        self.logged.emit(text)

    def step(self, i: int, n: int):
        self.stepped.emit(i, n)

    def run(self):
        try:
            result = self.fn(self)
        except Exception as e:                     # shown to the user, not swallowed
            self.failed.emit(f"{type(e).__name__}: {e}")
        else:
            self.succeeded.emit(result)
