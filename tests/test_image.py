import numpy as np
import pytest

from vfp.codes.keys import derive_seed
from vfp.codes.tardos import TardosCode, accusation_threshold
from vfp.eval import attacks as A
from vfp.eval.image_eval import synthetic_image
from vfp.eval.metrics import psnr
from vfp.image.common import ImageParams
from vfp.image.detect import detect_image
from vfp.image.embed import embed_image

USERS, USER, NSLOTS = 30, 4, 256
PARAMS = ImageParams(nslots=NSLOTS, strength=3.0)
SEED = derive_seed(b"t" * 32, "img")


@pytest.fixture(scope="module")
def setup():
    img = synthetic_image(960, 540)
    code = TardosCode.create(USERS, NSLOTS, derive_seed(b"t" * 32, "code"), n_colluders=1)
    word = code.codeword(USER)
    return img, code, word, embed_image(img, word, SEED, PARAMS)


def trace(setup, attack):
    img, code, word, marked = setup
    det = detect_image(attack(marked), img, SEED, PARAMS)
    s = code.scores(det.bits, det.confidence)
    return int(np.argmax(s)), s[USER], accusation_threshold(USERS, NSLOTS, 1e-3)


def test_mark_is_quiet(setup):
    img, _, _, marked = setup
    assert psnr(img, marked) > 40


@pytest.mark.parametrize("name", ["jpeg q50", "scale 0.75", "crop 90%", "rotate 2deg",
                                  "scale 0.6 + jpeg q60", "noise s=4"])
def test_traced_after_attack(setup, name):
    who, score, thr = trace(setup, A.IMAGE_ATTACKS[name])
    assert who == USER and score > thr


def test_unmarked_image_accuses_nobody(setup):
    img, code, _, _ = setup
    det = detect_image(img, img, SEED, PARAMS)
    s = code.scores(det.bits, det.confidence)
    assert s.max() < accusation_threshold(USERS, NSLOTS, 1e-3)
