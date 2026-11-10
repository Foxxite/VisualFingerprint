"""High-level actions: register a file, mark copies for receivers, trace a leak.

The CLI and the GUI both call these functions.
"""

from __future__ import annotations

import hashlib
import re
import shutil
from dataclasses import asdict, dataclass, field
from math import floor, log, pi, sqrt
from pathlib import Path
from typing import Callable

import cv2
import numpy as np

from .audio.common import AudioParams
from .audio.common import capacity as audio_capacity
from .audio.detect import detect_audio
from .audio.embed import embed_audio
from .audio.ffio import probe_audio
from .codes.tardos import TardosCode, accusation_threshold
from .image.common import DEFAULT_SLOTS, ImageParams, usable_slots
from .image.detect import detect_image
from .image.embed import embed_image
from .video.common import VideoParams
from .video.common import capacity as video_capacity
from .video.detect import detect_video
from .video.embed import embed_video
from .video.ffio import probe
from .workspace import Asset, Workspace

IMAGE_EXT = {".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff"}
VIDEO_EXT = {".mp4", ".mov", ".mkv", ".webm", ".avi", ".m4v"}
AUDIO_EXT = {".mp3", ".wav", ".flac", ".m4a", ".ogg", ".opus", ".aac"}

Progress = Callable[[str], None]


def kind_of(path) -> str:
    ext = Path(path).suffix.lower()
    if ext in IMAGE_EXT:
        return "image"
    if ext in VIDEO_EXT:
        return "video"
    if ext in AUDIO_EXT:
        return "audio"
    raise ValueError(f"unsupported file type: {ext}")


def read_image(path) -> np.ndarray:
    img = cv2.imdecode(np.fromfile(str(path), np.uint8), cv2.IMREAD_COLOR)   # works with any path
    if img is None:
        raise ValueError(f"cannot read image: {path}")
    return img


def write_image(path, img: np.ndarray):
    ext = Path(path).suffix.lower()
    opts = [cv2.IMWRITE_JPEG_QUALITY, 95] if ext in (".jpg", ".jpeg") else []
    ok, buf = cv2.imencode(ext, img, opts)
    if not ok:
        raise ValueError(f"cannot write image: {path}")
    buf.tofile(str(path))


