from __future__ import annotations

from pathlib import Path

from PySide6.QtCore import Qt, QUrl
from PySide6.QtGui import QDesktopServices
from PySide6.QtWidgets import (QAbstractItemView, QComboBox, QDialog, QDialogButtonBox,
                               QDoubleSpinBox, QFileDialog, QFormLayout, QGroupBox, QHBoxLayout,
                               QHeaderView, QLabel, QLineEdit, QListWidget, QListWidgetItem,
                               QMessageBox, QPlainTextEdit, QProgressBar, QPushButton, QSpinBox,
                               QSplitter, QTableWidget, QTableWidgetItem, QVBoxLayout, QWidget)

from .. import pipeline
from .common import Context, Worker

FILE_FILTER = ("Media (*.jpg *.jpeg *.png *.webp *.bmp *.tif *.tiff *.mp4 *.mov *.mkv *.webm *.avi *.m4v "
               "*.mp3 *.wav *.flac *.m4a *.ogg *.opus *.aac);;All files (*)")
EPS_CHOICES = [("1 in 100", 1e-2), ("1 in 1,000", 1e-3), ("1 in 10,000", 1e-4), ("1 in 1,000,000", 1e-6)]


class AddFileDialog(QDialog):
    """Pick an original file, choose settings, see what the file can carry before registering."""

    def __init__(self, parent=None, path: str = ""):
        super().__init__(parent)
        self.setWindowTitle("Add an original file")
        self.setMinimumWidth(560)
        form = QFormLayout()

        row = QHBoxLayout()
        self.path = QLineEdit(path)
        browse = QPushButton("Browse…")
        browse.clicked.connect(self._browse)
        row.addWidget(self.path, 1)
        row.addWidget(browse)
        form.addRow("File", row)

        self.name = QLineEdit()
        self.name.setPlaceholderText("Defaults to the file name")
        form.addRow("Name", self.name)

        self.max_receivers = QSpinBox(minimum=2, maximum=1_000_000, value=1000)
        self.colluders = QSpinBox(minimum=1, maximum=20, value=3)
        self.eps = QComboBox()
        for label, v in EPS_CHOICES:
            self.eps.addItem(label, v)
        self.eps.setCurrentIndex(1)
        self.strength = QDoubleSpinBox(minimum=0.0, maximum=20.0, singleStep=0.1, decimals=2)
        self.strength.setSpecialValueText("Default")
        self.strength.setToolTip("Higher = survives more damage but is easier to see or hear.")
        self.slots = QSpinBox(minimum=0, maximum=4096)
        self.slots.setSpecialValueText("Automatic")
        self.slots.setToolTip("Per image, or per segment for audio and video. More symbols make a group "
                              "of colluders easier to catch, but each symbol gets less of the file. "
                              "Automatic packs more into short files.")
        self.crf = QSpinBox(minimum=0, maximum=40)
        self.crf.setSpecialValueText("Default (16)")
        self.crf.setToolTip("Video only. Higher = smaller marked file, lower fidelity.")
        form.addRow("Most receivers", self.max_receivers)
        form.addRow("Plan for colluder groups of", self.colluders)
        form.addRow("Wrongly accused", self.eps)
        form.addRow("Mark strength", self.strength)
        form.addRow("Video quality (CRF)", self.crf)
        form.addRow("Symbols per image / segment", self.slots)

        self.preview = QLabel("Choose a file to see how much it can carry.")
        self.preview.setWordWrap(True)
        self.preview.setTextFormat(Qt.TextFormat.PlainText)
        box = QGroupBox("What this file can carry")
        QVBoxLayout(box).addWidget(self.preview)

        self.buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        self.buttons.button(QDialogButtonBox.StandardButton.Ok).setText("Register file")
        self.buttons.accepted.connect(self.accept)
        self.buttons.rejected.connect(self.reject)

        lay = QVBoxLayout(self)
        lay.addLayout(form)
        lay.addWidget(box)
        lay.addWidget(self.buttons)

        for w in (self.max_receivers, self.colluders, self.strength, self.crf, self.slots):
            w.valueChanged.connect(self.update_preview)
        self.eps.currentIndexChanged.connect(self.update_preview)
        self.path.textChanged.connect(self.update_preview)
        self.update_preview()

    def _browse(self):
        p, _ = QFileDialog.getOpenFileName(self, "Choose an original file", "", FILE_FILTER)
        if p:
            self.path.setText(p)

    def options(self) -> dict:
        return dict(max_receivers=self.max_receivers.value(), n_colluders=self.colluders.value(),
                    eps=self.eps.currentData(), strength=self.strength.value() or None,
                    crf=self.crf.value() or None, slots=self.slots.value() or None)

    def update_preview(self):
        ok = self.buttons.button(QDialogButtonBox.StandardButton.Ok)
        p = self.path.text().strip()
        self.crf.setEnabled(p.lower().endswith(tuple(pipeline.VIDEO_EXT)))
        if not p or not Path(p).is_file():
            self.preview.setText("Choose a file to see how much it can carry.")
            ok.setEnabled(False)
            return
        try:
            o = self.options()
            plan = pipeline.plan_file(p, o["max_receivers"], o["eps"], o["strength"], o["crf"], o["slots"],
                                      o["n_colluders"])
        except Exception as e:
            self.preview.setText(f"This file cannot be used: {e}")
            ok.setEnabled(False)
            return
        g = plan.guaranteed_colluders
        text = (f"Type: {plan.kind}\nCode length: {plan.m} symbols\n"
                f"Guaranteed against colluder groups of up to: {g if g else 'none (one leaker only)'}")
        if plan.notes:
            text += "\n\n" + "\n".join(plan.notes)
        self.preview.setText(text)
        ok.setEnabled(True)


