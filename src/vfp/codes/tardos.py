"""Binary Tardos fingerprinting code (Skoric symmetric accusation).

Setup:   for each code position i, draw bias p_i from the Tardos
         distribution (cutoff t). Each user j gets bit X[j,i] = 1 with
         probability p_i.
Tracing: given the symbol sequence y recovered from a leaked copy,
         user j scores S_j = sum_i g(X[j,i], y_i, p_i).
         Users with S_j above a threshold Z are accused.
"""

from __future__ import annotations

from dataclasses import dataclass
from math import ceil, log, pi, sqrt

import numpy as np


def required_length(n_users: int, n_colluders: int, eps1: float = 1e-3) -> int:
    """Code length m for false-accusation probability about eps1.

    Uses the Skoric et al. symmetric-score bound m = 2 pi^2 c^2 ln(1/eps1)
    for the single-user false-accusation target, with a union margin over
    n users handled by the threshold (see threshold()).
    """
    eps_single = eps1 / max(n_users, 1)
    return ceil(2 * pi**2 * n_colluders**2 * log(1 / eps_single))


@dataclass
class TardosCode:
    p: np.ndarray  # shape (m,)  bias per position
    seed: int      # seed used to draw codewords
    n_users: int

    @property
    def length(self) -> int:
        return len(self.p)

    @staticmethod
    def create(n_users: int, length: int, seed: int, n_colluders: int = 3) -> "TardosCode":
        rng = np.random.default_rng(seed)
        t = 1.0 / (300 * n_colluders)
        t_prime = np.arcsin(np.sqrt(t))
        r = rng.uniform(t_prime, pi / 2 - t_prime, size=length)
        p = np.sin(r) ** 2
        return TardosCode(p=p, seed=seed, n_users=n_users)

    def codeword(self, user_index: int) -> np.ndarray:
        """Bit string for one user. Derived from (seed, user_index): no big table needed."""
        rng = np.random.default_rng([self.seed, user_index + 1])
        return (rng.random(self.length) < self.p).astype(np.uint8)

    def all_codewords(self) -> np.ndarray:
        return np.stack([self.codeword(j) for j in range(self.n_users)])

    # ---- tracing -------------------------------------------------------
    def scores(self, y: np.ndarray, confidence: np.ndarray | None = None) -> np.ndarray:
        """Accusation scores for all users.

        y: recovered symbols, values in {0,1}. Use soft values in [-1,+1]
           (negative = 0, positive = 1) through `confidence` weighting:
           pass y as hard bits and confidence in [0,1] per position.
           Positions with confidence 0 (lost segments) are ignored.
        """
        y = np.asarray(y)
        w = np.ones(self.length) if confidence is None else np.asarray(confidence, float)
        X = self.all_codewords()                       # (n, m)
        p = self.p
        gp = np.sqrt((1 - p) / p)                      # weight when x=1
        gm = np.sqrt(p / (1 - p))                      # weight when x=0
        # Skoric symmetric: y=1 -> x=1: +gp, x=0: -gm ; y=0 -> x=0: +gm, x=1: -gp
        s_y1 = np.where(X == 1, gp, -gm)
        s_y0 = np.where(X == 0, gm, -gp)
        contrib = np.where(y[None, :] == 1, s_y1, s_y0) * w[None, :]
        return contrib.sum(axis=1)

    def accuse(self, y, confidence=None, n_colluders: int = 3, eps1: float = 1e-3):
        """Return (ranked list of (user_index, score)), threshold."""
        s = self.scores(y, confidence)
        z = accusation_threshold(self.n_users, self.length, eps1)
        order = np.argsort(-s)
        return [(int(j), float(s[j])) for j in order], z


def accusation_threshold(n_users: int, length: int, eps1: float = 1e-3) -> float:
    """Threshold so an innocent user passes with probability <= eps1 overall.

    An innocent score has mean 0 and variance about `length` (each term has
    unit variance under the symmetric score). Gaussian tail + union bound
    over n users.
    """
    from scipy.stats import norm

    return float(sqrt(length) * norm.isf(eps1 / max(n_users, 1)))
