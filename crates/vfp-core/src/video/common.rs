//! Video watermark parameters and sizes (same as `video/common.py`).

use super::ffio::VideoInfo;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoParams {
    /// One group of symbols per segment.
    pub seg_seconds: f64,
    /// Symbols per segment (spatial slots).
    pub nslots: usize,
    pub strength: f64,
    /// x264 quality of the marked copy.
    pub crf: i32,
    pub preset: String,
    /// Detect: use every n-th frame.
    pub frame_stride: usize,
}

impl Default for VideoParams {
    fn default() -> Self {
        VideoParams { seg_seconds: 0.5, nslots: 64, strength: 3.0, crf: 16, preset: "medium".into(), frame_stride: 1 }
    }
}

pub fn seg_frames(info: &VideoInfo, p: &VideoParams) -> usize {
    ((p.seg_seconds * info.fps()).round_ties_even() as usize).max(1)
}

/// Number of Tardos symbols (code length m) this video can carry.
pub fn capacity(info: &VideoInfo, p: &VideoParams, max_seconds: Option<f64>) -> usize {
    let mut n = info.nframes;
    if let Some(s) = max_seconds {
        n = n.min((s * info.fps()) as usize + 1);
    }
    let nseg = n.div_ceil(seg_frames(info, p)) + 1; // +1 absorbs frame-count estimate errors
    nseg * p.nslots
}

const CANON_AREA: f64 = 1280.0 * 720.0;

/// Working size of the mark. Big frames are marked at about 720p and scaled up,
/// so a later big downscale still meets a coarse mark.
pub fn canon_size(width: usize, height: usize) -> (usize, usize) {
    let s = (CANON_AREA / (width * height) as f64).sqrt().min(1.0);
    if s > 0.97 {
        return (width, height);
    }
    (((width as f64 * s).round_ties_even() as usize).max(16), ((height as f64 * s).round_ties_even() as usize).max(16))
}

/// Symbols per 0.5 s segment. Measured: up to 256 still traced harsh attacks; at 512 one failed.
pub const VIDEO_NSLOTS_MAX: usize = 256;

/// Symbols per segment so a clip of this length reaches `target` symbols.
pub fn auto_nslots(info: &VideoInfo, p: &VideoParams, target: usize) -> usize {
    let nseg = info.nframes.div_ceil(seg_frames(info, p)) + 1;
    let c = (target as f64 / nseg as f64).ceil() as usize;
    (c.div_ceil(8) * 8).max(p.nslots).min(VIDEO_NSLOTS_MAX)
}

/// A coarser mark is more visible at equal amplitude, so big frames get less.
pub fn effective_strength(width: usize, height: usize, p: &VideoParams) -> f32 {
    let (cw, _) = canon_size(width, height);
    (p.strength * (cw as f64 / width as f64).sqrt()) as f32
}

pub fn segment_seed(seed: u64, seg: u64) -> [u64; 3] {
    [seed, 0x7E6, seg]
}
