from __future__ import annotations

from pathlib import Path

from PySide6.QtCore import QRectF, Qt
from PySide6.QtGui import QColor, QPainter, QPen
from PySide6.QtWidgets import (QAbstractItemView, QComboBox, QFileDialog, QHBoxLayout, QHeaderView,
                               QLabel, QMessageBox, QProgressBar, QPushButton, QSizePolicy,
                               QStyledItemDelegate, QTableWidget, QTableWidgetItem, QVBoxLayout, QWidget)

from .. import pipeline
from .common import Context, Worker

ROLE_SCORE = Qt.ItemDataRole.UserRole + 1

# Banner colors are fixed (background and text together), so they read well in light and dark themes.
BANNERS = {
    "leaker": ("#fde2e2", "#7a1010"),
    "collusion": ("#ffe8cc", "#7a3b00"),
    "inconclusive": ("#e8eaed", "#2b2f36"),
    "idle": ("#e8eaed", "#2b2f36"),
}


class ScoreDelegate(QStyledItemDelegate):
    """Draws a bar for the score and a line where the accusation threshold is."""

    def paint(self, p: QPainter, opt, index):
        data = index.data(ROLE_SCORE)
        if not data:
            return super().paint(p, opt, index)
        score, thr, top = data
        r = QRectF(opt.rect).adjusted(6, 6, -60, -6)
        scale = max(top, thr * 1.25, 1.0)
        p.save()
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        p.setPen(Qt.PenStyle.NoPen)
        p.setBrush(QColor(128, 128, 128, 45))
        p.drawRoundedRect(r, 3, 3)
        w = max(0.0, min(1.0, score / scale)) * r.width()
        p.setBrush(QColor("#c0392b") if score > thr else QColor("#7f8c9a"))
        p.drawRoundedRect(QRectF(r.left(), r.top(), w, r.height()), 3, 3)
        x = r.left() + min(1.0, thr / scale) * r.width()
        p.setPen(QPen(opt.palette.text().color(), 1.5, Qt.PenStyle.DashLine))
        p.drawLine(int(x), int(r.top() - 3), int(x), int(r.bottom() + 3))
        p.setPen(opt.palette.text().color())
        p.drawText(QRectF(opt.rect.right() - 56, opt.rect.top(), 52, opt.rect.height()),
                   int(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter), f"{score:.0f}")
        p.restore()

    def sizeHint(self, opt, index):
        s = super().sizeHint(opt, index)
        s.setHeight(max(s.height(), 30))
        return s


