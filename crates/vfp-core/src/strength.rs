//! Passphrase strength gate.
//!
//! Shannon entropy alone is a weak test. It counts how evenly characters repeat. It does not
//! count how easy the text is to guess ("password1" scores well). So the estimate is the lowest of:
//!
//! 1. Shannon entropy of the characters, times the length (catches repeats such as "abababab").
//! 2. The `zxcvbn` guess estimate (catches dictionary words, names, keyboard patterns, dates).
//!
//! A passphrase passes when the estimate is at least [`MIN_BITS`] and the length is at least
//! [`MIN_CHARS`]. The server never sees the passphrase, so this gate runs on the client.

use crate::error::{err, Result};
use std::collections::HashMap;

pub const MIN_BITS: f64 = 64.0;
pub const MIN_CHARS: usize = 12;
/// Words in a generated passphrase (EFF large list, 12.9 bits per word).
pub const DICEWARE_WORDS: usize = 7;

#[derive(Clone, Debug, serde::Serialize)]
pub struct Strength {
    /// The lowest of the two estimates, in bits.
    pub bits: f64,
    pub shannon_bits: f64,
    pub guess_bits: f64,
    pub ok: bool,
    /// Plain-word advice. Empty when the passphrase is fine.
    pub advice: Vec<String>,
}

/// Shannon entropy of the characters, in bits per character.
fn shannon_per_char(s: &str) -> f64 {
    let mut counts: HashMap<char, usize> = HashMap::new();
    let mut n = 0usize;
    for c in s.chars() {
        *counts.entry(c).or_default() += 1;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    counts
        .values()
        .map(|&c| {
            let p = c as f64 / n as f64;
            -p * p.log2()
        })
        .sum()
}

pub fn assess(passphrase: &str) -> Strength {
    let len = passphrase.chars().count();
    let shannon_bits = shannon_per_char(passphrase) * len as f64;
    let guess_bits = if passphrase.is_empty() { 0.0 } else { zxcvbn::zxcvbn(passphrase, &[]).guesses_log10() * std::f64::consts::LOG2_10 };
    let bits = shannon_bits.min(guess_bits);
    let mut advice = Vec::new();
    if len < MIN_CHARS {
        advice.push(format!("Use at least {MIN_CHARS} characters."));
    }
    if shannon_bits < MIN_BITS {
        advice.push("Too many repeated characters. Mix more different characters.".to_string());
    }
    if guess_bits < MIN_BITS {
        advice.push("This is easy to guess (common words, names, dates or patterns). Use several unrelated words, or press \"Make one for me\".".to_string());
    }
    Strength { bits, shannon_bits, guess_bits, ok: len >= MIN_CHARS && bits >= MIN_BITS, advice }
}

/// `Ok` when the passphrase is strong enough. The error text is the advice.
pub fn require_strong(passphrase: &str) -> Result<()> {
    let s = assess(passphrase);
    if s.ok {
        Ok(())
    } else {
        err(format!("Passphrase is too weak ({:.0} bits, need {MIN_BITS:.0}). {}", s.bits, s.advice.join(" ")))
    }
}

fn random_below(n: usize) -> usize {
    // Rejection sampling, so every word has the same chance.
    let limit = (u32::MAX as usize / n) * n;
    loop {
        let mut b = [0u8; 4];
        getrandom::getrandom(&mut b).expect("system random source");
        let v = u32::from_le_bytes(b) as usize;
        if v < limit {
            return v % n;
        }
    }
}

/// A random passphrase of [`DICEWARE_WORDS`] words from the EFF large list (about 90 bits).
pub fn generate() -> String {
    let list = eff_wordlist::large::LIST;
    (0..DICEWARE_WORDS).map(|_| list[random_below(list.len())].1).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weak_passphrases_fail() {
        for p in ["", "short", "password1", "abababababab", "aaaaaaaaaaaaaaaa", "correct horse", "qwertyuiop12", "Summer2024!!!!"] {
            assert!(!assess(p).ok, "{p:?} should fail: {:?}", assess(p));
        }
        assert!(require_strong("password1").is_err());
    }

    #[test]
    fn shannon_alone_would_be_fooled() {
        // Many different characters, but it is a keyboard walk: zxcvbn must catch it.
        let s = assess("qwertyuiopasdfghjkl");
        assert!(s.shannon_bits > 60.0 && s.guess_bits < MIN_BITS, "{s:?}");
        assert!(!s.ok);
    }

    #[test]
    fn generated_passphrases_pass() {
        for _ in 0..50 {
            let p = generate();
            assert_eq!(p.split(' ').count(), DICEWARE_WORDS);
            let s = assess(&p);
            assert!(s.ok, "{p:?} -> {s:?}");
        }
        assert_ne!(generate(), generate());
    }

    #[test]
    fn long_random_text_passes() {
        assert!(assess("Xk9#mQ2$vL7!pR4&wZ8@jT5").ok);
    }
}
