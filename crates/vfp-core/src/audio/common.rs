//! Audio watermark parameters and layout (same as `audio/common.py`).

use crate::rng::Pcg64;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioParams {
    /// One group of symbols per segment.
    pub seg_seconds: f64,
    /// Symbols per segment.
    pub nslots: usize,
    /// Relative size of the mark (0.05 is about -26 dB under the signal).
    pub alpha: f64,
    pub nfft: usize,
    pub f_lo: f64,
    pub f_hi: f64,
}

impl Default for AudioParams {
    fn default() -> Self {
        AudioParams { seg_seconds: 1.0, nslots: 32, alpha: 0.05, nfft: 2048, f_lo: 300.0, f_hi: 8000.0 }
    }
}

impl AudioParams {
    pub fn hop(&self) -> usize {
        self.nfft / 2
    }

    /// Bin range [b0, b1) that carries the mark.
    pub fn band(&self, sr: u32) -> (usize, usize) {
        let b0 = (self.f_lo * self.nfft as f64 / sr as f64).ceil() as usize;
        let b1 = ((self.f_hi * self.nfft as f64 / sr as f64) as usize).min(self.nfft / 2);
        (b0, b1)
    }

    /// STFT frames in one segment.
    pub fn frames_per_seg(&self, sr: u32) -> usize {
        ((self.seg_seconds * sr as f64 / self.hop() as f64).round_ties_even() as usize).max(1)
    }

    /// Number of Tardos symbols (code length m) for audio of this length.
    pub fn capacity(&self, duration: f64) -> usize {
        ((duration / self.seg_seconds).ceil() as usize + 1) * self.nslots
    }
}

pub const AUDIO_NSLOTS_MAX: usize = 256;

/// Symbols per segment so a clip of this length reaches `target` symbols.
/// Long audio keeps the default; short audio packs more. Capped for reliability.
pub fn auto_nslots(duration: f64, p: &AudioParams, target: usize) -> usize {
    let nseg = (duration / p.seg_seconds).ceil() as usize + 1;
    let c = (target as f64 / nseg as f64).ceil() as usize;
    let k = c.div_ceil(8) * 8;
    k.max(p.nslots).min(AUDIO_NSLOTS_MAX)
}

/// Keyed +-1 pattern and keyed time-frequency cell -> slot map for one segment (frames x bins).
pub fn seg_layout(seed: u64, seg: u64, nframes: usize, nbins: usize, nslots: usize) -> (Vec<f32>, Vec<u32>) {
    let mut rng = Pcg64::from_seed(&[seed, 0xA0D10, seg]);
    let w = rng.signs_pm1(nframes * nbins);
    let slot = rng.permutation(nframes * nbins).into_iter().map(|p| (p % nslots as u64) as u32).collect();
    (w, slot)
}
