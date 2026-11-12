//! Find where a leaked audio file sits inside the original (sample-accurate).

use super::dsp::{butter_lowpass, correlate_valid, preemphasis, sosfiltfilt};

const BAND_HZ: f64 = 6000.0;

/// Whiten, then keep only the band every copy still has.
///
/// Whitening stops steady low notes from making many equal correlation peaks one period apart.
/// The low-pass drops the top octave, which codecs, resampling and filters remove anyway.
pub fn prep(a: &[f32], sr: u32) -> Vec<f64> {
    let x: Vec<f64> = a.iter().map(|&v| v as f64).collect();
    let e = preemphasis(&x, 0.97);
    let sos = butter_lowpass(6, BAND_HZ.min(sr as f64 * 0.45), sr as f64);
    sosfiltfilt(&sos, &e)
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Returns (off, score): leaked sample i matches original sample i + off.
/// `score` is the normalised correlation at the best place (1.0 = perfect).
pub fn find_offset(orig: &[f32], leak: &[f32], sr: u32) -> (i64, f64) {
    let srf = sr as f64;
    let d = ((srf / (2.5 * BAND_HZ)).floor() as usize).max(1);
    let (of, lf) = (prep(orig, sr), prep(leak, sr));
    let o: Vec<f64> = of.iter().step_by(d).copied().collect();
    let l: Vec<f64> = lf.iter().step_by(d).copied().collect();

    // keep a margin at the end: codecs pad the end of a file
    let margin = if l.len() > (2.0 * srf / d as f64) as usize { (0.3 * srf / d as f64) as usize } else { 0 };
    let w = (l.len() - margin).min((12.0 * srf / d as f64) as usize);
    // the loudest 12 s window in the first 90 s of the leaked file
    let step = ((3.0 * srf / d as f64) as usize).max(1);
    let limit = (l.len() - margin).min((90.0 * srf / d as f64) as usize);
    let n_starts = (limit.saturating_sub(w) + 1).max(1);
    let (mut st, mut best_e) = (0usize, f64::NEG_INFINITY);
    for s in (0..n_starts).step_by(step) {
        let e: f64 = l[s..(s + w).min(l.len())].iter().map(|v| v * v).sum();
        if e > best_e {
            best_e = e;
            st = s;
        }
    }
    let chunk = &l[st..st + w];
    if o.len() < w || w == 0 {
        return (0, 0.0);
    }
    let c = correlate_valid(&o, chunk);
    let mut cs = Vec::with_capacity(o.len() + 1);
    cs.push(0.0);
    let mut acc = 0.0;
    for v in &o {
        acc += v * v;
        cs.push(acc);
    }
    let chunk_e: f64 = chunk.iter().map(|v| v * v).sum();
    let (mut k0, mut best) = (0usize, f64::NEG_INFINITY);
    for (k, ck) in c.iter().enumerate() {
        let ew = cs[w + k] - cs[k];
        let score = ck / ((ew * chunk_e).sqrt() + 1e-12);
        if score > best {
            best = score;
            k0 = k;
        }
    }
    let off = (k0 as i64 - st as i64) * d as i64;

    // refine at full rate
    let lo = st * d;
    let ll = &lf[lo..(lo + w * d).min(lf.len())];
    let ll_norm = dot(ll, ll).sqrt();
    let (mut best_r, mut best_off) = (-2.0f64, off);
    for cand in (off - d as i64 - 2)..(off + d as i64 + 3) {
        let a = lo as i64 + cand;
        if a < 0 || a as usize + ll.len() > orig.len() {
            continue;
        }
        let seg = &of[a as usize..a as usize + ll.len()];
        let r = dot(ll, seg) / (ll_norm * dot(seg, seg).sqrt() + 1e-12);
        if r > best_r {
            best_r = r;
            best_off = cand;
        }
    }
    (best_off, best_r)
}
