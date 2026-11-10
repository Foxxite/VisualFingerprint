from __future__ import annotations

import sqlite3
from pathlib import Path

from PySide6.QtWidgets import (QFileDialog, QHBoxLayout, QHeaderView, QLabel, QLineEdit,
                               QMessageBox, QPushButton, QTableWidget, QTableWidgetItem,
                               QVBoxLayout, QWidget, QAbstractItemView)

from .common import Context


class ReceiversView(QWidget):
    def __init__(self, ctx: Context):
        super().__init__()
        self.ctx = ctx
        lay = QVBoxLayout(self)
        lay.addWidget(QLabel("Everyone who may get a marked copy. Each receiver gets a unique hidden code."))

        row = QHBoxLayout()
        self.name = QLineEdit(placeholderText="Name (for example: Alice Smith)")
        self.note = QLineEdit(placeholderText="Note (optional)")
        self.add_btn = QPushButton("Add receiver")
        self.import_btn = QPushButton("Import list…")
        self.name.returnPressed.connect(self.add)
        self.add_btn.clicked.connect(self.add)
        self.import_btn.clicked.connect(self.import_file)
        row.addWidget(self.name, 2)
        row.addWidget(self.note, 3)
        row.addWidget(self.add_btn)
        row.addWidget(self.import_btn)
        lay.addLayout(row)

        self.table = QTableWidget(0, 3)
        self.table.setHorizontalHeaderLabels(["Name", "Note", "Copies issued"])
        self.table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)
        self.table.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.Stretch)
        self.table.horizontalHeader().setSectionResizeMode(2, QHeaderView.ResizeMode.ResizeToContents)
        self.table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.table.verticalHeader().setVisible(False)
        lay.addWidget(self.table, 1)
        ctx.changed.connect(self.refresh)
        self.refresh()

    def refresh(self):
        rows = self.ctx.ws.receiver_stats()
        self.table.setRowCount(len(rows))
        for i, r in enumerate(rows):
            for c, v in enumerate((r["name"], r["note"], str(r["copies"]))):
                self.table.setItem(i, c, QTableWidgetItem(v))

    def _add_one(self, name: str, note: str) -> bool:
        try:
            self.ctx.ws.add_receiver(name, note)
            return True
        except sqlite3.IntegrityError:
            QMessageBox.warning(self, "Receiver exists", f"A receiver named “{name}” already exists.")
            return False

    def add(self):
        name = self.name.text().strip()
        if not name:
            return
        if self._add_one(name, self.note.text().strip()):
            self.name.clear()
            self.note.clear()
            self.ctx.changed.emit()

    def import_file(self):
        path, _ = QFileDialog.getOpenFileName(self, "Import receivers", "", "Text or CSV (*.txt *.csv);;All files (*)")
        if not path:
            return
        for line in Path(path).read_text(encoding="utf-8").splitlines():
            if line.strip():
                name, _, note = line.partition(",")
                try:
                    self.ctx.ws.add_receiver(name.strip(), note.strip())
                except sqlite3.IntegrityError:
                    pass                      # skip names that already exist
        self.ctx.changed.emit()