class MarkView(QWidget):
    def __init__(self, ctx: Context):
        super().__init__()
        self.ctx = ctx
        self.worker: Worker | None = None
        self.last_outdir: Path | None = None

        # ---- left: files
        left = QGroupBox("1. Original files")
        ll = QVBoxLayout(left)
        self.assets = QTableWidget(0, 4)
        self.assets.setHorizontalHeaderLabels(["Name", "Type", "Symbols", "Copies"])
        self.assets.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)
        self.assets.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.assets.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.assets.setSelectionMode(QAbstractItemView.SelectionMode.SingleSelection)
        self.assets.verticalHeader().setVisible(False)
        self.add_btn = QPushButton("Add file…")
        self.add_btn.clicked.connect(lambda: self.add_file())
        ll.addWidget(self.assets, 1)
        ll.addWidget(self.add_btn)

        # ---- right: receivers + output
        right = QGroupBox("2. Who gets a copy")
        rl = QVBoxLayout(right)
        self.recv = QListWidget()
        sel = QHBoxLayout()
        all_b, none_b = QPushButton("Select all"), QPushButton("Select none")
        all_b.clicked.connect(lambda: self._set_all(Qt.CheckState.Checked))
        none_b.clicked.connect(lambda: self._set_all(Qt.CheckState.Unchecked))
        sel.addWidget(all_b)
        sel.addWidget(none_b)
        sel.addStretch(1)
        out_row = QHBoxLayout()
        self.out = QLineEdit(str(Path.cwd() / "marked"))
        out_b = QPushButton("Browse…")
        out_b.clicked.connect(self._pick_out)
        out_row.addWidget(QLabel("Save to"))
        out_row.addWidget(self.out, 1)
        out_row.addWidget(out_b)
        self.go = QPushButton("Make marked copies")
        self.go.setDefault(True)
        self.go.clicked.connect(self.start)
        self.bar = QProgressBar()
        self.bar.setVisible(False)
        self.log = QPlainTextEdit(readOnly=True)
        self.log.setMaximumBlockCount(500)
        self.open_btn = QPushButton("Open output folder")
        self.open_btn.setVisible(False)
        self.open_btn.clicked.connect(self.open_out)
        rl.addWidget(self.recv, 1)
        rl.addLayout(sel)
        rl.addLayout(out_row)
        rl.addWidget(self.go)
        rl.addWidget(self.bar)
        rl.addWidget(self.log, 1)
        rl.addWidget(self.open_btn)

        split = QSplitter()
        split.addWidget(left)
        split.addWidget(right)
        split.setSizes([420, 520])
        lay = QVBoxLayout(self)
        lay.addWidget(QLabel("Register an original, pick receivers, and make one uniquely marked copy for each."))
        lay.addWidget(split, 1)
        ctx.changed.connect(self.refresh)
        self.refresh()

    # ---- data
    def refresh(self):
        stats = self.ctx.ws.asset_stats()
        keep = self.selected_asset_id()
        self.assets.setRowCount(len(stats))
        for i, (a, n) in enumerate(stats):
            for c, v in enumerate((a.name, a.kind, str(a.m), str(n))):
                it = QTableWidgetItem(v)
                if c == 0:
                    it.setData(Qt.ItemDataRole.UserRole, a.id)
                self.assets.setItem(i, c, it)
            if a.id == keep or (keep is None and i == len(stats) - 1):
                self.assets.selectRow(i)
        checked = {self.recv.item(i).text(): self.recv.item(i).checkState() for i in range(self.recv.count())}
        self.recv.clear()
        for r in self.ctx.ws.receivers():
            it = QListWidgetItem(r["name"])
            it.setFlags(it.flags() | Qt.ItemFlag.ItemIsUserCheckable)
            it.setCheckState(checked.get(r["name"], Qt.CheckState.Checked))
            self.recv.addItem(it)

    def selected_asset_id(self):
        rows = self.assets.selectionModel().selectedRows() if self.assets.selectionModel() else []
        return self.assets.item(rows[0].row(), 0).data(Qt.ItemDataRole.UserRole) if rows else None

    def checked_receivers(self) -> list[str]:
        return [self.recv.item(i).text() for i in range(self.recv.count())
                if self.recv.item(i).checkState() == Qt.CheckState.Checked]

    def _set_all(self, state):
        for i in range(self.recv.count()):
            self.recv.item(i).setCheckState(state)

    def _pick_out(self):
        d = QFileDialog.getExistingDirectory(self, "Save marked copies to", self.out.text())
        if d:
            self.out.setText(d)

    def open_out(self):
        if self.last_outdir:
            QDesktopServices.openUrl(QUrl.fromLocalFile(str(self.last_outdir)))

    # ---- actions
    def add_file(self, path: str = ""):
        dlg = AddFileDialog(self, path)
        if dlg.exec() != QDialog.DialogCode.Accepted:
            return
        o = dlg.options()
        try:
            pipeline.add_asset(self.ctx.ws, dlg.path.text().strip(), dlg.name.text().strip() or None,
                               o["max_receivers"], o["n_colluders"], o["eps"], o["strength"], o["crf"],
                               o["slots"])
        except Exception as e:
            QMessageBox.warning(self, "Cannot add file", str(e))
            return
        self.ctx.changed.emit()

    def start(self):
        aid, names = self.selected_asset_id(), self.checked_receivers()
        if aid is None:
            QMessageBox.information(self, "Choose a file", "Add and select an original file first.")
            return
        if not names:
            QMessageBox.information(self, "Choose receivers", "Tick at least one receiver.")
            return
        outdir = Path(self.out.text().strip())
        self.last_outdir = outdir
        self.log.clear()
        self.bar.setRange(0, len(names))
        self.bar.setValue(0)
        self.bar.setVisible(True)
        self.open_btn.setVisible(False)
        self.go.setEnabled(False)

        def job(w: Worker):
            ws = self.ctx.new_workspace_handle()
            asset = ws.asset(aid)
            outs = []
            for i, name in enumerate(names):
                w.log(f"Marking for {name} …")
                outs.append(pipeline.mark_for_receiver(
                    ws, asset, name, outdir,
                    progress=(lambda s: w.log(s)) if asset.kind == "video" else None))
                w.step(i + 1, len(names))
                w.log(f"  saved {outs[-1].name}")
            return outs

        self.worker = Worker(job, self)
        self.worker.logged.connect(self.log.appendPlainText)
        self.worker.stepped.connect(lambda i, n: self.bar.setValue(i))
        self.worker.succeeded.connect(self._done)
        self.worker.failed.connect(self._failed)
        self.worker.start()

    def _done(self, outs):
        self.go.setEnabled(True)
        self.log.appendPlainText(f"\nDone. {len(outs)} copies written to {self.last_outdir}")
        self.open_btn.setVisible(True)
        self.ctx.changed.emit()

    def _failed(self, msg):
        self.go.setEnabled(True)
        self.bar.setVisible(False)
        self.log.appendPlainText(f"\nFAILED: {msg}")
        QMessageBox.warning(self, "Marking failed", msg)
