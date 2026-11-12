//! Vault: keys and encryption for sensitive data.
//!
//! ```text
//! passphrase --Argon2id(salt)--> KEK --unwraps--> DEK (random 256 bit, never stored in clear)
//! DEK --HKDF(label)--> sub-keys: "files", "master", "db-fields", "db-index"
//! ```
//!
//! `vault.json` holds the salt, the Argon2 cost and the wrapped DEK. It is safe to store on a
//! server, but anyone who has it can try passphrases offline. So the passphrase must be strong
//! (see [`crate::strength`]) and the Argon2 cost is high.
//!
//! Files use the `.vfpenc` format: XChaCha20-Poly1305 in the STREAM construction, 1 MiB chunks.
//! A cut-off file, a changed chunk, a wrong key or a wrong `aad` all fail to decrypt.

use crate::error::{err, Error, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::io::{Read, Write};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const VAULT_FILE: &str = "vault.json";
const MAGIC: &[u8; 8] = b"VFPENC1\0";
pub const CHUNK: usize = 1 << 20;
const TAG: usize = 16;
const STREAM_NONCE: usize = 19; // 24 (XChaCha20) - 4 (counter) - 1 (last flag)

/// How the server treats the key. Stored in `vault.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protection {
    /// The server stores ciphertext. The client sends the key with each request over HTTPS.
    Protected,
    /// The server never gets the key. The client decrypts and works locally.
    ZeroKnowledge,
}

/// Argon2id cost.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    /// Desktop default: 256 MiB, 3 passes.
    pub const DESKTOP: KdfParams = KdfParams { m_kib: 256 * 1024, t: 3, p: 1 };
    /// Phones: 64 MiB, 3 passes.
    pub const MOBILE: KdfParams = KdfParams { m_kib: 64 * 1024, t: 3, p: 1 };
    /// Fast, for tests only.
    pub const TEST: KdfParams = KdfParams { m_kib: 64, t: 1, p: 1 };
}

/// The data key. Wiped from memory when dropped.
#[derive(Clone, ZeroizeOnDrop)]
pub struct Dek([u8; 32]);

impl Dek {
    pub fn random() -> Result<Dek> {
        let mut k = [0u8; 32];
        getrandom::getrandom(&mut k)?;
        Ok(Dek(k))
    }

    pub fn from_bytes(b: &[u8]) -> Result<Dek> {
        let k: [u8; 32] = b.try_into().map_err(|_| Error("key must be 32 bytes".into()))?;
        Ok(Dek(k))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Hex text for sending in a header over HTTPS (Protected mode only).
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    pub fn from_hex(s: &str) -> Result<Dek> {
        let b = unhex(s.trim())?;
        Dek::from_bytes(&b)
    }

    /// One key per job, so a key is never used for two purposes.
    pub fn subkey(&self, label: &str) -> Key32 {
        let hk = Hkdf::<Sha256>::new(Some(b"vfp-vault-v1"), &self.0);
        let mut out = [0u8; 32];
        hk.expand(label.as_bytes(), &mut out).expect("32 bytes is a valid HKDF length");
        Key32(out)
    }
}

impl std::fmt::Debug for Dek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Dek(<hidden>)")
    }
}

#[derive(Clone, ZeroizeOnDrop)]
pub struct Key32([u8; 32]);

impl Key32 {
    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(Key::from_slice(&self.0))
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return err("bad hex text");
    }
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|_| Error("bad hex text".into()))).collect()
}

fn fail() -> Error {
    Error("cannot decrypt: wrong key, or the data is damaged".into())
}

// ---- small values (database fields, master key, wrapped DEK)

/// Encrypt a short value. Output: 24-byte random nonce, then ciphertext and tag.
pub fn seal(key: &Key32, aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; 24];
    getrandom::getrandom(&mut nonce)?;
    let ct = key.cipher().encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad }).map_err(|_| Error("encryption failed".into()))?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn open(key: &Key32, aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < 24 + TAG {
        return Err(fail());
    }
    let (nonce, ct) = sealed.split_at(24);
    key.cipher().decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad }).map_err(|_| fail())
}

