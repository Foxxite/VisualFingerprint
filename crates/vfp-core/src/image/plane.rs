//! Luma-plane embed and detect primitives shared by images and video frames.

use super::common::{block_dct, block_idct, grid, layout_parts, pad8, positions, strengths, NPOS};
use crate::perceptual::block_mask;

/// Keyed block -> slot map and pattern for an h x w plane.
pub fn make_layout(h: usize, w: usize, parts: &[u64], nslots: usize) -> (Vec<u32>, Vec<f32>) {
    let (nbh, nbw) = grid(h, w);
    layout_parts(nbh, nbw, parts, nslots)
}

/// `y`: float luma (h x w). `sym_pm1`: +-1 per slot. Returns the marked float plane.
pub fn embed_plane(y: &[f32], h: usize, w: usize, sym_pm1: &[f32], slot: &[u32], pat: &[f32], strength: f32) -> Vec<f32> {
    let (plane, ph, pw) = pad8(y, h, w);
    let mut coef = block_dct(&plane, ph, pw);
    let alpha = strengths(&coef, strength);
    let pos = positions();
    for (b, c) in coef.iter_mut().enumerate() {
        let s = sym_pm1[slot[b] as usize];
        for (k, &(u, v)) in pos.iter().enumerate() {
            c[u * 8 + v] += alpha[b] * s * pat[b * NPOS + k];
        }
    }
    let y2 = block_idct(&coef, ph, pw);
    let mut out = Vec::with_capacity(h * w);
    for r in 0..h {
        out.extend_from_slice(&y2[r * pw..r * pw + w]);
    }
    out
}

/// Pixel valid mask (h x w) -> per-block bool. A block counts only if all its pixels do.
pub fn block_valid(valid: &[bool], h: usize, w: usize) -> Vec<bool> {
    let (nbh, nbw) = grid(h, w);
    (0..nbh * nbw)
        .map(|b| {
            let (by, bx) = (b / nbw, b % nbw);
            (0..8).all(|r| (0..8).all(|c| valid[((by * 8 + r).min(h - 1)) * w + (bx * 8 + c).min(w - 1)]))
        })
        .collect()
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { 0.5 * (v[n / 2 - 1] + v[n / 2]) }
}

/// Correlate (leaked - original) with the key pattern, per slot.
/// Returns (num, den, sigma). Per-slot z = num / (sigma * sqrt(NPOS * den)).
/// Frames add up: sum num/sigma^2 and den/sigma^2 over frames.
#[allow(clippy::too_many_arguments)]
pub fn plane_stats(
    y_orig: &[f32], y_leak: &[f32], h: usize, w: usize, bvalid: &[bool], slot: &[u32], pat: &[f32],
    strength: f32, nslots: usize,
) -> (Vec<f64>, Vec<f64>, f64) {
    let (po, ph, pw) = pad8(y_orig, h, w);
    let (pl, _, _) = pad8(y_leak, h, w);
    let (co, cl) = (block_dct(&po, ph, pw), block_dct(&pl, ph, pw));
    let alpha: Vec<f32> = co.iter().map(|c| strength * block_mask(c)).collect();
    let pos = positions();
    let (mut num, mut den) = (vec![0f64; nslots], vec![0f64; nslots]);
    let mut dv: Vec<f64> = Vec::new();
    for b in 0..co.len() {
        if !bvalid[b] {
            continue;
        }
        let mut proj = 0f64;
        for (k, &(u, v)) in pos.iter().enumerate() {
            let d = (cl[b][u * 8 + v] - co[b][u * 8 + v]) as f64;
            dv.push(d);
            proj += d * pat[b * NPOS + k] as f64;
        }
        let a = alpha[b] as f64;
        num[slot[b] as usize] += a * proj;
        den[slot[b] as usize] += a * a;
    }
    let sigma = if dv.is_empty() {
        1.0
    } else {
        let m = median(&mut dv.clone());
        let mut dev: Vec<f64> = dv.iter().map(|x| (x - m).abs()).collect();
        1.4826 * median(&mut dev)
    };
    (num, den, sigma.max(1e-3))
}