class InvestigateView(QWidget):
    def __init__(self, ctx: Context):
        super().__init__()
        self.ctx = ctx
        self.worker: Worker | None = None
        self.result = None
        self.leaked: Path | None = None
        self.setAcceptDrops(True)

        lay = QVBoxLayout(self)
        lay.addWidget(QLabel("Found a leaked file? Choose it (or drop it here) to see who it came from."))

        row = QHBoxLayout()
        self.file_lbl = QLabel("No file chosen")
        self.file_lbl.setTextInteractionFlags(Qt.TextInteractionFlag.TextSelectableByMouse)
        # A long path must not widen the window: let the label shrink and show the tail.
        self.file_lbl.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Preferred)
        self.file_lbl.setMinimumWidth(80)
        self.choose = QPushButton("Choose leaked file…")
        self.choose.clicked.connect(self.pick)
        self.asset = QComboBox()
        self.go = QPushButton("Trace")
        self.go.setEnabled(False)
        self.go.clicked.connect(self.start)
        row.addWidget(self.choose)
        row.addWidget(self.file_lbl, 1)
        row.addWidget(QLabel("Compare with"))
        row.addWidget(self.asset)
        row.addWidget(self.go)
        lay.addLayout(row)

        self.bar = QProgressBar()
        self.bar.setRange(0, 0)
        self.bar.setVisible(False)
        lay.addWidget(self.bar)

        self.banner = QLabel("Result will appear here.")
        self.banner.setWordWrap(True)
        self.banner.setMargin(12)
        self._style_banner("idle")
        lay.addWidget(self.banner)

        self.warn = QLabel()
        self.warn.setWordWrap(True)
        self.warn.setVisible(False)
        lay.addWidget(self.warn)

        self.table = QTableWidget(0, 3)
        self.table.setHorizontalHeaderLabels(["Rank", "Receiver", "Score  (dashed line = threshold)"])
        h = self.table.horizontalHeader()
        h.setSectionResizeMode(0, QHeaderView.ResizeMode.ResizeToContents)
        h.setSectionResizeMode(1, QHeaderView.ResizeMode.ResizeToContents)
        h.setSectionResizeMode(2, QHeaderView.ResizeMode.Stretch)
        self.table.setItemDelegateForColumn(2, ScoreDelegate(self.table))
        self.table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.setSelectionMode(QAbstractItemView.SelectionMode.NoSelection)
        self.table.verticalHeader().setVisible(False)
        lay.addWidget(self.table, 1)

        self.evidence = QLabel()
        self.evidence.setWordWrap(True)
        lay.addWidget(self.evidence)
        foot = QHBoxLayout()
        self.save = QPushButton("Save report…")
        self.save.setEnabled(False)
        self.save.clicked.connect(self.save_report)
        foot.addWidget(QLabel("A score is statistical evidence, not proof."), 1)
        foot.addWidget(self.save)
        lay.addLayout(foot)

        ctx.changed.connect(self.refresh_assets)
        self.refresh_assets()

    # ---- helpers
    def _style_banner(self, kind: str):
        bg, fg = BANNERS[kind]
        self.banner.setStyleSheet(f"background:{bg}; color:{fg}; border-radius:6px; font-size:15px; font-weight:600;")

    def refresh_assets(self):
        cur = self.asset.currentData()
        self.asset.clear()
        self.asset.addItem("Find automatically", None)
        for a, n in self.ctx.ws.asset_stats():
            if n:
                self.asset.addItem(f"{a.name} ({a.kind})", a.id)
        i = self.asset.findData(cur)
        self.asset.setCurrentIndex(max(i, 0))

    def set_file(self, path: str):
        self.leaked = Path(path)
        self.file_lbl.setText(self.leaked.name)
        self.file_lbl.setToolTip(str(self.leaked))
        self.go.setEnabled(True)

    def pick(self):
        p, _ = QFileDialog.getOpenFileName(self, "Choose the leaked file")
        if p:
            self.set_file(p)

    def dragEnterEvent(self, e):
        if e.mimeData().hasUrls():
            e.acceptProposedAction()

    def dropEvent(self, e):
        urls = e.mimeData().urls()
        if urls:
            self.set_file(urls[0].toLocalFile())
            self.start()

    # ---- run
    def start(self):
        if not self.leaked or (self.worker and self.worker.isRunning()):
            return
        leaked, asset = self.leaked, self.asset.currentData()
        self.go.setEnabled(False)
        self.bar.setVisible(True)
        self._style_banner("idle")
        self.banner.setText("Reading the hidden code… this can take a minute for long videos.")

        def job(w: Worker):
            with self.ctx.new_workspace_handle() as ws:
                return pipeline.trace_leak(ws, leaked, asset)

        self.worker = Worker(job, self)
        self.worker.succeeded.connect(self._done)
        self.worker.failed.connect(self._failed)
        self.worker.start()

    def _failed(self, msg):
        self.bar.setVisible(False)
        self.go.setEnabled(True)
        self._style_banner("idle")
        self.banner.setText("Could not read this file.")
        QMessageBox.warning(self, "Trace failed", msg)

    def _done(self, r):
        self.result = r
        self.bar.setVisible(False)
        self.go.setEnabled(True)
        self.save.setEnabled(True)
        kind = ("leaker" if len(r.flagged) == 1 else "collusion" if r.flagged else "inconclusive")
        self._style_banner(kind)
        self.banner.setText(r.verdict)
        self.warn.setVisible(bool(r.warnings))
        self.warn.setText("\n".join("⚠ " + w for w in r.warnings))
        top = r.ranking[0][1] if r.ranking else 1.0
        self.table.setRowCount(len(r.ranking))
        for i, (name, score) in enumerate(r.ranking):
            self.table.setItem(i, 0, QTableWidgetItem(str(i + 1)))
            self.table.setItem(i, 1, QTableWidgetItem(name))
            it = QTableWidgetItem()
            it.setData(ROLE_SCORE, (score, r.threshold, top))
            self.table.setItem(i, 2, it)
        self.evidence.setText(f"Matched file: {r.asset} ({r.kind})   ·   threshold {r.threshold:.0f}   ·   "
                              f"evidence: {r.evidence}")

    def save_report(self):
        if not self.result:
            return
        p, _ = QFileDialog.getSaveFileName(self, "Save report", f"trace-report-{self.leaked.stem}.txt",
                                           "Text (*.txt)")
        if p:
            Path(p).write_text(pipeline.report_text(self.result, self.leaked), encoding="utf-8")