/// Keyed hash for looking up an encrypted field (for example a receiver name) without storing it.
pub fn blind_index(key: &Key32, text: &str) -> String {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(&key.0).expect("HMAC takes any key length");
    m.update(text.as_bytes());
    hex(&m.finalize().into_bytes())
}

// ---- files

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let k = r.read(&mut buf[n..])?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

/// Encrypt `r` into `w` in the `.vfpenc` format. `aad` binds the file to its purpose
/// (for example `b"original:7"`); decrypting needs the same `aad`.
pub fn encrypt_stream(key: &Key32, aad: &[u8], r: &mut impl Read, w: &mut impl Write) -> Result<u64> {
    let mut prefix = [0u8; STREAM_NONCE];
    getrandom::getrandom(&mut prefix)?;
    w.write_all(MAGIC)?;
    w.write_all(&(CHUNK as u32).to_le_bytes())?;
    w.write_all(&prefix)?;
    let mut enc = EncryptorBE32::from_aead(key.cipher(), (&prefix).into());
    let mut total = 0u64;
    let mut cur = vec![0u8; CHUNK];
    let mut next = vec![0u8; CHUNK];
    let mut cur_len = read_up_to(r, &mut cur)?;
    loop {
        total += cur_len as u64;
        if cur_len < CHUNK {
            w.write_all(&enc.encrypt_last(Payload { msg: &cur[..cur_len], aad }).map_err(|_| Error("encryption failed".into()))?)?;
            break;
        }
        let next_len = read_up_to(r, &mut next)?;
        if next_len == 0 {
            w.write_all(&enc.encrypt_last(Payload { msg: &cur[..cur_len], aad }).map_err(|_| Error("encryption failed".into()))?)?;
            break;
        }
        w.write_all(&enc.encrypt_next(Payload { msg: &cur[..cur_len], aad }).map_err(|_| Error("encryption failed".into()))?)?;
        std::mem::swap(&mut cur, &mut next);
        cur_len = next_len;
    }
    cur.zeroize();
    next.zeroize();
    Ok(total)
}

/// Decrypt a `.vfpenc` stream. Fails on a wrong key, a wrong `aad`, a changed or cut-off file.
pub fn decrypt_stream(key: &Key32, aad: &[u8], r: &mut impl Read, w: &mut impl Write) -> Result<u64> {
    let mut head = [0u8; 8 + 4 + STREAM_NONCE];
    if read_up_to(r, &mut head)? != head.len() || &head[..8] != MAGIC {
        return err("not an encrypted VisualFingerprint file");
    }
    let chunk = u32::from_le_bytes(head[8..12].try_into().unwrap()) as usize;
    if chunk == 0 || chunk > 64 << 20 {
        return err("bad chunk size in encrypted file");
    }
    let prefix: [u8; STREAM_NONCE] = head[12..].try_into().unwrap();
    let mut dec = DecryptorBE32::from_aead(key.cipher(), (&prefix).into());
    let size = chunk + TAG;
    let mut total = 0u64;
    let mut cur = vec![0u8; size];
    let mut next = vec![0u8; size];
    let mut cur_len = read_up_to(r, &mut cur)?;
    loop {
        if cur_len < TAG {
            return Err(fail());
        }
        let last = if cur_len < size {
            true
        } else {
            let n = read_up_to(r, &mut next)?;
            if n == 0 {
                true
            } else {
                // not the last chunk
                let mut plain = dec.decrypt_next(Payload { msg: &cur[..cur_len], aad }).map_err(|_| fail())?;
                w.write_all(&plain)?;
                total += plain.len() as u64;
                plain.zeroize();
                std::mem::swap(&mut cur, &mut next);
                cur_len = n;
                false
            }
        };
        if last {
            let mut plain = dec.decrypt_last(Payload { msg: &cur[..cur_len], aad }).map_err(|_| fail())?;
            w.write_all(&plain)?;
            total += plain.len() as u64;
            plain.zeroize();
            break;
        }
    }
    Ok(total)
}

pub fn encrypt_bytes(key: &Key32, aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(plain.len() + 64);
    encrypt_stream(key, aad, &mut &plain[..], &mut out)?;
    Ok(out)
}

