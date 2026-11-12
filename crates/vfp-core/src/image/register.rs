//! Align a leaked image to the original. Handles scale, crop, mild rotation and perspective.
//!
//! Same steps as `src/vfp/image/register.py`, in pure Rust:
//! 1. identity check, 2. feature match + RANSAC homography, 3. ECC refinement (sub-pixel),
//! 4. keep the best of the candidates by pixel error.

use super::color::to_ycrcb;
use super::features::{detect, match_ratio};
use super::warp::*;
use super::Bgr;
use crate::rng::Pcg64;
use nalgebra::{DMatrix, DVector, SMatrix, SVector};

#[derive(Clone, Debug)]
pub struct Registration {
    /// Maps original coordinates to leaked coordinates. `None` = already aligned.
    pub m: Option<Mat3>,
    pub method: &'static str,
    pub err: f64,
    pub inliers: usize,
}

pub fn gray_plane(img: &Bgr) -> Plane {
    let (y, _, _) = to_ycrcb(img);
    Plane { h: img.h, w: img.w, d: y.iter().map(|&v| v as f32).collect() }
}

// ---------------------------------------------------------------- homography
type Pt = (f64, f64);

fn normalise(p: &[Pt]) -> (Vec<Pt>, Mat3) {
    let n = p.len() as f64;
    let (cx, cy) = (p.iter().map(|q| q.0).sum::<f64>() / n, p.iter().map(|q| q.1).sum::<f64>() / n);
    let d = p.iter().map(|q| ((q.0 - cx).powi(2) + (q.1 - cy).powi(2)).sqrt()).sum::<f64>() / n;
    let s = if d > 1e-12 { 2f64.sqrt() / d } else { 1.0 };
    let t = [[s, 0.0, -s * cx], [0.0, s, -s * cy], [0.0, 0.0, 1.0]];
    (p.iter().map(|q| ((q.0 - cx) * s, (q.1 - cy) * s)).collect(), t)
}

/// Direct linear transform with Hartley normalisation. `src` -> `dst`.
fn homography_dlt(src: &[Pt], dst: &[Pt]) -> Option<Mat3> {
    if src.len() < 4 {
        return None;
    }
    let ((s, ts), (d, td)) = (normalise(src), normalise(dst));
    let mut ata = SMatrix::<f64, 9, 9>::zeros();
    for (a, b) in s.iter().zip(&d) {
        let r1 = SVector::<f64, 9>::from_row_slice(&[-a.0, -a.1, -1.0, 0.0, 0.0, 0.0, b.0 * a.0, b.0 * a.1, b.0]);
        let r2 = SVector::<f64, 9>::from_row_slice(&[0.0, 0.0, 0.0, -a.0, -a.1, -1.0, b.1 * a.0, b.1 * a.1, b.1]);
        ata += r1 * r1.transpose() + r2 * r2.transpose();
    }
    let eig = ata.symmetric_eigen();
    let k = (0..9).min_by(|&i, &j| eig.eigenvalues[i].partial_cmp(&eig.eigenvalues[j]).unwrap())?;
    let h = eig.eigenvectors.column(k);
    let hn = [[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], h[8]]];
    let full = mul(&inv(&td)?, &mul(&hn, &ts));
    let k = full[2][2];
    if k.abs() < 1e-12 {
        return None;
    }
    let mut o = full;
    for r in o.iter_mut() {
        for v in r.iter_mut() {
            *v /= k;
        }
    }
    Some(o)
}

fn transfer_err(h: &Mat3, a: Pt, b: Pt) -> f64 {
    match project(h, a.0, a.1) {
        Some((x, y)) => ((x - b.0).powi(2) + (y - b.1).powi(2)).sqrt(),
        None => f64::INFINITY,
    }
}

fn non_collinear(p: &[Pt; 4]) -> bool {
    (0..4).all(|skip| {
        let q: Vec<_> = (0..4).filter(|&i| i != skip).map(|i| p[i]).collect();
        ((q[1].0 - q[0].0) * (q[2].1 - q[0].1) - (q[1].1 - q[0].1) * (q[2].0 - q[0].0)).abs() > 1.0
    })
}

