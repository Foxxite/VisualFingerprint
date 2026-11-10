"""Command line: vfp init | receivers | asset | mark | trace"""

from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import asdict
from pathlib import Path

from . import pipeline
from .workspace import Workspace


def home_dir(args) -> Path:
    return Path(args.home or os.environ.get("VFP_HOME") or ".vfp")


def err(msg: str):
    print(msg, file=sys.stderr)


_opened: list[Workspace] = []


def open_ws(args) -> Workspace:
    """Open the workspace. It is closed when the command ends (see main)."""
    ws = Workspace(home_dir(args))
    _opened.append(ws)
    return ws


def cmd_init(a):
    home = home_dir(a)
    existed = (home / "master.key").exists()
    Workspace.init(home).close()
    print(f"Workspace ready: {home.resolve()}")
    if not existed:
        print(f"A new secret key was made: {(home / 'master.key').resolve()}")
    print("BACK UP master.key. Without it you cannot trace a leak. Never share it.")


def cmd_receivers(a):
    ws = open_ws(a)
    if a.action == "add":
        for n in a.names:
            ws.add_receiver(n, a.note or "")
            print(f"added receiver: {n}")
    elif a.action == "import":
        for line in Path(a.names[0]).read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            name, _, note = line.partition(",")
            ws.add_receiver(name.strip(), note.strip())
            print(f"added receiver: {name.strip()}")
    else:
        for r in ws.receivers():
            print(f"{r['id']:>4}  {r['name']:<30} {r['note']}")


def cmd_asset(a):
    ws = open_ws(a)
    if a.action == "add":
        asset, plan = pipeline.add_asset(ws, a.file, a.name, a.max_receivers, a.colluders,
                                         a.eps, a.strength, a.crf, a.slots)
        print(f"registered asset '{asset.name}' (id {asset.id}, {asset.kind})")
        _print_plan(plan, asset.max_receivers, asset.eps)
    elif a.action == "plan":
        plan = pipeline.plan_file(a.file, a.max_receivers, a.eps, a.strength, a.crf, a.slots, a.colluders)
        _print_plan(plan, a.max_receivers, a.eps)
    else:
        for x in ws.assets():
            n = len(ws.issues(x.id))
            print(f"{x.id:>4}  {x.name:<30} {x.kind:<6} symbols={x.m:<6} copies issued={n}")


def _print_plan(plan, max_receivers, eps):
    print(f"  code length: {plan.m} symbols")
    print(f"  planned for up to {max_receivers} receivers, false-accusation rate {eps:g}")
    g = plan.guaranteed_colluders
    print(f"  guaranteed against colluder groups of up to: {g if g else 'none (single leaker only)'}")
    for n in plan.notes:
        print(f"  NOTE: {n}")


def cmd_mark(a):
    ws = open_ws(a)
    asset = ws.asset(a.asset)
    targets = [r["name"] for r in ws.receivers()] if a.all else a.to
    if not targets:
        err("Say who gets a copy: --to NAME [NAME ...] or --all")
        return 2
    outdir = Path(a.out)
    for name in targets:
        print(f"marking for {name} ...", flush=True)
        out = pipeline.mark_for_receiver(ws, asset, name, outdir,
                                         progress=(lambda s: print(s, end="\r", file=sys.stderr))
                                         if asset.kind == "video" else None)
        print(f"  -> {out}")


def cmd_trace(a):
    ws = open_ws(a)
    r = pipeline.trace_leak(ws, a.file, a.asset)
    if a.json:
        print(json.dumps(asdict(r), indent=2))
        return 0
    print(f"asset: {r.asset} ({r.kind})   accusation threshold: {r.threshold:.1f}")
    print(f"evidence: {r.evidence}")
    for w in r.warnings:
        print(f"WARNING: {w}")
    print(f"\n{'rank':>4}  {'receiver':<30}{'score':>9}  ")
    for i, (n, s) in enumerate(r.ranking[: a.top], 1):
        print(f"{i:>4}  {n:<30}{s:>9.1f}  {'<-- above threshold' if s > r.threshold else ''}")
    print(f"\n{r.verdict}")
    print("A score is statistical evidence, not proof. Keep the leaked file and this report.")
    return 0


def cmd_gui(a):
    try:
        from .gui.app import MainWindow
        from PySide6.QtWidgets import QApplication
    except ImportError:
        err("The GUI needs PySide6. Install it with: pip install -e .[gui]")
        return 1
    app = QApplication(sys.argv[:1])
    win = MainWindow(home_dir(a) if (home_dir(a) / "vfp.db").exists() else None)
    win.show()
    return app.exec()


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="vfp", description="Per-receiver invisible watermarking and leak tracing")
    p.add_argument("--home", help="workspace folder (default: $VFP_HOME or ./.vfp)")
    sub = p.add_subparsers(dest="cmd", required=True)

    sub.add_parser("init", help="create a workspace and a secret key").set_defaults(fn=cmd_init)
    sub.add_parser("gui", help="open the desktop app").set_defaults(fn=cmd_gui)

    r = sub.add_parser("receivers", help="add, import or list receivers")
    r.add_argument("action", choices=["add", "import", "list"])
    r.add_argument("names", nargs="*", help="names to add, or a file for import (one 'name[,note]' per line)")
    r.add_argument("--note")
    r.set_defaults(fn=cmd_receivers)

    s = sub.add_parser("asset", help="register an original file, plan it, or list assets")
    s.add_argument("action", choices=["add", "plan", "list"])
    s.add_argument("file", nargs="?")
    s.add_argument("--name")
    s.add_argument("--max-receivers", type=int, default=1000)
    s.add_argument("--colluders", type=int, default=3, help="design size of a colluder group")
    s.add_argument("--eps", type=float, default=1e-3, help="false-accusation rate")
    s.add_argument("--strength", type=float, help="mark strength (image/video; for audio: alpha)")
    s.add_argument("--slots", type=int, help="symbols per image, or per segment for audio and video "
                   "(default: chosen automatically; short files pack more symbols)")
    s.add_argument("--crf", type=int, help="video only: x264 quality of marked copies "
                   "(default 16; higher = smaller file, lower fidelity)")
    s.set_defaults(fn=cmd_asset)

    m = sub.add_parser("mark", help="make marked copies for receivers")
    m.add_argument("asset")
    m.add_argument("--to", nargs="*", default=[])
    m.add_argument("--all", action="store_true")
    m.add_argument("--out", default="marked")
    m.set_defaults(fn=cmd_mark)

    t = sub.add_parser("trace", help="find who leaked a file")
    t.add_argument("file")
    t.add_argument("--asset", help="asset name or id (default: try all of the same type)")
    t.add_argument("--top", type=int, default=5)
    t.add_argument("--json", action="store_true")
    t.set_defaults(fn=cmd_trace)
    return p


def main(argv=None) -> int:
    a = build_parser().parse_args(argv)
    if a.cmd == "asset" and a.action in ("add", "plan") and not a.file:
        err("asset add/plan needs a file")
        return 2
    try:
        return a.fn(a) or 0
    except (KeyError, ValueError, FileNotFoundError) as e:
        err(f"error: {e}")
        return 1
    finally:
        while _opened:
            _opened.pop().close()


if __name__ == "__main__":
    sys.exit(main())
