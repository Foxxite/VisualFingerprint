//! Alignment tests on a procedural "photo": crop, scale, rotation, then a full mark and trace.

use vfp_core::image::common::ImageParams;
use vfp_core::image::detect::detect_image;
use vfp_core::image::embed::embed_image;
use vfp_core::image::register::estimate_warp;
use vfp_core::image::warp::{inv, project, warp_plane, Mat3, Plane};
use vfp_core::image::Bgr;
use vfp_core::rng::Pcg64;

/// Multi-octave value noise, different per channel. Looks like a rough natural texture.
fn scene(h: usize, w: usize) -> Bgr {
    let mut data = vec![0u8; h * w * 3];
    for c in 0..3 {
        let mut acc = vec![0f64; h * w];
        for (oct, cells) in [6usize, 12, 24, 48, 96].into_iter().enumerate() {
            let mut rng = Pcg64::from_seed(&[1000 + c as u64, oct as u64]);
            let lat: Vec<f64> = (0..(cells + 2) * (cells + 2)).map(|_| rng.next_f64() - 0.5).collect();
            let amp = 140.0 / 2f64.powi(oct as i32);
            for y in 0..h {
                for x in 0..w {
                    let (fx, fy) = (x as f64 / w as f64 * cells as f64, y as f64 / h as f64 * cells as f64);
                    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                    let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
                    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
                    let g = |i: usize, j: usize| lat[j * (cells + 2) + i];
                    let v = (g(x0, y0) * (1.0 - sx) + g(x0 + 1, y0) * sx) * (1.0 - sy)
                        + (g(x0, y0 + 1) * (1.0 - sx) + g(x0 + 1, y0 + 1) * sx) * sy;
                    acc[y * w + x] += v * amp;
                }
            }
        }
        for i in 0..h * w {
            data[3 * i + c] = (128.0 + acc[i]).round().clamp(0.0, 255.0) as u8;
        }
    }
    // sharp structure and grain, so there are corners to find (a real photo has them)
    let mut rng = Pcg64::from_seed(&[77]);
    for _ in 0..(h * w / 700) {
        let (rw, rh) = (6 + (rng.next_f64() * 40.0) as usize, 6 + (rng.next_f64() * 40.0) as usize);
        let (x0, y0) = ((rng.next_f64() * w as f64) as usize, (rng.next_f64() * h as f64) as usize);
        let col = [rng.next_f64() * 255.0, rng.next_f64() * 255.0, rng.next_f64() * 255.0];
        for y in y0..(y0 + rh).min(h) {
            for x in x0..(x0 + rw).min(w) {
                for c in 0..3 {
                    let v = data[3 * (y * w + x) + c] as f64;
                    data[3 * (y * w + x) + c] = (0.35 * v + 0.65 * col[c]).round() as u8;
                }
            }
        }
    }
    for v in data.iter_mut() {
        *v = (*v as f64 + (rng.next_f64() - 0.5) * 10.0).round().clamp(0.0, 255.0) as u8;
    }
    Bgr::new(h, w, data)
}

/// Resample `img` through `g` (leaked coords -> original coords) into an lh x lw image.
fn attack(img: &Bgr, g: &Mat3, lh: usize, lw: usize) -> Bgr {
    let mut out = vec![0u8; lh * lw * 3];
    for c in 0..3 {
        let p = Plane { h: img.h, w: img.w, d: (0..img.h * img.w).map(|i| img.data[3 * i + c] as f32).collect() };
        let q = warp_plane(&p, g, lh, lw, false);
        for i in 0..lh * lw {
            out[3 * i + c] = q.d[i].round().clamp(0.0, 255.0) as u8;
        }
    }
    Bgr::new(lh, lw, out)
}

fn crop_scale(ox: f64, oy: f64, s: f64) -> Mat3 {
    [[1.0 / s, 0.0, ox], [0.0, 1.0 / s, oy], [0.0, 0.0, 1.0]]
}

