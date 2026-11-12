//! Signal-processing basics that the audio code needs, written to match SciPy:
//! periodic Hann window, STFT / inverse STFT, Butterworth low-pass (second-order sections),
//! zero-phase filtering (`sosfiltfilt`), and fast correlation.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;
use std::f64::consts::PI;
use std::sync::Arc;

pub type C32 = Complex<f32>;

/// `scipy.signal.get_window("hann", n)`: periodic.
pub fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()) as f32).collect()
}

pub struct Stft {
    pub nfft: usize,
    pub hop: usize,
    win: Vec<f32>,
    fwd: Arc<dyn realfft::RealToComplex<f32>>,
    inv: Arc<dyn realfft::ComplexToReal<f32>>,
}

impl Stft {
    pub fn new(nfft: usize, hop: usize) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        Stft { nfft, hop, win: hann(nfft), fwd: planner.plan_fft_forward(nfft), inv: planner.plan_fft_inverse(nfft) }
    }

    pub fn nbins(&self) -> usize {
        self.nfft / 2 + 1
    }

    /// Frames x bins, scaled like SciPy's `stft` (divided by the window sum).
    /// `boundary`: add nfft/2 zeros at both ends. `padded`: zero-pad the tail to fill the last frame.
    pub fn forward(&self, x: &[f32], boundary: bool, padded: bool) -> Vec<Vec<C32>> {
        let nfft = self.nfft;
        let mut xp: Vec<f32> = Vec::with_capacity(x.len() + nfft + self.hop);
        if boundary {
            xp.resize(nfft / 2, 0.0);
        }
        xp.extend_from_slice(x);
        if boundary {
            xp.resize(xp.len() + nfft / 2, 0.0);
        }
        if xp.len() < nfft {
            xp.resize(nfft, 0.0);
        } else if padded {
            let r = (xp.len() - nfft) % self.hop;
            if r != 0 {
                xp.resize(xp.len() + self.hop - r, 0.0);
            }
        }
        let nt = (xp.len() - nfft) / self.hop + 1;
        let scale = 1.0 / self.win.iter().sum::<f32>();
        let mut buf = vec![0f32; nfft];
        (0..nt)
            .map(|t| {
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = xp[t * self.hop + i] * self.win[i];
                }
                let mut out = self.fwd.make_output_vec();
                self.fwd.process(&mut buf, &mut out).expect("fft size");
                for v in out.iter_mut() {
                    *v *= scale;
                }
                out
            })
            .collect()
    }

    /// Overlap-add inverse. With `boundary`, drops the nfft/2 samples added by `forward`.
    pub fn inverse(&self, z: &[Vec<C32>], boundary: bool) -> Vec<f32> {
        let (nfft, nt) = (self.nfft, z.len());
        let total = nfft + (nt - 1) * self.hop;
        let (mut x, mut norm) = (vec![0f64; total], vec![0f64; total]);
        let wsum: f32 = self.win.iter().sum();
        let mut spec = self.inv.make_input_vec();
        let mut frame = vec![0f32; nfft];
        for (t, zt) in z.iter().enumerate() {
            spec.copy_from_slice(zt);
            spec[0].im = 0.0;
            spec[nfft / 2].im = 0.0;
            self.inv.process(&mut spec, &mut frame).expect("fft size");
            for i in 0..nfft {
                let w = self.win[i] as f64;
                x[t * self.hop + i] += (frame[i] / nfft as f32 * wsum) as f64 * w;
                norm[t * self.hop + i] += w * w;
            }
        }
        let y: Vec<f32> = x.iter().zip(&norm).map(|(a, n)| (a / if *n > 1e-10 { *n } else { 1.0 }) as f32).collect();
        if boundary {
            y[nfft / 2..total - nfft / 2].to_vec()
        } else {
            y
        }
    }
}

// ---------------------------------------------------------------- filters
/// One second-order section: b0 b1 b2 / 1 a1 a2.
#[derive(Clone, Copy, Debug)]
pub struct Sos {
    pub b: [f64; 3],
    pub a: [f64; 3],
}

/// Butterworth low-pass of even `order`, cut-off `fc` Hz at sample rate `fs` (like `butter(.., output="sos")`).
pub fn butter_lowpass(order: usize, fc: f64, fs: f64) -> Vec<Sos> {
    assert!(order % 2 == 0, "even orders only");
    let warped = 2.0 * fs * (PI * fc / fs).tan();
    let fs2 = 2.0 * fs;
    let mut out = vec![];
    for k in 0..order / 2 {
        // analog pole in the left half plane, upper half of the conjugate pair
        let ang = PI * (2 * k + 1 + order) as f64 / (2 * order) as f64;
        let (pr, pi) = (warped * ang.cos(), warped * ang.sin());
        // bilinear transform z = (fs2 + p) / (fs2 - p)
        let (nr, ni) = (fs2 + pr, pi);
        let (dr, di) = (fs2 - pr, -pi);
        let den = dr * dr + di * di;
        let (zr, zi) = ((nr * dr + ni * di) / den, (ni * dr - nr * di) / den);
        let a1 = -2.0 * zr;
        let a2 = zr * zr + zi * zi;
        let g = (1.0 + a1 + a2) / 4.0; // unity gain at DC
        out.push(Sos { b: [g, 2.0 * g, g], a: [1.0, a1, a2] });
    }
    out
}

