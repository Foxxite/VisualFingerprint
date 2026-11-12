//! Embed one receiver's symbols into a video.

use super::common::{canon_size, capacity, effective_strength, seg_frames, segment_seed, VideoParams};
use super::ffio::{decode_yuv, probe, Encoder};
use crate::error::{err, Result};
use crate::image::plane::{embed_plane, make_layout};
use crate::image::warp::{resize_area_u8, resize_cubic};
use rayon::prelude::*;
use std::path::Path;

pub type Progress<'a> = Option<&'a dyn Fn(usize, usize)>;

/// Write a marked copy of `src` to `dst`. `symbols`: bits, at least `capacity()` long.
pub fn embed_video(src: &Path, dst: &Path, symbols: &[u8], seed: u64, p: &VideoParams, progress: Progress) -> Result<()> {
    let info = probe(src)?;
    let need = capacity(&info, p, None);
    if symbols.len() < need {
        return err(format!("need {need} symbols for this video, got {}", symbols.len()));
    }
    let (sf, k) = (seg_frames(&info, p), p.nslots);
    let (w, h) = (info.width, info.height);
    let (cw, ch) = canon_size(w, h);
    let scaled = (cw, ch) != (w, h);
    let strength = effective_strength(w, h, p);

    let mut cur_seg = usize::MAX;
    let (mut slot, mut pat, mut sym) = (vec![], vec![], vec![]);
    let mut enc = Encoder::new(dst, &info, p.crf, &p.preset, Some(src), None)?;
    let mut n = 0usize;
    for fr in decode_yuv(&info, None, 0.0, None)? {
        let seg = n / sf;
        if seg != cur_seg {
            cur_seg = seg;
            (slot, pat) = make_layout(ch, cw, &segment_seed(seed, seg as u64), k);
            let part = symbols.get(seg * k..(seg + 1) * k).ok_or_else(|| crate::error::Error("the video has more frames than expected".into()))?;
            sym = part.iter().map(|&b| 2.0 * b as f32 - 1.0).collect();
        }
        let y2: Vec<f32> = if scaled {
            let yc: Vec<f32> = resize_area_u8(&fr.y, h, w, ch, cw).par_iter().map(|&v| v as f32).collect();
            let marked = embed_plane(&yc, ch, cw, &sym, &slot, &pat, strength);
            let delta: Vec<f32> = marked.par_iter().zip(&yc).map(|(a, b)| a - b).collect();
            let up = resize_cubic(&delta, ch, cw, h, w);
            fr.y.par_iter().zip(&up).map(|(&y, d)| y as f32 + d).collect()
        } else {
            let yf: Vec<f32> = fr.y.par_iter().map(|&v| v as f32).collect();
            embed_plane(&yf, h, w, &sym, &slot, &pat, strength)
        };
        let out: Vec<u8> = y2.par_iter().map(|v| v.round_ties_even().clamp(0.0, 255.0) as u8).collect();
        enc.write(&out, &fr.u, &fr.v)?;
        n += 1;
        if let Some(cb) = progress {
            if n % 10 == 0 {
                cb(n, info.nframes);
            }
        }
    }
    enc.finish()
}
