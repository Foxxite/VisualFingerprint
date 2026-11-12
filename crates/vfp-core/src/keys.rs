//! Key derivation. One master secret gives independent per-purpose seeds.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// 64-bit seed from the master key and text labels (same as `derive_seed` in Python).
pub fn derive_seed(master: &[u8], labels: &[&str]) -> u64 {
    let msg = labels.join("\x1f");
    let mut mac = Hmac::<Sha256>::new_from_slice(master).expect("HMAC takes any key length");
    mac.update(msg.as_bytes());
    let d = mac.finalize().into_bytes();
    u64::from_be_bytes(d[..8].try_into().unwrap())
}