/// RANSAC homography. Returns (H, inlier count). `thresh` in pixels.
fn ransac_homography(src: &[Pt], dst: &[Pt], thresh: f64) -> Option<(Mat3, usize)> {
    let n = src.len();
    let mut rng = Pcg64::from_seed(&[20_261_112]);
    let (mut best_h, mut best_in) = (None::<Mat3>, 0usize);
    let (mut iters, mut i) = (2000usize, 0usize);
    while i < iters {
        i += 1;
        let mut idx = [0usize; 4];
        for k in 0..4 {
            loop {
                let c = rng.interval_pub(n as u64 - 1) as usize;
                if !idx[..k].contains(&c) {
                    idx[k] = c;
                    break;
                }
            }
        }
        let s = [src[idx[0]], src[idx[1]], src[idx[2]], src[idx[3]]];
        let d = [dst[idx[0]], dst[idx[1]], dst[idx[2]], dst[idx[3]]];
        if !non_collinear(&s) || !non_collinear(&d) {
            continue;
        }
        let Some(h) = homography_dlt(&s, &d) else { continue };
        let cnt = (0..n).filter(|&j| transfer_err(&h, src[j], dst[j]) < thresh).count();
        if cnt > best_in {
            best_in = cnt;
            best_h = Some(h);
            let w = cnt as f64 / n as f64;
            let denom = (1.0 - w.powi(4)).max(1e-12).ln();
            if denom < 0.0 {
                iters = iters.min(((1.0f64 - 0.995).ln() / denom).ceil().max(1.0) as usize);
            }
        }
    }
    let mut h = best_h?;
    // refit on all inliers, a couple of times
    for _ in 0..2 {
        let (s, d): (Vec<Pt>, Vec<Pt>) = (0..n).filter(|&j| transfer_err(&h, src[j], dst[j]) < thresh).map(|j| (src[j], dst[j])).unzip();
        if let Some(r) = homography_dlt(&s, &d) {
            h = r;
        }
    }
    let cnt = (0..n).filter(|&j| transfer_err(&h, src[j], dst[j]) < thresh).count();
    Some((h, cnt))
}

// ---------------------------------------------------------------- ECC
/// Sub-pixel refinement (Evangelidis & Psarakis ECC, homography model).
///
/// `w0` maps template coordinates to input coordinates (normalised so `w0[2][2] == 1`).
/// Returns the refined map. Only every `stride`-th template pixel is used, and the input is
/// sampled at the warped position, so cost does not grow with the image size.
fn ecc_homography(template: &Plane, input: &Plane, w0: &Mat3, iters: usize, eps: f64, stride: usize) -> Option<Mat3> {
    let tmpl = gaussian(template, 1.1, 2);
    let inp = gaussian(input, 1.1, 2);
    let mut wm = *w0;
    let margin = 5.0;
    let (ymax, xmax) = (input.h as f64 - 1.0 - margin, input.w as f64 - 1.0 - margin);
    let samples: Vec<(usize, usize)> = (0..template.h)
        .step_by(stride)
        .flat_map(|y| (0..template.w).step_by(stride).map(move |x| (x, y)))
        .collect();

    for _ in 0..iters {
        // per sample: template value, warped input value, Jacobian row
        let mut rows: Vec<(f64, f64, [f64; 8])> = Vec::with_capacity(samples.len());
        for &(x, y) in &samples {
            let (xf, yf) = (x as f64, y as f64);
            let d = wm[2][0] * xf + wm[2][1] * yf + 1.0;
            if d.abs() < 1e-9 {
                continue;
            }
            let px = (wm[0][0] * xf + wm[0][1] * yf + wm[0][2]) / d;
            let py = (wm[1][0] * xf + wm[1][1] * yf + wm[1][2]) / d;
            if px < margin || py < margin || px > xmax || py > ymax {
                continue;
            }
            let iv = bilinear(&inp, px, py) as f64;
            let gx = 0.5 * (bilinear(&inp, px + 1.0, py) - bilinear(&inp, px - 1.0, py)) as f64;
            let gy = 0.5 * (bilinear(&inp, px, py + 1.0) - bilinear(&inp, px, py - 1.0)) as f64;
            let (gxv, gyv) = (gx / d, gy / d);
            let g = -(gxv * px + gyv * py);
            rows.push((
                tmpl.d[y * template.w + x] as f64,
                iv,
                [gxv * xf, gxv * yf, gxv, gyv * xf, gyv * yf, gyv, g * xf, g * yf],
            ));
        }
        if rows.len() < 200 {
            return None;
        }
        let n = rows.len() as f64;
        let mt = rows.iter().map(|r| r.0).sum::<f64>() / n;
        let mi = rows.iter().map(|r| r.1).sum::<f64>() / n;

        let (mut hess, mut ip, mut tp) = (SMatrix::<f64, 8, 8>::zeros(), SVector::<f64, 8>::zeros(), SVector::<f64, 8>::zeros());
        let (mut corr, mut inorm) = (0f64, 0f64);
        for (t, i, j) in &rows {
            let (t, im) = (t - mt, i - mi);
            let jv = SVector::<f64, 8>::from_row_slice(j);
            hess += jv * jv.transpose();
            ip += jv * im;
            tp += jv * t;
            corr += t * im;
            inorm += im * im;
        }
        let hinv = hess.try_inverse()?;
        let iph = hinv * ip;
        let lambda_n = inorm - ip.dot(&iph);
        let lambda_d = corr - tp.dot(&iph);
        if lambda_d <= 0.0 || !lambda_d.is_finite() {
            return None;
        }
        let lambda = lambda_n / lambda_d;
        let mut ep = SVector::<f64, 8>::zeros();
        for (t, i, j) in &rows {
            ep += SVector::<f64, 8>::from_row_slice(j) * (lambda * (t - mt) - (i - mi));
        }
        let delta = hinv * ep;
        if !delta.iter().all(|v| v.is_finite()) {
            return None;
        }
        wm[0][0] += delta[0];
        wm[0][1] += delta[1];
        wm[0][2] += delta[2];
        wm[1][0] += delta[3];
        wm[1][1] += delta[4];
        wm[1][2] += delta[5];
        wm[2][0] += delta[6];
        wm[2][1] += delta[7];
        if delta.norm() < eps {
            break;
        }
    }
    Some(wm)
}

