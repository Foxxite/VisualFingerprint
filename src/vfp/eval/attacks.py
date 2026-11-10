"""Image attacks. Each takes and returns a uint8 BGR image."""

from __future__ import annotations

import cv2
import numpy as np


def jpeg(q):
    def f(im):
        ok, buf = cv2.imencode(".jpg", im, [cv2.IMWRITE_JPEG_QUALITY, q])
        return cv2.imdecode(buf, cv2.IMREAD_COLOR)
    return f


def scale(s):
    return lambda im: cv2.resize(im, None, fx=s, fy=s, interpolation=cv2.INTER_AREA)


def crop(frac):
    def f(im):
        h, w = im.shape[:2]
        dh, dw = int(h * (1 - frac) / 2), int(w * (1 - frac) / 2)
        return im[dh:h - dh, dw:w - dw].copy()
    return f


def blur(sigma):
    return lambda im: cv2.GaussianBlur(im, (0, 0), sigma)


def noise(sigma, seed=0):
    def f(im):
        n = np.random.default_rng(seed).normal(0, sigma, im.shape)
        return np.clip(im + n, 0, 255).astype(np.uint8)
    return f


def brightness_contrast(alpha=1.15, beta=-10):
    return lambda im: cv2.convertScaleAbs(im, alpha=alpha, beta=beta)


def rotate(deg, s=1.0):
    def f(im):
        h, w = im.shape[:2]
        M = cv2.getRotationMatrix2D((w / 2, h / 2), deg, s)
        return cv2.warpAffine(im, M, (w, h), flags=cv2.INTER_CUBIC, borderMode=cv2.BORDER_REFLECT)
    return f


def chain(*fs):
    def f(im):
        for g in fs:
            im = g(im)
        return im
    return f


IMAGE_ATTACKS = {
    "none": lambda im: im,
    "jpeg q90": jpeg(90),
    "jpeg q75": jpeg(75),
    "jpeg q50": jpeg(50),
    "jpeg q30": jpeg(30),
    "scale 0.75": scale(0.75),
    "scale 0.5": scale(0.5),
    "crop 90%": crop(0.9),
    "crop 70%": crop(0.7),
    "blur s=1.0": blur(1.0),
    "noise s=4": noise(4),
    "bright/contrast": brightness_contrast(),
    "rotate 2deg": rotate(2.0),
    "scale 0.6 + jpeg q60": chain(scale(0.6), jpeg(60)),
    "crop 85% + jpeg q70": chain(crop(0.85), jpeg(70)),
}
