//! Embed one receiver's symbols into an image.

use super::color::{from_ycrcb, to_ycrcb};
use super::common::{block_dct, block_idct, layout, pad8, positions, strengths, ImageParams, NPOS};
use super::Bgr;

/// `symbols`: `params.nslots` bits (0/1). Returns the marked image.
pub fn embed_image(img: &Bgr, symbols: &[u8], seed: u64, params: &ImageParams) -> Result<Bgr, String> {
    if symbols.len() != params.nslots {
        return Err(format!("need {} symbols, got {}", params.nslots, symbols.len()));
    }
    let (h, w) = (img.h, img.w);
    let (y8, cr, cb) = to_ycrcb(img);
    let y: Vec<f32> = y8.iter().map(|&v| v as f32).collect();
    let (plane, ph, pw) = pad8(&y, h, w);
    let mut coef = block_dct(&plane, ph, pw);
    let (slot, pat) = layout(ph / 8, pw / 8, seed, params.nslots);
    let alpha = strengths(&coef, params.strength);
    let pos = positions();
    for (b, c) in coef.iter_mut().enumerate() {
        let sign = 2.0 * symbols[slot[b] as usize] as f32 - 1.0;
        for (k, &(u, v)) in pos.iter().enumerate() {
            c[u * 8 + v] += alpha[b] * sign * pat[b * NPOS + k];
        }
    }
    let y2 = block_idct(&coef, ph, pw);
    let mut yout = vec![0u8; h * w];
    for r in 0..h {
        for c in 0..w {
            yout[r * w + c] = y2[r * pw + c].round_ties_even().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(from_ycrcb(h, w, &yout, &cr, &cb))
}