pub fn decrypt_bytes(key: &Key32, aad: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len());
    decrypt_stream(key, aad, &mut &data[..], &mut out)?;
    Ok(out)
}

// ---- vault.json

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VaultFile {
    pub version: u32,
    pub protection: Protection,
    pub kdf: KdfParams,
    /// hex
    pub salt: String,
    /// hex: nonce + ciphertext of the DEK, sealed by the KEK
    pub wrapped_dek: String,
}

fn kek(passphrase: &str, salt: &[u8], p: &KdfParams) -> Result<Key32> {
    let params = Params::new(p.m_kib, p.t, p.p, Some(32)).map_err(|e| Error(format!("bad Argon2 settings: {e}")))?;
    let mut out = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| Error(format!("key derivation failed: {e}")))?;
    Ok(Key32(out))
}

const WRAP_AAD: &[u8] = b"vfp:wrapped-dek:v1";

impl VaultFile {
    /// Make a new vault. The passphrase must be strong. Returns the file and the new DEK.
    pub fn create(passphrase: &str, protection: Protection, kdf: KdfParams) -> Result<(VaultFile, Dek)> {
        crate::strength::require_strong(passphrase)?;
        let dek = Dek::random()?;
        let v = Self::wrap(passphrase, protection, kdf, &dek)?;
        Ok((v, dek))
    }

    fn wrap(passphrase: &str, protection: Protection, kdf: KdfParams, dek: &Dek) -> Result<VaultFile> {
        let mut salt = [0u8; 16];
        getrandom::getrandom(&mut salt)?;
        let k = kek(passphrase, &salt, &kdf)?;
        let wrapped = seal(&k, WRAP_AAD, dek.as_bytes())?;
        Ok(VaultFile { version: 1, protection, kdf, salt: hex(&salt), wrapped_dek: hex(&wrapped) })
    }

    /// Check the passphrase and return the DEK.
    pub fn unlock(&self, passphrase: &str) -> Result<Dek> {
        if self.version != 1 {
            return err(format!("unknown vault version {}", self.version));
        }
        let k = kek(passphrase, &unhex(&self.salt)?, &self.kdf)?;
        let mut plain = open(&k, WRAP_AAD, &unhex(&self.wrapped_dek)?).map_err(|_| Error("wrong passphrase".into()))?;
        let dek = Dek::from_bytes(&plain);
        plain.zeroize();
        dek
    }

    /// New passphrase, same DEK. No file needs re-encrypting.
    pub fn change_passphrase(&self, old: &str, new: &str) -> Result<VaultFile> {
        crate::strength::require_strong(new)?;
        let dek = self.unlock(old)?;
        Self::wrap(new, self.protection, self.kdf, &dek)
    }