def sha256(path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def supported_colluders(m: int, max_receivers: int, eps: float) -> int:
    """Largest colluder group the Tardos bound guarantees for a code of length m. 0 = one leaker only."""
    return int(floor(sqrt(m / (2 * pi**2 * log(max_receivers / eps)))))


# ---------------------------------------------------------------- register
@dataclass
class Plan:
    kind: str
    m: int
    params: dict
    guaranteed_colluders: int
    notes: list = field(default_factory=list)


def plan_file(path, max_receivers=1000, eps=1e-3, strength=None, crf=None, slots=None) -> Plan:
    kind = kind_of(path)
    notes = []
    if kind == "image":
        img = read_image(path)
        h, w = img.shape[:2]
        want = slots or DEFAULT_SLOTS
        slots = usable_slots(h, w, wanted=want)
        if slots < want:
            notes.append(f"Image ({w}x{h}) is too small for {want} symbols: only {slots} fit. "
                         f"Tracing is weaker.")
        p = ImageParams(nslots=slots, **({"strength": strength} if strength else {}))
        m = slots
    elif kind == "video":
        kw = {**({"strength": strength} if strength else {}), **({"crf": crf} if crf is not None else {})}
        p = VideoParams(**kw)
        m = video_capacity(probe(str(path)), p)
    else:
        p = AudioParams(**({"alpha": strength} if strength else {}))
        m = audio_capacity(probe_audio(str(path)).duration, p)
    g = supported_colluders(m, max_receivers, eps)
    if g == 0:
        notes.append("Enough for one leaker. The theory does not guarantee tracing a group of colluders "
                     "with this many symbols. (The theory is cautious; tests on real files did better.) "
                     "Use a longer or larger file, fewer receivers, or a looser false-accusation rate "
                     "for a stronger guarantee.")
    return Plan(kind, m, asdict(p), g, notes)


def add_asset(ws: Workspace, path, name=None, max_receivers=1000, n_colluders=3, eps=1e-3,
              strength=None, crf=None, slots=None) -> tuple[Asset, Plan]:
    path = Path(path)
    plan = plan_file(path, max_receivers, eps, strength, crf, slots)
    name = name or path.stem
    aid = ws.add_asset(name, plan.kind, path, sha256(path), plan.params, plan.m,
                       max_receivers, n_colluders, eps)
    dest = ws.home / "originals" / f"{aid}_{path.name}"
    shutil.copy2(path, dest)
    ws.set_asset_path(aid, dest)
    return ws.asset(aid), plan


# ---------------------------------------------------------------- mark
def _code(ws: Workspace, a: Asset, n_users: int) -> TardosCode:
    return TardosCode.create(n_users, a.m, ws.tardos_seed(a.id), n_colluders=a.n_colluders)


def safe_name(s: str) -> str:
    return re.sub(r"[^\w.-]+", "_", s).strip("_") or "receiver"


def mark_for_receiver(ws: Workspace, a: Asset, receiver, outdir, progress: Progress | None = None) -> Path:
    """Write one marked copy for one receiver. Returns the output path."""
    outdir = Path(outdir)
    outdir.mkdir(parents=True, exist_ok=True)
    rec = ws.receiver(receiver)
    ui = ws.issue(a.id, rec["id"])
    word = _code(ws, a, ui + 1).codeword(ui)
    src = Path(a.orig_path)
    ext = src.suffix.lower()
    if a.kind == "image" and ext == ".webp":
        ext = ".png"                                  # keep the copy lossless
    out = outdir / f"{safe_name(Path(a.name).stem)}__{safe_name(rec['name'])}{ext}"
    seed = ws.layout_seed(a.id)

    if a.kind == "image":
        p = ImageParams(**a.params)
        write_image(out, embed_image(read_image(src), word, seed, p))
    elif a.kind == "video":
        p = VideoParams(**a.params)
        embed_video(str(src), str(out), word, seed, p,
                    progress=(lambda n, t: progress(f"  frame {n}/{t}")) if progress else None)
    else:
        embed_audio(str(src), str(out), word, seed, AudioParams(**a.params))
    ws.issue(a.id, rec["id"], out)
    return out


# ---------------------------------------------------------------- trace
@dataclass
class TraceResult:
    asset: str
    kind: str
    ranking: list            # [(receiver name, score)]
    threshold: float
    flagged: list            # receivers above the threshold
    verdict: str
    warnings: list
    evidence: dict


def report_text(r: TraceResult, leaked) -> str:
    """Plain-text report to keep with the leaked file."""
    import time
    leaked = Path(leaked)
    lines = [
        "VisualFingerprint trace report",
        f"Date:            {time.strftime('%Y-%m-%d %H:%M:%S')}",
        f"Leaked file:     {leaked.name}",
        f"SHA-256:         {sha256(leaked)}",
        f"Matched asset:   {r.asset} ({r.kind})",
        f"Threshold:       {r.threshold:.1f} (score a receiver must beat to be accused)",
        f"Evidence:        {r.evidence}",
        "",
        f"RESULT: {r.verdict}",
    ]
    lines += [f"WARNING: {w}" for w in r.warnings]
    lines += ["", "Rank  Receiver                        Score"]
    lines += [f"{i:>4}  {n:<30}{s:>8.1f}{'   above threshold' if s > r.threshold else ''}"
              for i, (n, s) in enumerate(r.ranking, 1)]
    lines += ["", "A score is statistical evidence, not proof. Keep this report with the leaked file."]
    return "\n".join(lines)


def trace_leak(ws: Workspace, leaked, asset=None) -> TraceResult:
    """Score a leaked file against one asset (or find the matching asset by trying each)."""
    leaked = Path(leaked)
    kind = kind_of(leaked)
    if asset is not None:
        return _trace_one(ws, ws.asset(asset), leaked)
    cands = [a for a in ws.assets() if a.kind == kind and ws.issues(a.id)]
    if not cands:
        raise ValueError(f"no registered {kind} assets with issued copies")
    best = None
    for a in cands:
        r = _trace_one(ws, a, leaked)
        strength = r.ranking[0][1] / r.threshold if r.ranking else 0.0
        if best is None or strength > best[0]:
            best = (strength, r)
    return best[1]


def _trace_one(ws: Workspace, a: Asset, leaked: Path) -> TraceResult:
    issued = ws.issues(a.id)
    if not issued:
        raise ValueError(f"asset '{a.name}' has no issued copies")
    seed, src = ws.layout_seed(a.id), a.orig_path
    warnings, ev = [], {}
    if a.kind == "image":
        det = detect_image(read_image(leaked), read_image(src), seed, ImageParams(**a.params))
        ev = {"alignment": det.info.get("method"), "overlap": round(det.valid_fraction, 2)}
        if det.valid_fraction < 0.3:
            warnings.append("Less than 30% of the leaked image overlaps the original.")
        cover = det.valid_fraction
    elif a.kind == "video":
        det = detect_video(str(leaked), src, seed, VideoParams(**a.params), m=a.m)
        ev = {"time_offset_frames": det.info["tau"], "alignment": det.info["warp"],
              "symbols_with_evidence": round(det.coverage, 2)}
        cover = det.coverage
    else:
        det = detect_audio(str(leaked), src, seed, AudioParams(**a.params), m=a.m)
        ev = {"offset_samples": det.info["offset"], "alignment_score": round(det.info["align"], 2),
              "symbols_with_evidence": round(det.coverage, 2)}
        cover = det.coverage
        if det.info["align"] < 0.5:
            warnings.append("Audio alignment is weak. The file may have been sped up, slowed down, "
                            "or changed too much.")
    code = _code(ws, a, max(r["user_index"] for r in issued) + 1)
    scores = code.scores(det.bits, det.confidence)
    thr = accusation_threshold(a.max_receivers, a.m, a.eps)
    names = {r["user_index"]: r["name"] for r in issued}
    ranking = sorted(((names[i], float(scores[i])) for i in names), key=lambda t: -t[1])
    flagged = [n for n, s in ranking if s > thr]
    if not flagged and cover < 0.5:
        warnings.append("Less than half of the code symbols could be read (the copy may be a short "
                        "excerpt or heavily damaged). This can explain the weak result.")
    if not flagged:
        verdict = "INCONCLUSIVE: no receiver is above the accusation threshold."
        if ranking and ranking[0][1] > 0:
            verdict += (f" Leading candidate: {ranking[0][0]} (score {ranking[0][1]:.0f}, "
                        f"threshold {thr:.0f}). This is a lead, not grounds to accuse.")
    elif len(flagged) == 1:
        verdict = f"LEAKER: {flagged[0]}"
    else:
        verdict = "POSSIBLE COLLUSION: " + ", ".join(flagged)
    return TraceResult(a.name, a.kind, ranking, float(thr), flagged, verdict, warnings, ev)
