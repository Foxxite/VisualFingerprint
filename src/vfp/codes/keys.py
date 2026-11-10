"""Key derivation. One master secret -> independent per-purpose seeds."""

from __future__ import annotations

import hashlib
import hmac
import secrets
from pathlib import Path


def new_master_key() -> bytes:
    return secrets.token_bytes(32)


def load_or_create_master_key(path: str | Path) -> bytes:
    """Read the master key from a file. Create the file if it is missing."""
    p = Path(path)
    if p.exists():
        return bytes.fromhex(p.read_text().strip())
    key = new_master_key()
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(key.hex())
    return key


def derive_seed(master: bytes, *labels: str) -> int:
    """Derive a 64-bit integer seed from the master key and text labels.

    Example: derive_seed(key, "asset", asset_id, "tardos")
    """
    msg = "\x1f".join(labels).encode()
    digest = hmac.new(master, msg, hashlib.sha256).digest()
    return int.from_bytes(digest[:8], "big")
