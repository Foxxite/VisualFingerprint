from __future__ import annotations

import cv2
import numpy as np


def psnr(a: np.ndarray, b: np.ndarray) -> float:
    mse = np.mean((a.astype(np.float64) - b.astype(np.float64)) ** 2)
    return float("inf") if mse == 0 else 10 * np.log10(255**2 / mse)


def ssim(a: np.ndarray, b: np.ndarray) -> float:
    """Mean SSIM on grayscale, 11x11 gaussian window."""
    x = cv2.cvtColor(a, cv2.COLOR_BGR2GRAY).astype(np.float64)
    y = cv2.cvtColor(b, cv2.COLOR_BGR2GRAY).astype(np.float64)
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    g = lambda im: cv2.GaussianBlur(im, (11, 11), 1.5)
    mx, my = g(x), g(y)
    sxx, syy, sxy = g(x * x) - mx**2, g(y * y) - my**2, g(x * y) - mx * my
    s = ((2 * mx * my + c1) * (2 * sxy + c2)) / ((mx**2 + my**2 + c1) * (sxx + syy + c2))
    return float(s.mean())