fn normalised(m: &Mat3) -> Mat3 {
    let k = m[2][2];
    let mut o = *m;
    for r in o.iter_mut() {
        for v in r.iter_mut() {
            *v /= k;
        }
    }
    o
}

// ---------------------------------------------------------------- top level
/// Largest image (pixels) the feature and first ECC stage work on.
const COARSE_PX: usize = 2_000_000;
/// ECC iterations at full size after a reduced-size pass.
const FINE_ITERS: usize = 8;
/// Pixels used for error checks and the full-size ECC pass.
const CHECK_PX: usize = 1_500_000;

fn stride_for(px: usize) -> usize {
    ((px as f64 / CHECK_PX as f64).sqrt().ceil() as usize).max(1)
}

/// Mean absolute gray error of `leaked` warped into the original grid, on every `stride`-th pixel.
/// 1e9 if less than 20% of the grid lands inside the leak.
fn warp_err(lg: &Plane, og: &Plane, m: &Mat3, stride: usize) -> f64 {
    let (mut sum, mut cnt, mut total) = (0f64, 0usize, 0usize);
    for y in (0..og.h).step_by(stride) {
        for x in (0..og.w).step_by(stride) {
            total += 1;
            let Some((sx, sy)) = project(m, x as f64, y as f64) else { continue };
            let (ix, iy) = (sx.round(), sy.round());
            if ix < 0.0 || iy < 0.0 || ix >= lg.w as f64 || iy >= lg.h as f64 {
                continue;
            }
            let v = bilinear(lg, sx, sy).round().clamp(0.0, 255.0);
            sum += (v - og.d[y * og.w + x]).abs() as f64;
            cnt += 1;
        }
    }
    if (cnt as f64) < 0.2 * total as f64 {
        return 1e9;
    }
    sum / cnt as f64
}

