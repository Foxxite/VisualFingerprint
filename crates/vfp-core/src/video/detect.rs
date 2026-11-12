//! Read symbols from a leaked video, using the original as reference.

use super::common::{canon_size, capacity, effective_strength, seg_frames, segment_seed, VideoParams};
use super::ffio::{decode_thumbs, decode_yuv, probe, FrameReader, VideoInfo};
use super::sync::find_offsets;
use crate::error::Result;
use crate::image::common::NPOS;
use crate::image::plane::{block_valid, make_layout, plane_stats};
use crate::image::register::{apply_warp_plane, estimate_warp_gray};
use crate::image::warp::{resize_area_u8, Mat3, Plane};
use std::collections::HashMap;
use std::path::Path;

/// Leaked frames buffered for sync and warp estimation.
const HEAD: usize = 60;
/// Leaked frames used to confirm the time offset: spread over several segments, so a wrong
/// offset lands some frames in the wrong segment.
const SAMPLE_STEP: usize = 2;

pub struct VideoDetection {
    pub z: Vec<f64>,
    pub bits: Vec<u8>,
    pub confidence: Vec<f64>,
    /// Share of symbols that had any evidence.
    pub coverage: f64,
    pub tau: i64,
    pub sync_quality: f64,
    pub sync_energy: f64,
    pub warp: String,
    pub warp_err: f64,
    pub frames_used: usize,
}

/// An 8-bit luma frame.
#[derive(Clone)]
struct Luma {
    h: usize,
    w: usize,
    d: Vec<u8>,
}

impl Luma {
    fn plane(&self) -> Plane {
        Plane { h: self.h, w: self.w, d: self.d.iter().map(|&v| v as f32).collect() }
    }
}

/// Resize to the working size if needed.
fn canon(y: &Luma, cw: usize, ch: usize) -> Luma {
    if (y.h, y.w) == (ch, cw) {
        return y.clone();
    }
    Luma { h: ch, w: cw, d: resize_area_u8(&y.d, y.h, y.w, ch, cw) }
}

/// Shrink a big leaked frame to about the working scale before warping.
fn prescale(y: Luma, cw: usize) -> Luma {
    let f = cw as f64 / y.w as f64;
    if f < 0.75 {
        let (nw, nh) = ((y.w as f64 * f).round_ties_even() as usize, (y.h as f64 * f).round_ties_even() as usize);
        return Luma { h: nh, w: nw, d: resize_area_u8(&y.d, y.h, y.w, nh, nw) };
    }
    y
}

fn seek_start(info: &VideoInfo, start: i64) -> f64 {
    if start > 0 { ((start as f64 - 0.25) / info.fps()).max(0.0) } else { 0.0 }
}

/// Walk the original frames forward from `start`. Frame numbers must not decrease.
struct Cursor {
    idx: i64,
    reader: Option<FrameReader>,
    cur: Option<Luma>,
    dims: (usize, usize),
}

impl Cursor {
    fn new(info: &VideoInfo, start: i64) -> Result<Self> {
        Ok(Cursor { idx: start - 1, reader: Some(decode_yuv(info, None, seek_start(info, start), None)?), cur: None, dims: (info.height, info.width) })
    }

    fn get(&mut self, j: i64) -> Option<Luma> {
        while self.idx < j {
            let fr = self.reader.as_mut()?.next()?;
            self.cur = Some(Luma { h: self.dims.0, w: self.dims.1, d: fr.y });
            self.idx += 1;
        }
        if self.idx == j { self.cur.clone() } else { None }
    }
}

/// Decode a few original frames (by number) at working size.
fn grab(info: &VideoInfo, js: &[i64], cw: usize, ch: usize) -> Result<HashMap<i64, Luma>> {
    let mut js: Vec<i64> = js.iter().copied().filter(|&j| j >= 0).collect();
    js.sort_unstable();
    js.dedup();
    let mut out = HashMap::new();
    let (Some(&start), Some(&end)) = (js.first(), js.last()) else { return Ok(out) };
    let reader = decode_yuv(info, None, seek_start(info, start), Some((end - start + 1) as usize))?;
    for (k, fr) in reader.enumerate() {
        let j = start + k as i64;
        if js.binary_search(&j).is_ok() {
            out.insert(j, canon(&Luma { h: info.height, w: info.width, d: fr.y }, cw, ch));
        }
    }
    Ok(out)
}

