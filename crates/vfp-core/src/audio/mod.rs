//! Audio watermark: STFT cell scaling, offset search, informed detection.

pub mod align;
pub mod common;
pub mod detect;
pub mod dsp;
pub mod embed;
pub mod ffio;

use crate::error::{err, Result};
use common::AudioParams;
use std::path::Path;

/// Mark an audio file. The output codec follows the extension of `dst`.
pub fn embed_audio(src: &Path, dst: &Path, symbols: &[u8], seed: u64, p: &AudioParams) -> Result<()> {
    let info = ffio::probe_audio(src)?;
    if info.channels > 2 {
        return err("only mono and stereo are supported");
    }
    let x = ffio::decode_audio(src, None, None)?;
    let y = embed::embed_audio_array(&x, info.sr, symbols, seed, p)?;
    ffio::encode_audio(&y, info.sr, dst)
}

/// Read the symbols from a leaked file, using the original as reference.
pub fn detect_audio(leaked: &Path, original: &Path, seed: u64, p: &AudioParams, m: usize) -> Result<detect::AudioDetection> {
    let info = ffio::probe_audio(original)?;
    let xo = ffio::decode_audio(original, None, Some(1))?.remove(0);
    let yl = ffio::decode_audio(leaked, Some(info.sr), Some(1))?.remove(0);
    Ok(detect::detect_arrays(&xo, &yl, info.sr, seed, p, m))
}
