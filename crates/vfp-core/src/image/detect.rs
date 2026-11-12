//! Read symbols from a leaked image that is already aligned to the original.
//!
//! `detect_image` aligns first (see `register.rs`), then reads the symbols.

use super::color::to_ycrcb;
use super::common::{block_dct, layout, pad8, positions, strengths, ImageParams, NPOS};
use super::register::register;
use super::Bgr;

pub struct ImageDetection {
    pub z: Vec<f64>,
    pub bits: Vec<u8>,
    pub confidence: Vec<f64>,
    pub valid_fraction: f64,
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { 0.5 * (v[n / 2 - 1] + v[n / 2]) }
}

fn luma_padded(img: &Bgr) -> (Vec<f32>, usize, usize) {
    let (y8, _, _) = to_ycrcb(img);
    let y: Vec<f32> = y8.iter().map(|&v| v as f32).collect();
    pad8(&y, img.h, img.w)
}

/// `valid`: per-pixel mask of the aligned leak (h*w), or `None` when every pixel is valid.
pub fn detect_aligned(
    leaked: &Bgr,
    original: &Bgr,
    valid: Option<&[bool]>,
    seed: u64,
    params: &ImageParams,
) -> ImageDetection {
    assert_eq!((leaked.h, leaked.w), (original.h, original.w), "images must be aligned");
    let (yo, ph, pw) = luma_padded(original);
    let (yl, _, _) = luma_padded(leaked);
    let (co, cl) = (block_dct(&yo, ph, pw), block_dct(&yl, ph, pw));
    let (nbh, nbw) = (ph / 8, pw / 8);
    let (slot, pat) = layout(nbh, nbw, seed, params.nslots);
    let alpha = strengths(&co, params.strength);

    // A block counts only if all its pixels are valid (edge-padded like the Python code).
    let bvalid: Vec<bool> = (0..nbh * nbw)
        .map(|b| match valid {
            None => true,
            Some(m) => {
                let (by, bx) = (b / nbw, b % nbw);
                (0..8).all(|r| {
                    (0..8).all(|c| {
                        let (rr, cc) = ((by * 8 + r).min(original.h - 1), (bx * 8 + c).min(original.w - 1));
                        m[rr * original.w + cc]
                    })
                })
            }
        })
        .collect();

    let pos = positions();
    let n = params.nslots;
    let (mut num, mut den) = (vec![0f64; n], vec![0f64; n]);
    let mut dv: Vec<f64> = Vec::new();
    for b in 0..nbh * nbw {
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
    }
    .max(1e-3);

    let z: Vec<f64> = (0..n)
        .map(|s| if den[s] > 0.0 { num[s] / (sigma * sigma * NPOS as f64 * den[s].max(1e-9)).sqrt() } else { 0.0 })
        .collect();
    ImageDetection {
        bits: z.iter().map(|&x| (x > 0.0) as u8).collect(),
        confidence: z.iter().map(|x| (x.abs() / 3.0).clamp(0.0, 1.0)).collect(),
        valid_fraction: bvalid.iter().filter(|&&b| b).count() as f64 / bvalid.len() as f64,
        z,
    }
}

/// Align the leak to the original, then read the symbols.
pub fn detect_image(leaked: &Bgr, original: &Bgr, seed: u64, params: &ImageParams) -> (ImageDetection, super::register::Registration) {
    let (aligned, valid, reg) = register(leaked, original);
    (detect_aligned(&aligned, original, Some(&valid), seed, params), reg)
}
