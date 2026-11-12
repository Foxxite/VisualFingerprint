//! Binary Tardos code with the Skoric symmetric accusation score.

use crate::rng::Pcg64;
use statrs::distribution::{ContinuousCDF, Normal};
use std::f64::consts::PI;

/// Smallest sum of squared weights we accuse on (about 25 reliable symbols).
pub const MIN_EVIDENCE: f64 = 25.0;

/// Code length for a false-accusation rate of about `eps1`.
pub fn required_length(n_users: u64, n_colluders: u64, eps1: f64) -> u64 {
    let eps_single = eps1 / n_users.max(1) as f64;
    (2.0 * PI * PI * (n_colluders * n_colluders) as f64 * (1.0 / eps_single).ln()).ceil() as u64
}

/// Largest colluder group the Tardos bound guarantees. 0 means one leaker only.
pub fn supported_colluders(m: u64, max_receivers: u64, eps: f64) -> u64 {
    (m as f64 / (2.0 * PI * PI * (max_receivers as f64 / eps).ln())).sqrt().floor() as u64
}

/// Upper-tail point of the standard normal: P(Z > x) = q.
fn norm_isf(q: f64) -> f64 {
    -Normal::new(0.0, 1.0).unwrap().inverse_cdf(q)
}

/// Score a user must beat. `weights` are the per-symbol confidences; `None` = all fully reliable.
pub fn accusation_threshold(n_users: u64, length: usize, eps1: f64, weights: Option<&[f64]>) -> f64 {
    let var = match weights {
        None => length as f64,
        Some(w) => w.iter().map(|x| x * x).sum(),
    };
    var.max(MIN_EVIDENCE).sqrt() * norm_isf(eps1 / n_users.max(1) as f64)
}

pub struct TardosCode {
    pub p: Vec<f64>,
    pub seed: u64,
}

impl TardosCode {
    pub fn create(length: usize, seed: u64, n_colluders: u64) -> Self {
        let mut rng = Pcg64::from_seed(&[seed]);
        let t = 1.0 / (300.0 * n_colluders as f64);
        let tp = t.sqrt().asin();
        let p = rng.uniform(tp, PI / 2.0 - tp, length).into_iter().map(|r| r.sin().powi(2)).collect();
        TardosCode { p, seed }
    }

    pub fn length(&self) -> usize {
        self.p.len()
    }

    /// Bit string for one user, derived from (seed, user_index).
    pub fn codeword(&self, user_index: u64) -> Vec<u8> {
        let mut rng = Pcg64::from_seed(&[self.seed, user_index + 1]);
        self.p.iter().map(|&p| (rng.next_f64() < p) as u8).collect()
    }

    /// Accusation score of one user for the recovered symbols `y` (0/1) and confidences.
    pub fn score(&self, user_index: u64, y: &[u8], conf: Option<&[f64]>) -> f64 {
        let x = self.codeword(user_index);
        let mut s = 0.0;
        for i in 0..self.length() {
            let p = self.p[i];
            let gp = ((1.0 - p) / p).sqrt();
            let gm = (p / (1.0 - p)).sqrt();
            let c = if y[i] == 1 {
                if x[i] == 1 { gp } else { -gm }
            } else if x[i] == 0 {
                gm
            } else {
                -gp
            };
            s += c * conf.map_or(1.0, |w| w[i]);
        }
        s
    }
}