/// Direct form II transposed, cascaded. `zi` has two states per section.
fn sosfilt(sos: &[Sos], x: &[f64], zi: &mut [[f64; 2]]) -> Vec<f64> {
    let mut y = x.to_vec();
    for (s, st) in sos.iter().zip(zi.iter_mut()) {
        for v in y.iter_mut() {
            let xin = *v;
            let out = s.b[0] * xin + st[0];
            st[0] = s.b[1] * xin - s.a[1] * out + st[1];
            st[1] = s.b[2] * xin - s.a[2] * out;
            *v = out;
        }
    }
    y
}

/// Steady-state start values for a unit step (`sosfilt_zi`).
fn sosfilt_zi(sos: &[Sos]) -> Vec<[f64; 2]> {
    let mut zi = vec![];
    let mut scale = 1.0;
    for s in sos {
        // solve (I - A) z = B for the 2nd-order section
        let (a1, a2) = (s.a[1], s.a[2]);
        let (b1, b2) = (s.b[1] - a1 * s.b[0], s.b[2] - a2 * s.b[0]);
        let (m00, m01, m10, m11) = (1.0 + a1, -1.0, a2, 1.0);
        let det = m00 * m11 - m01 * m10;
        let z = [(b1 * m11 - m01 * b2) / det, (m00 * b2 - m10 * b1) / det];
        zi.push([scale * z[0], scale * z[1]]);
        scale *= s.b.iter().sum::<f64>() / s.a.iter().sum::<f64>();
    }
    zi
}

/// Zero-phase forward-backward filtering with odd edge extension (`scipy.signal.sosfiltfilt`).
pub fn sosfiltfilt(sos: &[Sos], x: &[f64]) -> Vec<f64> {
    let padlen = (3 * (2 * sos.len() + 1)).min(x.len().saturating_sub(1));
    let n = x.len();
    let mut ext = Vec::with_capacity(n + 2 * padlen);
    for i in (1..=padlen).rev() {
        ext.push(2.0 * x[0] - x[i]);
    }
    ext.extend_from_slice(x);
    for i in 1..=padlen {
        ext.push(2.0 * x[n - 1] - x[n - 1 - i]);
    }
    let zi = sosfilt_zi(sos);
    let mut z: Vec<[f64; 2]> = zi.iter().map(|v| [v[0] * ext[0], v[1] * ext[0]]).collect();
    let mut y = sosfilt(sos, &ext, &mut z);
    let y0 = *y.last().unwrap();
    y.reverse();
    let mut z: Vec<[f64; 2]> = zi.iter().map(|v| [v[0] * y0, v[1] * y0]).collect();
    let mut y = sosfilt(sos, &y, &mut z);
    y.reverse();
    y[padlen..padlen + n].to_vec()
}

/// First-order FIR `y[n] = x[n] - c x[n-1]` (pre-emphasis).
pub fn preemphasis(x: &[f64], c: f64) -> Vec<f64> {
    let mut y = Vec::with_capacity(x.len());
    let mut prev = 0.0;
    for &v in x {
        y.push(v - c * prev);
        prev = v;
    }
    y
}

/// `fftconvolve(a, b[::-1], mode="valid")`: for k in 0..=len(a)-len(b),  sum_i a[k+i] * b[i].
pub fn correlate_valid(a: &[f64], b: &[f64]) -> Vec<f64> {
    let n = (a.len() + b.len()).next_power_of_two();
    let mut planner = RealFftPlanner::<f64>::new();
    let (fwd, inv) = (planner.plan_fft_forward(n), planner.plan_fft_inverse(n));
    let mut pa = vec![0f64; n];
    pa[..a.len()].copy_from_slice(a);
    let mut pb = vec![0f64; n];
    for (i, v) in b.iter().rev().enumerate() {
        pb[i] = *v;
    }
    let (mut fa, mut fb) = (fwd.make_output_vec(), fwd.make_output_vec());
    fwd.process(&mut pa, &mut fa).unwrap();
    fwd.process(&mut pb, &mut fb).unwrap();
    for (x, y) in fa.iter_mut().zip(&fb) {
        *x *= *y;
    }
    fa[0].im = 0.0;
    let last = fa.len() - 1;
    fa[last].im = 0.0;
    let mut full = vec![0f64; n];
    inv.process(&mut fa, &mut full).unwrap();
    // convolution index (k + len(b) - 1) holds correlation at k
    (0..=a.len() - b.len()).map(|k| full[k + b.len() - 1] / n as f64).collect()
}
