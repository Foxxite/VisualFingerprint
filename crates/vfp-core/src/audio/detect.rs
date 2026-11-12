//! Read symbols from a leaked audio file, using the original as reference.

use super::align::find_offset;
use super::common::{seg_layout, AudioParams};
use super::dsp::Stft;

/// Bins per local gain estimate.
const GAIN_GROUP: usize = 96;

pub struct AudioDetection {
    pub z: Vec<f64>,
    pub bits: Vec<u8>,
    pub confidence: Vec<f64>,
    /// Share of code symbols that got any evidence.
    pub coverage: f64,
    pub offset: i64,
    pub align: f64,
    pub gain: f64,
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

fn finish(z: Vec<f64>, coverage: f64, offset: i64, align: f64, gain: f64) -> AudioDetection {
    AudioDetection {
        bits: z.iter().map(|&v| (v > 0.0) as u8).collect(),
        confidence: z.iter().map(|v| (v.abs() / 3.0).clamp(0.0, 1.0)).collect(),
        z,
        coverage,
        offset,
        align,
        gain,
    }
}

/// `xo`, `yl`: mono arrays at the same sample rate `sr`. `m`: code length (symbols).
pub fn detect_arrays(xo: &[f32], yl: &[f32], sr: u32, seed: u64, p: &AudioParams, m: usize) -> AudioDetection {
    let (off, score) = find_offset(xo, yl, sr);
    let h = p.hop();
    let lo = (off.max(0) as usize).div_ceil(h) * h; // start on a frame boundary of the original
    let hi = (xo.len() as i64).min(off + yl.len() as i64);
    if hi - (lo as i64) < (p.nfft * 2) as i64 {
        return finish(vec![0.0; m], 0.0, off, score, 1.0);
    }
    let hi = hi as usize;
    let xr = &xo[lo..hi];
    let yr0 = &yl[(lo as i64 - off) as usize..(hi as i64 - off) as usize];
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum::<f64>();
    let gain = dot(yr0, xr) / (dot(xr, xr) + 1e-12);
    let yr: Vec<f32> = if gain.abs() > 1e-3 { yr0.iter().map(|&v| (v as f64 / gain) as f32).collect() } else { yr0.to_vec() };

    let stft = Stft::new(p.nfft, h);
    let (zo, zl) = (stft.forward(xr, false, false), stft.forward(&yr, false, false));
    let (b0, b1) = p.band(sr);
    let (nb, nf) = (b1 - b0, zo.len());

    // frame-by-frame work
    let mut absz = vec![0f64; nf * nb];
    let mut wn = vec![0f64; nf * nb];
    let mut wd = vec![0f64; nf * nb];
    for t in 0..nf {
        let (o, l) = (&zo[t][b0..b1], &zl[t][b0..b1]);
        for j in 0..nb {
            absz[t * nb + j] = ((o[j].re * o[j].re + o[j].im * o[j].im) as f64).sqrt();
        }
        // undo level changes that vary over time and frequency: best gain per group of bins
        let mut lg = vec![0f64; nb]; // leaked, gain-corrected, real and imaginary parts
        let mut li = vec![0f64; nb];
        for g0 in (0..nb).step_by(GAIN_GROUP) {
            let g1 = (g0 + GAIN_GROUP).min(nb);
            let (mut num, mut den) = (0f64, 0f64);
            for j in g0..g1 {
                num += (l[j].re * o[j].re + l[j].im * o[j].im) as f64;
                den += absz[t * nb + j].powi(2);
            }
            let g = num / (den + 1e-12);
            let g = if g.abs() > 1e-3 { g } else { 1.0 };
            for j in g0..g1 {
                lg[j] = l[j].re as f64 / g;
                li[j] = l[j].im as f64 / g;
            }
        }
        // change along the signal
        let mut proj: Vec<f64> = (0..nb)
            .map(|j| ((lg[j] - o[j].re as f64) * o[j].re as f64 + (li[j] - o[j].im as f64) * o[j].im as f64) / (absz[t * nb + j] + 1e-9))
            .collect();
        let med = median(&mut proj.clone());
        let mut dev: Vec<f64> = proj.iter().map(|v| (v - med).abs()).collect();
        let sigma = (1.4826 * median(&mut dev)).max(1e-7);
        for j in 0..nb {
            let a = absz[t * nb + j];
            wn[t * nb + j] = a * proj[j] / (sigma * sigma);
            wd[t * nb + j] = a * a / (sigma * sigma);
        }
        proj.clear();
    }

    let (fs, k) = (p.frames_per_seg(sr), p.nslots);
    let (mut acc_num, mut acc_den) = (vec![0f64; m], vec![0f64; m]);
    let t_of = |j: usize| lo / h + 1 + j; // frame number on the original's grid
    let mut j = 0;
    while j < nf {
        let seg = t_of(j) / fs;
        let mut j1 = j;
        while j1 < nf && t_of(j1) / fs == seg {
            j1 += 1;
        }
        if (seg + 1) * k <= m {
            let (w, slot) = seg_layout(seed, seg as u64, fs, nb, k);
            for jj in j..j1 {
                let loc = t_of(jj) - seg * fs;
                for b in 0..nb {
                    let s = seg * k + slot[loc * nb + b] as usize;
                    acc_num[s] += wn[jj * nb + b] * w[loc * nb + b] as f64;
                    acc_den[s] += wd[jj * nb + b];
                }
            }
        }
        j = j1;
    }
    let z: Vec<f64> = (0..m).map(|i| if acc_den[i] > 0.0 { acc_num[i] / acc_den[i].max(1e-12).sqrt() } else { 0.0 }).collect();
    let coverage = acc_den.iter().filter(|&&d| d > 0.0).count() as f64 / m as f64;
    finish(z, coverage, off, score, gain)
}