type Layout = (Vec<u32>, Vec<f32>);

struct Layouts<'a> {
    cache: HashMap<u64, Layout>,
    seed: u64,
    ch: usize,
    cw: usize,
    k: usize,
    _p: std::marker::PhantomData<&'a ()>,
}

impl Layouts<'_> {
    fn get(&mut self, seg: u64) -> &Layout {
        let (seed, ch, cw, k) = (self.seed, self.ch, self.cw, self.k);
        self.cache.entry(seg).or_insert_with(|| make_layout(ch, cw, &segment_seed(seed, seg), k))
    }
}

pub fn detect_video(
    leaked: &Path, original: &Path, seed: u64, p: &VideoParams, m: Option<usize>, progress: Option<&dyn Fn(usize, usize)>,
) -> Result<VideoDetection> {
    let (io, il) = (probe(original)?, probe(leaked)?);
    let (sf, k) = (seg_frames(&io, p), p.nslots);
    let m = m.unwrap_or_else(|| capacity(&io, p, None));
    let (cw, ch) = canon_size(io.width, io.height);
    let (fps_o, fps_l) = (io.fps(), il.fps());
    let strength = effective_strength(io.width, io.height, p);

    // 1. candidate time offsets from thumbnails
    let (to, tl) = (decode_thumbs(&io, (32, 18))?, decode_thumbs(&il, (32, 18))?);
    let cands = find_offsets(&to, fps_o, &tl, fps_l);
    let ratio = fps_o / fps_l;

    let mut leaked_frames = decode_yuv(&il, None, 0.0, None)?
        .enumerate()
        .map(|(i, f)| (i, prescale(Luma { h: il.height, w: il.width, d: f.y }, cw)));
    let head: Vec<(usize, Luma)> = leaked_frames.by_ref().take(HEAD).collect();
    let mut sample: Vec<&(usize, Luma)> = (0..HEAD).step_by(SAMPLE_STEP).filter(|&k| k < head.len()).map(|k| &head[k]).collect();
    if sample.is_empty() {
        sample = head.iter().take(1).collect();
    }

    let mut layouts = Layouts { cache: HashMap::new(), seed, ch, cw, k, _p: Default::default() };
    let jmap_with = |i: usize, tau: i64| (i as f64 * ratio).round_ties_even() as i64 + tau;

    // 2. confirm the offset with the watermark: the right one shows strong correlation
    struct Cand {
        score: f64,
        tau: i64,
        dist: f64,
        m: Option<Mat3>,
        method: String,
        err: f64,
    }
    let mut results: Vec<Cand> = vec![];
    for &(tau, dist) in &cands {
        let js: Vec<i64> = sample.iter().map(|(i, _)| jmap_with(*i, tau)).collect();
        let orig_frames = grab(&io, &js, cw, ch)?;
        let pairs: Vec<(&Luma, &Luma, i64)> = sample.iter().zip(&js).filter_map(|((_, yl), &j)| orig_frames.get(&j).map(|yo| (yl, yo, j))).collect();
        let Some(first) = pairs.first() else { continue };
        let reg = estimate_warp_gray(&first.0.plane(), &first.1.plane());
        let mut energy = vec![];
        for (yl, yo, j) in &pairs {
            let seg = (*j as usize) / sf;
            if (seg + 1) * k > m {
                continue;
            }
            let (al, valid) = apply_warp_plane(&yl.plane(), reg.m.as_ref(), ch, cw);
            let bv = block_valid(&valid, ch, cw);
            // Score = energy with the real pattern minus energy with a control pattern (a segment
            // that does not exist). Picture mismatch raises both equally; only a real mark raises the first.
            let mut e = [0f64; 2];
            for (n, sg) in [seg as u64, 100_000 + seg as u64].into_iter().enumerate() {
                let (slot, pat) = layouts.get(sg);
                let (num, den, sig) = plane_stats(&yo.plane().d, &al.d, ch, cw, &bv, slot, pat, strength, k);
                let zs: Vec<f64> = (0..k).filter(|&s| den[s] > 0.0).map(|s| (num[s] / (sig * (NPOS as f64 * den[s]).sqrt())).powi(2)).collect();
                e[n] = if zs.is_empty() { 0.0 } else { zs.iter().sum::<f64>() / zs.len() as f64 };
            }
            energy.push(e[0] - e[1]);
        }
        let score = if energy.is_empty() { 0.0 } else { energy.iter().sum::<f64>() / energy.len() as f64 };
        if std::env::var_os("VFP_TRACE").is_some() {
            eprintln!("    [video] offset {tau}: dist {dist:.3}, watermark score {score:.1} from {} frames, warp {}", energy.len(), reg.method);
        }
        results.push(Cand { score, tau, dist, m: reg.m, method: reg.method.to_string(), err: reg.err });
    }
    let best = if results.is_empty() {
        Cand { score: 0.0, tau: 0, dist: 1.0, m: None, method: "none".into(), err: 0.0 }
    } else {
        let top = results.iter().map(|r| r.score).fold(f64::MIN, f64::max);
        // Prior: most leaks are not trimmed. If "no shift" scores close to the best, take it.
        // A real trim wins by a wide margin and is not affected.
        let mut bi = 0;
        for (i, r) in results.iter().enumerate() {
            if r.score > results[bi].score {
                bi = i;
            }
        }
        if let Some(zi) = results.iter().position(|r| r.tau == 0 && r.score >= 0.6 * top) {
            bi = zi;
        }
        results.swap_remove(bi)
    };
    let (tau, mw) = (best.tau, best.m);

    // 3. accumulate evidence per symbol over all frames
    let jmap = |i: usize| jmap_with(i, tau);
    let mut orig = Cursor::new(&io, jmap(0).max(0))?;
    let (mut acc_num, mut acc_den) = (vec![0f64; m], vec![0f64; m]);
    let mut n_done = 0usize;
    let rest = leaked_frames; // continues after the buffered head
    for (i, yl) in head.iter().cloned().chain(rest) {
        if i % p.frame_stride != 0 {
            continue;
        }
        let j = jmap(i);
        if j < 0 {
            continue;
        }
        let seg = (j as usize) / sf;
        if (seg + 1) * k > m {
            break;
        }
        let Some(yo) = orig.get(j) else { continue };
        let (slot, pat) = layouts.get(seg as u64);
        let (al, valid) = apply_warp_plane(&yl.plane(), mw.as_ref(), ch, cw);
        let bv = block_valid(&valid, ch, cw);
        let (num, den, sig) = plane_stats(&canon(&yo, cw, ch).plane().d, &al.d, ch, cw, &bv, slot, pat, strength, k);
        for s in 0..k {
            acc_num[seg * k + s] += num[s] / (sig * sig);
            acc_den[seg * k + s] += den[s] / (sig * sig);
        }
        n_done += 1;
        if let Some(cb) = progress {
            if n_done % 10 == 0 {
                cb(n_done, tl.len());
            }
        }
        if layouts.cache.len() > 4 {
            if let Some(&oldest) = layouts.cache.keys().min() {
                layouts.cache.remove(&oldest);
            }
        }
    }

    let z: Vec<f64> = (0..m).map(|i| if acc_den[i] > 0.0 { acc_num[i] / (NPOS as f64 * acc_den[i].max(1e-12)).sqrt() } else { 0.0 }).collect();
    Ok(VideoDetection {
        bits: z.iter().map(|&v| (v > 0.0) as u8).collect(),
        confidence: z.iter().map(|v| (v.abs() / 3.0).clamp(0.0, 1.0)).collect(),
        coverage: acc_den.iter().filter(|&&d| d > 0.0).count() as f64 / m as f64,
        z,
        tau,
        sync_quality: best.dist,
        sync_energy: best.score,
        warp: best.method,
        warp_err: best.err,
        frames_used: n_done,
    })
}
