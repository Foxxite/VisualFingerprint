"""Workspace: the SQLite database and the master key.

Layout of a workspace folder:
    master.key        secret. Without it nobody can trace a leak. Back it up. Never share it.
    vfp.db            receivers, assets, issued copies
    originals/        private copies of the unmarked files (tracing needs them)
"""

from __future__ import annotations

import json
import sqlite3
import time
from dataclasses import dataclass
from pathlib import Path

from .codes.keys import derive_seed, load_or_create_master_key

SCHEMA = """
CREATE TABLE IF NOT EXISTS receivers (
    id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, note TEXT DEFAULT '', created REAL);
CREATE TABLE IF NOT EXISTS assets (
    id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, kind TEXT NOT NULL,
    orig_path TEXT, sha256 TEXT, params TEXT, m INTEGER, max_receivers INTEGER,
    n_colluders INTEGER, eps REAL, created REAL);
CREATE TABLE IF NOT EXISTS issues (
    id INTEGER PRIMARY KEY, asset_id INTEGER NOT NULL, receiver_id INTEGER NOT NULL,
    user_index INTEGER NOT NULL, out_path TEXT, created REAL,
    UNIQUE(asset_id, receiver_id), UNIQUE(asset_id, user_index));
"""


@dataclass
class Asset:
    id: int
    name: str
    kind: str
    orig_path: str
    sha256: str
    params: dict
    m: int
    max_receivers: int
    n_colluders: int
    eps: float


class Workspace:
    def __init__(self, home: str | Path):
        self.home = Path(home)
        self.db_path = self.home / "vfp.db"
        self.key_path = self.home / "master.key"
        if not self.db_path.exists():
            raise FileNotFoundError(f"No workspace at {self.home}. Run: vfp init")
        self.master = load_or_create_master_key(self.key_path)
        self.db = sqlite3.connect(self.db_path)
        self.db.row_factory = sqlite3.Row

    @classmethod
    def init(cls, home: str | Path) -> "Workspace":
        home = Path(home)
        (home / "originals").mkdir(parents=True, exist_ok=True)
        load_or_create_master_key(home / "master.key")
        con = sqlite3.connect(home / "vfp.db")
        con.executescript(SCHEMA)
        con.commit()
        con.close()
        return cls(home)

    # ---- seeds (all derived from the master key; nothing secret is stored in the DB) ----
    def tardos_seed(self, asset_id: int) -> int:
        return derive_seed(self.master, "asset", str(asset_id), "tardos")

    def layout_seed(self, asset_id: int) -> int:
        return derive_seed(self.master, "asset", str(asset_id), "layout")

    # ---- receivers ----
    def add_receiver(self, name: str, note: str = "") -> int:
        cur = self.db.execute("INSERT INTO receivers(name, note, created) VALUES (?,?,?)",
                              (name, note, time.time()))
        self.db.commit()
        return cur.lastrowid

    def receivers(self):
        return self.db.execute("SELECT * FROM receivers ORDER BY id").fetchall()

    def receiver_stats(self):
        """Receivers with the number of copies issued to each."""
        return self.db.execute(
            "SELECT r.id, r.name, r.note, COUNT(i.id) AS copies FROM receivers r "
            "LEFT JOIN issues i ON i.receiver_id = r.id GROUP BY r.id ORDER BY r.id").fetchall()

    def asset_stats(self):
        """Assets with the number of copies issued for each."""
        out = []
        for a in self.assets():
            out.append((a, len(self.issues(a.id))))
        return out

    def receiver(self, name_or_id) -> sqlite3.Row:
        key = str(name_or_id)
        row = self.db.execute("SELECT * FROM receivers WHERE name=? OR id=?",
                              (key, int(key) if key.isdigit() else -1)).fetchone()
        if row is None:
            raise KeyError(f"unknown receiver: {name_or_id}")
        return row

    # ---- assets ----
    def add_asset(self, name, kind, orig_path, sha256, params, m, max_receivers, n_colluders, eps) -> int:
        cur = self.db.execute(
            "INSERT INTO assets(name,kind,orig_path,sha256,params,m,max_receivers,n_colluders,eps,created)"
            " VALUES (?,?,?,?,?,?,?,?,?,?)",
            (name, kind, str(orig_path), sha256, json.dumps(params), m, max_receivers,
             n_colluders, eps, time.time()))
        self.db.commit()
        return cur.lastrowid

    def set_asset_path(self, asset_id: int, path):
        self.db.execute("UPDATE assets SET orig_path=? WHERE id=?", (str(path), asset_id))
        self.db.commit()

    def _asset(self, r) -> Asset:
        return Asset(r["id"], r["name"], r["kind"], r["orig_path"], r["sha256"],
                     json.loads(r["params"]), r["m"], r["max_receivers"], r["n_colluders"], r["eps"])

    def assets(self):
        return [self._asset(r) for r in self.db.execute("SELECT * FROM assets ORDER BY id")]

    def asset(self, name_or_id) -> Asset:
        key = str(name_or_id)
        r = self.db.execute("SELECT * FROM assets WHERE name=? OR id=?",
                            (key, int(key) if key.isdigit() else -1)).fetchone()
        if r is None:
            raise KeyError(f"unknown asset: {name_or_id}")
        return self._asset(r)

    # ---- issued copies ----
    def issue(self, asset_id: int, receiver_id: int, out_path=None) -> int:
        """Return this receiver's code index for this asset. Create it on first use."""
        row = self.db.execute("SELECT user_index FROM issues WHERE asset_id=? AND receiver_id=?",
                              (asset_id, receiver_id)).fetchone()
        if row is None:
            nxt = self.db.execute("SELECT COALESCE(MAX(user_index)+1, 0) FROM issues WHERE asset_id=?",
                                  (asset_id,)).fetchone()[0]
            self.db.execute("INSERT INTO issues(asset_id,receiver_id,user_index,out_path,created)"
                            " VALUES (?,?,?,?,?)", (asset_id, receiver_id, nxt, str(out_path or ""), time.time()))
            self.db.commit()
            return nxt
        if out_path:
            self.db.execute("UPDATE issues SET out_path=? WHERE asset_id=? AND receiver_id=?",
                            (str(out_path), asset_id, receiver_id))
            self.db.commit()
        return row["user_index"]

    def issues(self, asset_id: int):
        return self.db.execute(
            "SELECT i.user_index, i.out_path, r.name FROM issues i JOIN receivers r ON r.id=i.receiver_id"
            " WHERE i.asset_id=? ORDER BY i.user_index", (asset_id,)).fetchall()
