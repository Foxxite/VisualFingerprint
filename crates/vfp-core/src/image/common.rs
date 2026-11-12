//! Shared layout and transform helpers for image embed and detect.

use crate::perceptual::block_mask;
use crate::rng::Pcg64;
use rayon::prelude::*;

/// Mid/low DCT positions (u, v) inside an 8x8 block.
pub fn positions() -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    for u in 0..5 {
        for w in 0..5 {
            if (2..=5).contains(&(u + w)) {
                v.push((u, w));
            }
        }
    }
    v
}
pub const NPOS: usize = 16;

#[derive(Clone, Copy, Debug)]
pub struct ImageParams {
    pub nslots: usize,
    pub strength: f32,
}

impl Default for ImageParams {
    fn default() -> Self {
        ImageParams { nslots: 256, strength: 3.0 }
    }
}

pub const MIN_BLOCKS_PER_SLOT: usize = 12;
pub const DEFAULT_SLOTS: usize = 1024;

/// Symbols an image of this size can carry (at most `wanted`, at least 8).
pub fn usable_slots(height: usize, width: usize, wanted: usize) -> usize {
    let blocks = (height / 8) * (width / 8);
    8.max(wanted.min(blocks / MIN_BLOCKS_PER_SLOT))
}

/// Block grid for a plane of h x w pixels (the plane is padded up to a multiple of 8).
pub fn grid(h: usize, w: usize) -> (usize, usize) {
    (h.div_ceil(8), w.div_ceil(8))
}

/// BGR -> luma as f32, padded on the bottom/right with the edge pixel to a multiple of 8.
pub fn pad8(y: &[f32], h: usize, w: usize) -> (Vec<f32>, usize, usize) {
    let (nbh, nbw) = grid(h, w);
    let (ph, pw) = (nbh * 8, nbw * 8);
    let mut out = vec![0f32; ph * pw];
    for r in 0..ph {
        let sr = r.min(h - 1);
        for c in 0..pw {
            out[r * pw + c] = y[sr * w + c.min(w - 1)];
        }
    }
    (out, ph, pw)
}

fn dct_matrix() -> [[f32; 8]; 8] {
    let mut d = [[0f32; 8]; 8];
    for (k, row) in d.iter_mut().enumerate() {
        let c = if k == 0 { (1.0f64 / 8.0).sqrt() } else { 0.5 };
        for (n, v) in row.iter_mut().enumerate() {
            *v = (c * (std::f64::consts::PI * (2 * n + 1) as f64 * k as f64 / 16.0).cos()) as f32;
        }
    }
    d
}

/// Orthonormal 8x8 DCT of every block. Output: nb blocks of 64 values (row-major, [u][v]).
pub fn block_dct(plane: &[f32], ph: usize, pw: usize) -> Vec<[f32; 64]> {
    let d = dct_matrix();
    let nbw = pw / 8;
    (0..(ph / 8) * nbw)
        .into_par_iter()
        .map(|i| {
            let (by, bx) = (i / nbw, i % nbw);
            let mut b = [[0f32; 8]; 8];
            for (r, row) in b.iter_mut().enumerate() {
                for (c, v) in row.iter_mut().enumerate() {
                    *v = plane[(by * 8 + r) * pw + bx * 8 + c];
                }
            }
            let mut t = [[0f32; 8]; 8]; // t = D * B
            for u in 0..8 {
                for c in 0..8 {
                    t[u][c] = (0..8).map(|r| d[u][r] * b[r][c]).sum();
                }
            }
            let mut o = [0f32; 64]; // o = t * D^T
            for u in 0..8 {
                for v in 0..8 {
                    o[u * 8 + v] = (0..8).map(|c| t[u][c] * d[v][c]).sum();
                }
            }
            o
        })
        .collect()
}

/// Inverse of `block_dct`. Returns the ph x pw plane.
pub fn block_idct(coef: &[[f32; 64]], ph: usize, pw: usize) -> Vec<f32> {
    let d = dct_matrix();
    let nbw = pw / 8;
    let blocks: Vec<[f32; 64]> = coef
        .par_iter()
        .map(|o| {
            let mut t = [[0f32; 8]; 8]; // t = D^T * O
            for r in 0..8 {
                for v in 0..8 {
                    t[r][v] = (0..8).map(|u| d[u][r] * o[u * 8 + v]).sum();
                }
            }
            let mut out = [0f32; 64];
            for r in 0..8 {
                for c in 0..8 {
                    out[r * 8 + c] = (0..8).map(|v| t[r][v] * d[v][c]).sum();
                }
            }
            out
        })
        .collect();
    let mut out = vec![0f32; ph * pw];
    for (i, blk) in blocks.iter().enumerate() {
        let (by, bx) = (i / nbw, i % nbw);
        for r in 0..8 {
            for c in 0..8 {
                out[(by * 8 + r) * pw + bx * 8 + c] = blk[r * 8 + c];
            }
        }
    }
    out
}

/// Keyed block -> slot map and keyed +-1 pattern (nb x NPOS, row-major).
pub fn layout(nbh: usize, nbw: usize, seed: u64, nslots: usize) -> (Vec<u32>, Vec<f32>) {
    layout_parts(nbh, nbw, &[seed], nslots)
}

/// Same, with the seed given as several integers (video uses `(seed, 0x7E6, segment)`).
pub fn layout_parts(nbh: usize, nbw: usize, parts: &[u64], nslots: usize) -> (Vec<u32>, Vec<f32>) {
    let nb = nbh * nbw;
    let mut key = parts.to_vec();
    key.push(0x1A6E);
    let mut rng = Pcg64::from_seed(&key);
    let slot = rng.permutation(nb).into_iter().map(|p| (p % nslots as u64) as u32).collect();
    let pat = rng.signs_pm1(nb * NPOS);
    (slot, pat)
}

/// Per-block mark strength: base strength times the perceptual mask of the original.
pub fn strengths(coef_orig: &[[f32; 64]], strength: f32) -> Vec<f32> {
    coef_orig.iter().map(|c| strength * block_mask(c)).collect()
}