fn rot_scale(w: usize, h: usize, deg: f64, s: f64) -> Mat3 {
    let (sn, cs) = deg.to_radians().sin_cos();
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    // leaked -> original: undo scale and rotation about the centre
    let (a, b) = (cs / s, sn / s);
    [[a, -b, cx - a * cx + b * cy], [b, a, cy - b * cx - a * cy], [0.0, 0.0, 1.0]]
}

/// Largest error (pixels) of the estimated original->leaked map on a grid of original points.
fn map_error(est: &Mat3, g: &Mat3, orig_w: usize, orig_h: usize, lw: usize, lh: usize) -> f64 {
    let truth = inv(g).unwrap();
    let mut worst = 0f64;
    for gy in 1..6 {
        for gx in 1..6 {
            let (x, y) = (gx as f64 * orig_w as f64 / 6.0, gy as f64 * orig_h as f64 / 6.0);
            let (tx, ty) = project(&truth, x, y).unwrap();
            if tx < 5.0 || ty < 5.0 || tx > lw as f64 - 5.0 || ty > lh as f64 - 5.0 {
                continue; // not visible in the leak
            }
            let (ex, ey) = project(est, x, y).unwrap();
            worst = worst.max(((ex - tx).powi(2) + (ey - ty).powi(2)).sqrt());
        }
    }
    worst
}

#[test]
fn aligned_leak_is_identity() {
    let o = scene(300, 400);
    let mut l = o.clone();
    for (i, v) in l.data.iter_mut().enumerate() {
        *v = v.saturating_add((i % 3) as u8); // tiny change, like re-compression
    }
    let r = estimate_warp(&l, &o);
    assert!(r.m.is_none(), "{r:?}");
}

#[test]
fn crop_and_downscale() {
    let o = scene(480, 640);
    let g = crop_scale(70.0, 50.0, 0.6);
    let (lw, lh) = (330usize, 250usize);
    let l = attack(&o, &g, lh, lw);
    let r = estimate_warp(&l, &o);
    println!("{r:?}");
    let m = r.m.expect("a warp must be found");
    let e = map_error(&m, &g, 640, 480, lw, lh);
    println!("max map error {e:.3} px");
    assert!(e < 1.5, "map error {e}");
}

#[test]
fn rotation_and_scale() {
    let o = scene(480, 640);
    let g = rot_scale(640, 480, 3.0, 0.9);
    let l = attack(&o, &g, 432, 576);
    let r = estimate_warp(&l, &o);
    println!("{r:?}");
    let m = r.m.expect("a warp must be found");
    // the leaked frame is the same scene shrunk by 0.9 about its own centre: compare in leaked space
    let e = map_error(&m, &g, 640, 480, 576, 432);
    println!("max map error {e:.3} px");
    assert!(e < 2.0, "map error {e}");
}

#[test]
fn mark_attack_trace() {
    let o = scene(480, 640);
    let p = ImageParams { nslots: 64, strength: 3.0 };
    let seed = 987_654_321u64;
    let sym: Vec<u8> = (0..64).map(|i| ((i * 7 + 3) % 5 < 2) as u8).collect();
    let marked = embed_image(&o, &sym, seed, &p).unwrap();

    for (name, g, lh, lw) in [
        ("untouched", crop_scale(0.0, 0.0, 1.0), 480, 640),
        ("crop+scale 0.7", crop_scale(40.0, 30.0, 0.7), 320, 410),
        ("rotate 2 deg + scale 0.9", rot_scale(640, 480, 2.0, 0.9), 432, 576),
    ] {
        let leak = attack(&marked, &g, lh, lw);
        let (det, reg) = detect_image(&leak, &o, seed, &p);
        let ok = det.bits.iter().zip(&sym).filter(|(a, b)| a == b).count();
        println!("{name}: {ok}/64 bits right, method {}, overlap {:.2}", reg.method, det.valid_fraction);
        assert!(ok >= 60, "{name}: only {ok}/64 bits right");
    }
}