    pub fn load(path: &std::path::Path) -> Result<Option<VaultFile>> {
        if !path.exists() {
            return Ok(None);
        }
        serde_json::from_slice(&std::fs::read(path)?).map(Some).map_err(|e| Error(format!("bad {VAULT_FILE}: {e}")))
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(|e| Error(e.to_string()))?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASS: &str = "gravel unmoved shabby plaything cactus bonfire";

    fn key() -> Key32 {
        Dek::random().unwrap().subkey("files")
    }

    #[test]
    fn small_values_round_trip_and_are_bound_to_aad() {
        let k = key();
        let s = seal(&k, b"receiver:name", b"Anna").unwrap();
        assert!(!s.windows(4).any(|w| w == b"Anna"));
        assert_eq!(open(&k, b"receiver:name", &s).unwrap(), b"Anna");
        assert!(open(&k, b"receiver:note", &s).is_err());
        assert!(open(&key(), b"receiver:name", &s).is_err());
        let mut bad = s.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(open(&k, b"receiver:name", &bad).is_err());
    }

    #[test]
    fn stream_round_trip_for_many_sizes() {
        let k = key();
        for n in [0usize, 1, 100, CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK, 2 * CHUNK + 5] {
            let data: Vec<u8> = (0..n).map(|i| (i * 31 % 251) as u8).collect();
            let enc = encrypt_bytes(&k, b"original:1", &data).unwrap();
            assert!(enc.len() > n, "size {n}");
            assert_eq!(decrypt_bytes(&k, b"original:1", &enc).unwrap(), data, "size {n}");
        }
    }

    #[test]
    fn ciphertext_hides_plaintext() {
        let k = key();
        let data = b"SECRET-MARKER-SECRET-MARKER".repeat(1000);
        let enc = encrypt_bytes(&k, b"x", &data).unwrap();
        assert!(!enc.windows(13).any(|w| w == b"SECRET-MARKER"));
    }

    #[test]
    fn tamper_truncation_wrong_key_and_wrong_aad_fail() {
        let k = key();
        let data = vec![7u8; 2 * CHUNK + 100];
        let enc = encrypt_bytes(&k, b"original:1", &data).unwrap();
        // wrong key / aad
        assert!(decrypt_bytes(&key(), b"original:1", &enc).is_err());
        assert!(decrypt_bytes(&k, b"original:2", &enc).is_err());
        // flipped byte in the middle
        let mut t = enc.clone();
        t[CHUNK + 500] ^= 1;
        assert!(decrypt_bytes(&k, b"original:1", &t).is_err());
        // cut after a whole chunk (a clean cut must not look like a short file)
        let full = 8 + 4 + STREAM_NONCE + CHUNK + TAG;
        assert!(decrypt_bytes(&k, b"original:1", &enc[..full]).is_err());
        assert!(decrypt_bytes(&k, b"original:1", &enc[..enc.len() - 1]).is_err());
        // a file that is exactly one chunk long, cut to zero chunks
        let one = encrypt_bytes(&k, b"a", &vec![1u8; CHUNK]).unwrap();
        assert!(decrypt_bytes(&k, b"a", &one[..8 + 4 + STREAM_NONCE]).is_err());
        // not our format
        assert!(decrypt_bytes(&k, b"a", b"hello world, not encrypted at all").is_err());
        // chunks swapped
        let a = 8 + 4 + STREAM_NONCE;
        let mut sw = enc.clone();
        let (c1, c2) = (a, a + CHUNK + TAG);
        for i in 0..CHUNK + TAG {
            sw.swap(c1 + i, c2 + i);
        }
        assert!(decrypt_bytes(&k, b"original:1", &sw).is_err());
    }

    #[test]
    fn vault_unlock_wrong_passphrase_and_change() {
        let (v, dek) = VaultFile::create(PASS, Protection::Protected, KdfParams::TEST).unwrap();
        assert_eq!(v.unlock(PASS).unwrap().as_bytes(), dek.as_bytes());
        assert!(v.unlock("some other passphrase here").is_err());
        let new = "plaza minnow handled spiral drizzle yeast";
        let v2 = v.change_passphrase(PASS, new).unwrap();
        assert_eq!(v2.unlock(new).unwrap().as_bytes(), dek.as_bytes());
        assert!(v2.unlock(PASS).is_err());
        assert!(v.change_passphrase(PASS, "weak").is_err());
        // json round trip
        let j = serde_json::to_string(&v2).unwrap();
        assert!(!j.contains(&dek.to_hex()));
        let back: VaultFile = serde_json::from_str(&j).unwrap();
        assert_eq!(back.unlock(new).unwrap().as_bytes(), dek.as_bytes());
    }

    #[test]
    fn weak_passphrase_cannot_make_a_vault() {
        assert!(VaultFile::create("password1", Protection::Protected, KdfParams::TEST).is_err());
    }

    #[test]
    fn subkeys_differ_and_blind_index_is_stable() {
        let d = Dek::random().unwrap();
        assert_ne!(d.subkey("files").0, d.subkey("master").0);
        assert_eq!(d.subkey("files").0, d.subkey("files").0);
        let i = d.subkey("db-index");
        assert_eq!(blind_index(&i, "Anna"), blind_index(&i, "Anna"));
        assert_ne!(blind_index(&i, "Anna"), blind_index(&i, "Ben"));
        assert_eq!(Dek::from_hex(&d.to_hex()).unwrap().as_bytes(), d.as_bytes());
    }
}
