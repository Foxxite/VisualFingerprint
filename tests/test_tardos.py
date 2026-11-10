import numpy as np
import pytest

from vfp.codes.keys import derive_seed
from vfp.codes.tardos import TardosCode, accusation_threshold, required_length

N_USERS = 200
C = 3
EPS = 1e-3


def make_code():
    m = required_length(N_USERS, C, EPS)
    return TardosCode.create(N_USERS, m, seed=derive_seed(b"k" * 32, "t"), n_colluders=C)


def collude(words, strategy, rng):
    """words: (c, m) bits. Return pirate sequence."""
    c, m = words.shape
    agree = (words == words[0]).all(axis=0)
    out = words[0].copy()
    if strategy == "majority":
        out = (words.sum(axis=0) * 2 > c).astype(np.uint8)
    elif strategy == "interleave":
        out = words[rng.integers(0, c, size=m), np.arange(m)]
    elif strategy == "minority":
        s = words.sum(axis=0)
        out = np.where(agree, words[0], (s * 2 < c).astype(np.uint8))
    elif strategy == "all0":
        out = np.where(agree, words[0], 0).astype(np.uint8)
    elif strategy == "all1":
        out = np.where(agree, words[0], 1).astype(np.uint8)
    return out


def test_codeword_is_deterministic():
    code = make_code()
    assert (code.codeword(5) == code.codeword(5)).all()
    assert not (code.codeword(5) == code.codeword(6)).all()


def test_codeword_density_matches_bias():
    code = make_code()
    mean = np.mean([code.codeword(j) for j in range(N_USERS)], axis=0)
    assert abs(mean.mean() - code.p.mean()) < 0.03


@pytest.mark.parametrize("strategy", ["majority", "interleave", "minority", "all0", "all1"])
def test_colluders_caught_innocents_not(strategy):
    code = make_code()
    z = accusation_threshold(N_USERS, code.length, EPS)
    rng = np.random.default_rng(1)
    caught = 0
    false_acc = 0
    trials = 20
    for _ in range(trials):
        guilty = rng.choice(N_USERS, size=C, replace=False)
        words = np.stack([code.codeword(j) for j in guilty])
        y = collude(words, strategy, rng)
        s = code.scores(y)
        accused = set(np.flatnonzero(s > z))
        caught += len(accused & set(guilty)) > 0
        false_acc += len(accused - set(guilty)) > 0
    assert false_acc == 0
    assert caught >= trials - 1  # at least one colluder found nearly always


def test_lost_segments_are_ignored():
    code = make_code()
    word = code.codeword(7)
    conf = np.ones(code.length)
    conf[: code.length // 3] = 0  # a third of the symbols could not be read
    s = code.scores(word, conf)
    assert np.argmax(s) == 7
