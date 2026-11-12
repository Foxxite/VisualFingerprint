//! Embed one receiver's symbols into audio.
//!
//! Each time-frequency cell of the STFT is scaled by `1 + alpha * symbol * pattern`.
//! The mark follows the signal, so quiet parts get a quiet mark.

use super::common::{seg_layout, AudioParams};
use super::dsp::Stft;
use crate::error::{err, Result};

/// `x`: one `Vec<f32>` per channel. Returns the marked samples, same shape.
pub fn embed_audio_array(x: &[Vec<f32>], sr: u32, symbols: &[u8], seed: u64, p: &AudioParams) -> Result<Vec<Vec<f32>>> {
    let n = x.first().map_or(0, |c| c.len());
    let need = p.capacity(n as f64 / sr as f64);
    if symbols.len() < need {
        return err(format!("need {need} symbols for this audio, got {}", symbols.len()));
    }
    let stft = Stft::new(p.nfft, p.hop());
    let mut z: Vec<_> = x.iter().map(|ch| stft.forward(ch, true, true)).collect();
    let (b0, b1) = p.band(sr);
    let (fs, k) = (p.frames_per_seg(sr), p.nslots);
    let nt = z[0].len();
    let nb = b1 - b0;
    let alpha = p.alpha as f32;
    for seg in 0..nt.div_ceil(fs) {
        let (c0, c1) = (seg * fs, nt.min((seg + 1) * fs));
        let (w, slot) = seg_layout(seed, seg as u64, fs, nb, k);
        let sym = &symbols[seg * k..(seg + 1) * k];
        for zc in z.iter_mut() {
            for c in 0..c1 - c0 {
                for j in 0..nb {
                    let s = 2.0 * sym[slot[c * nb + j] as usize] as f32 - 1.0;
                    zc[c0 + c][b0 + j] *= 1.0 + alpha * s * w[c * nb + j];
                }
            }
        }
    }
    Ok(z.iter().map(|zc| stft.inverse(zc, true)[..n].iter().map(|v| v.clamp(-1.0, 1.0)).collect()).collect())
}