pub fn estimate_warp(leaked: &Bgr, original: &Bgr) -> Registration {
    let t0 = std::time::Instant::now();
    let trace = std::env::var_os("VFP_TRACE").is_some();
    let lap = |what: &str| {
        if trace {
            eprintln!("    [register] {what}: {:.2}s", t0.elapsed().as_secs_f64())
        }
    };
    let (lg, og) = (gray_plane(leaked), gray_plane(original));
    let (h, w) = (og.h, og.w);
    let id_err = (lg.h == og.h && lg.w == og.w)
        .then(|| lg.d.iter().zip(&og.d).map(|(a, b)| (a - b).abs() as f64).sum::<f64>() / (h * w) as f64);
    if let Some(e) = id_err {
        if e < 2.0 {
            return Registration { m: None, method: "identity", err: e, inliers: 0 };
        }
    }

    // The coarse stage runs on reduced copies (<= COARSE_PX). A 12 MP photo would be too slow otherwise.
    let ((lgs, al), (ogs, ao)) = (downscale(&lg, COARSE_PX), downscale(&og, COARSE_PX));
    let reduced = lgs.d.len() != lg.d.len() || ogs.d.len() != og.d.len();

    // coarse: feature matches + RANSAC (leaked -> original)
    let (fl, fo) = (detect(&lgs), detect(&ogs));
    lap("features");
    let mut hm = None;
    if fl.pts.len() > 12 && fo.pts.len() > 12 {
        let good = match_ratio(&fl, &fo, 0.75);
        if good.len() >= 12 {
            let src: Vec<Pt> = good.iter().map(|&(i, _)| (fl.pts[i].0 as f64, fl.pts[i].1 as f64)).collect();
            let dst: Vec<Pt> = good.iter().map(|&(_, j)| (fo.pts[j].0 as f64, fo.pts[j].1 as f64)).collect();
            hm = ransac_homography(&src, &dst, 3.0);
        }
    }
    let Some((hmat, inl)) = hm.filter(|&(_, n)| n >= 10) else {
        if let Some(e) = id_err {
            return Registration { m: None, method: "identity", err: e, inliers: 0 };
        }
        let (sx, sy) = (lg.w as f64 / w as f64, lg.h as f64 / h as f64);
        let m = [[sx, 0.0, 0.5 * sx - 0.5], [0.0, sy, 0.5 * sy - 0.5], [0.0, 0.0, 1.0]];
        let err = warp_err(&lg, &og, &m, stride_for(h * w));
        return Registration { m: Some(m), method: "resize-fallback", err, inliers: 0 };
    };
    lap("match+ransac");
    let mut m = normalised(&inv(&hmat).unwrap_or(IDENTITY));

    // ECC refinement on the reduced pair, kept only if it lowers the error
    let s_small = stride_for(ogs.h * ogs.w);
    if let Some(cand) = ecc_homography(&ogs, &lgs, &m, 60, 1e-6, 1) {
        if warp_err(&lgs, &ogs, &cand, s_small) < warp_err(&lgs, &ogs, &m, s_small) {
            m = cand;
        }
    }
    lap("ecc (coarse)");

    let stride = stride_for(h * w);
    if reduced {
        // back to full-size coordinates, then a short ECC pass at full size for sub-pixel accuracy
        if let Some(ail) = inv(&al) {
            m = normalised(&mul(&ail, &mul(&m, &ao)));
        }
        if let Some(cand) = ecc_homography(&og, &lg, &m, FINE_ITERS, 1e-6, stride) {
            if warp_err(&lg, &og, &cand, stride) < warp_err(&lg, &og, &m, stride) {
                m = cand;
            }
        }
        lap("ecc (full size)");
    }
    let err = warp_err(&lg, &og, &m, stride);
    if let Some(e) = id_err {
        if err >= 0.8 * e {
            return Registration { m: None, method: "identity", err: e, inliers: inl };
        }
    }
    Registration { m: Some(m), method: "orb+ecc", err, inliers: inl }
}

/// Warp the leaked image into the original grid. Returns (aligned, valid mask).
pub fn apply_warp(leaked: &Bgr, m: Option<&Mat3>, h: usize, w: usize) -> (Bgr, Vec<bool>) {
    let Some(m) = m else {
        return (leaked.clone(), vec![true; h * w]);
    };
    let mut data = vec![0u8; h * w * 3];
    for c in 0..3 {
        let plane = Plane { h: leaked.h, w: leaked.w, d: (0..leaked.h * leaked.w).map(|i| leaked.data[3 * i + c] as f32).collect() };
        let warped = warp_plane(&plane, m, h, w, true);
        for i in 0..h * w {
            data[3 * i + c] = warped.d[i].round().clamp(0.0, 255.0) as u8;
        }
    }
    let inside = warp_inside(leaked.h, leaked.w, None, m, h, w);
    (Bgr { h, w, data }, erode(&inside, h, w, 5))
}

/// Returns (aligned image, valid mask, registration info).
pub fn register(leaked: &Bgr, original: &Bgr) -> (Bgr, Vec<bool>, Registration) {
    let reg = estimate_warp(leaked, original);
    let (aligned, valid) = apply_warp(leaked, reg.m.as_ref(), original.h, original.w);
    (aligned, valid, reg)
}

// keep the nalgebra dynamic types referenced so a future solver swap is easy
#[allow(dead_code)]
type _Dyn = (DMatrix<f64>, DVector<f64>);
