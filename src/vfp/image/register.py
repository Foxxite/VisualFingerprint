"""Align a leaked image or video frame to the original.

Handles scale, crop, mild rotation and perspective. Works on BGR or single-plane
(luma) arrays. A video estimates the warp once and applies it to every frame.
"""

from __future__ import annotations

import cv2
import numpy as np


def _gray(a):
    return a if a.ndim == 2 else cv2.cvtColor(a, cv2.COLOR_BGR2GRAY)


def _warp(img, M, size, flags):
    return cv2.warpPerspective(img, M, size, flags=flags | cv2.WARP_INVERSE_MAP,
                               borderMode=cv2.BORDER_CONSTANT, borderValue=0)


def estimate_warp(leaked: np.ndarray, original: np.ndarray):
    """Return (M, info). M maps original coords -> leaked coords (3x3), or None if
    the two are already aligned. info has 'method' and 'err' (mean abs gray error)."""
    H, W = original.shape[:2]
    og, lg = _gray(original), _gray(leaked)
    id_err = None
    if lg.shape == og.shape:
        id_err = float(np.mean(np.abs(og.astype(np.int16) - lg.astype(np.int16))))
        if id_err < 2.0:   # clearly aligned (JPEG, noise, blur, tone changes)
            return None, {"method": "identity", "err": id_err}
        # else a small shift or rotation may hide here: try features, keep the better.

    def warp_err(M):
        a = _warp(lg, M, (W, H), cv2.INTER_LINEAR)
        m = _warp(np.full(lg.shape, 255, np.uint8), M, (W, H), cv2.INTER_NEAREST) > 0
        if m.mean() < 0.2:
            return 1e9
        return float(np.abs(a.astype(np.int16) - og.astype(np.int16))[m].mean())

    # coarse: ORB features + RANSAC homography (leaked -> original)
    orb = cv2.ORB_create(5000)
    k1, d1 = orb.detectAndCompute(lg, None)
    k2, d2 = orb.detectAndCompute(og, None)
    Hm, inl = None, 0
    if d1 is not None and d2 is not None and len(k1) > 12 and len(k2) > 12:
        pairs = cv2.BFMatcher(cv2.NORM_HAMMING).knnMatch(d1, d2, k=2)
        good = [m for m, n in (p for p in pairs if len(p) == 2) if m.distance < 0.75 * n.distance]
        if len(good) >= 12:
            src = np.float32([k1[m.queryIdx].pt for m in good])
            dst = np.float32([k2[m.trainIdx].pt for m in good])
            Hm, mask = cv2.findHomography(src, dst, cv2.RANSAC, 3.0)
            inl = int(mask.sum()) if mask is not None else 0

    if Hm is None or inl < 10:
        if id_err is not None:
            return None, {"method": "identity", "err": id_err}
        # fallback: assume the whole frame was only resized
        lh, lw = lg.shape[:2]
        sx, sy = lw / W, lh / H
        M = np.array([[sx, 0, 0.5 * sx - 0.5], [0, sy, 0.5 * sy - 0.5], [0, 0, 1]])
        return M, {"method": "resize-fallback", "inliers": inl, "err": warp_err(M)}

    M = np.linalg.inv(Hm)

    # fine: ECC refinement for sub-pixel accuracy (kept only if it lowers the error)
    try:
        flags = cv2.INTER_LINEAR
        pre = _warp(lg, M, (W, H), flags)
        pre_mask = _warp(np.full(lg.shape, 255, np.uint8), M, (W, H), cv2.INTER_NEAREST)
        pre_mask = cv2.erode(pre_mask, np.ones((9, 9), np.uint8))
        W2 = np.eye(3, dtype=np.float32)
        crit = (cv2.TERM_CRITERIA_EPS | cv2.TERM_CRITERIA_COUNT, 60, 1e-6)
        _, W2 = cv2.findTransformECC(og.astype(np.float32), pre.astype(np.float32), W2,
                                     cv2.MOTION_HOMOGRAPHY, crit, pre_mask, 5)
        cand = M @ np.asarray(W2, np.float64)
        if warp_err(cand) < warp_err(M):
            M = cand
    except cv2.error:
        pass

    err = warp_err(M)
    if id_err is not None and err >= 0.8 * id_err:   # resampling blurs: need a clear win
        return None, {"method": "identity", "err": id_err}
    return M, {"method": "orb+ecc", "inliers": inl, "err": err}


def apply_warp(leaked: np.ndarray, M, size_wh):
    """Warp `leaked` into the original grid. Returns (aligned, valid_bool_mask)."""
    W, H = size_wh
    if M is None:
        return leaked, np.ones((H, W), bool)
    aligned = _warp(leaked, M, (W, H), cv2.INTER_CUBIC)
    ones = np.full(leaked.shape[:2], 255, np.uint8)
    vm = _warp(ones, M, (W, H), cv2.INTER_NEAREST)
    valid = cv2.erode(vm, np.ones((5, 5), np.uint8)) > 0
    return aligned, valid


def register(leaked: np.ndarray, original: np.ndarray):
    """Return (aligned, valid, info) for a single image."""
    M, info = estimate_warp(leaked, original)
    aligned, valid = apply_warp(leaked, M, (original.shape[1], original.shape[0]))
    return aligned, valid, info
