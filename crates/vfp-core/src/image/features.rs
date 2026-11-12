//! ORB-style features in pure Rust: FAST corners on an image pyramid,
//! intensity-centroid orientation, rotated BRIEF descriptors (256 bits).
//!
//! The sampling pattern is generated from a fixed seed, so it is the same on every machine.

use super::warp::{gaussian, Plane};

pub type Desc = [u64; 4];

pub struct Features {
    /// Keypoint positions in level-0 (full image) pixels.
    pub pts: Vec<(f32, f32)>,
    pub desc: Vec<Desc>,
}

const SCALE: f64 = 1.2;
const MAX_LEVELS: usize = 8;
const FAST_T: f32 = 20.0;
const PER_LEVEL: usize = 900;
const CELL: usize = 20;
const PATCH_R: i32 = 15;
const BORDER: usize = 23;

const CIRCLE: [(i32, i32); 16] = [
    (0, -3), (1, -3), (2, -2), (3, -1), (3, 0), (3, 1), (2, 2), (1, 3),
    (0, 3), (-1, 3), (-2, 2), (-3, 1), (-3, 0), (-3, -1), (-2, -2), (-1, -3),
];

/// 256 point pairs inside the 31x31 patch, from a fixed xorshift generator.
fn pattern() -> Vec<[i32; 4]> {
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut gauss = || {
        let (u, v) = (next().max(1e-12), next());
        let g = (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos();
        ((g * 6.0).round() as i32).clamp(-PATCH_R, PATCH_R)
    };
    (0..256).map(|_| [gauss(), gauss(), gauss(), gauss()]).collect()
}

/// Resample to (nh, nw) with pixel-centre alignment, after a light blur.
fn resize(p: &Plane, nh: usize, nw: usize) -> Plane {
    let b = gaussian(p, 0.7, 2);
    let (sx, sy) = (p.w as f64 / nw as f64, p.h as f64 / nh as f64);
    Plane::from_fn(nh, nw, |x, y| {
        super::warp::bilinear(&b, (x as f64 + 0.5) * sx - 0.5, (y as f64 + 0.5) * sy - 0.5)
    })
}

/// FAST-9 corners with a score (sum of contrast over the arc). Non-maximum suppressed.
fn fast(p: &Plane) -> Vec<(usize, usize, f32)> {
    let (h, w) = (p.h, p.w);
    let mut score = vec![0f32; h * w];
    for y in BORDER..h.saturating_sub(BORDER) {
        for x in BORDER..w.saturating_sub(BORDER) {
            let c = p.d[y * w + x];
            let mut diffs = [0f32; 16];
            for (i, &(dx, dy)) in CIRCLE.iter().enumerate() {
                diffs[i] = p.d[(y as i32 + dy) as usize * w + (x as i32 + dx) as usize] - c;
            }
            let mut best = 0f32;
            for sign in [1.0f32, -1.0] {
                let (mut run, mut maxrun) = (0, 0);
                for i in 0..32 {
                    if diffs[i % 16] * sign > FAST_T {
                        run += 1;
                        maxrun = maxrun.max(run);
                    } else {
                        run = 0;
                    }
                }
                if maxrun >= 9 {
                    let s: f32 = diffs.iter().map(|d| (d * sign - FAST_T).max(0.0)).sum();
                    best = best.max(s);
                }
            }
            score[y * w + x] = best;
        }
    }
    let mut out = Vec::new();
    for y in BORDER..h.saturating_sub(BORDER) {
        for x in BORDER..w.saturating_sub(BORDER) {
            let s = score[y * w + x];
            if s > 0.0
                && (-1i32..=1).all(|dy| (-1i32..=1).all(|dx| score[(y as i32 + dy) as usize * w + (x as i32 + dx) as usize] <= s))
            {
                out.push((x, y, s));
            }
        }
    }
    out
}

/// Keep the best corner per grid cell, then the best `limit` overall.
fn spread(mut pts: Vec<(usize, usize, f32)>, w: usize, h: usize, limit: usize) -> Vec<(usize, usize, f32)> {
    let cols = w.div_ceil(CELL);
    let mut best: Vec<Option<(usize, usize, f32)>> = vec![None; cols * h.div_ceil(CELL)];
    for p in pts.drain(..) {
        let slot = &mut best[(p.1 / CELL) * cols + p.0 / CELL];
        if slot.is_none_or(|b| p.2 > b.2) {
            *slot = Some(p);
        }
    }
    let mut v: Vec<_> = best.into_iter().flatten().collect();
    v.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    v.truncate(limit);
    v
}

pub fn detect(gray: &Plane) -> Features {
    let pat = pattern();
    let mut feats = Features { pts: vec![], desc: vec![] };
    let mut level = gray.clone();
    for l in 0..MAX_LEVELS {
        if level.h.min(level.w) < 2 * BORDER + 20 {
            break;
        }
        let s = SCALE.powi(l as i32);
        let blurred = gaussian(&level, 2.0, 5);
        for (x, y, _) in spread(fast(&level), level.w, level.h, PER_LEVEL) {
            // orientation: intensity centroid in a round patch
            let (mut m10, mut m01) = (0f64, 0f64);
            for dy in -PATCH_R..=PATCH_R {
                for dx in -PATCH_R..=PATCH_R {
                    if dx * dx + dy * dy <= PATCH_R * PATCH_R {
                        let v = level.d[(y as i32 + dy) as usize * level.w + (x as i32 + dx) as usize] as f64;
                        m10 += dx as f64 * v;
                        m01 += dy as f64 * v;
                    }
                }
            }
            let ang = m01.atan2(m10);
            let (sa, ca) = ang.sin_cos();
            let sample = |px: i32, py: i32| {
                let rx = (px as f64 * ca - py as f64 * sa).round() as i32;
                let ry = (px as f64 * sa + py as f64 * ca).round() as i32;
                blurred.d[(y as i32 + ry) as usize * level.w + (x as i32 + rx) as usize]
            };
            let mut d: Desc = [0; 4];
            for (i, q) in pat.iter().enumerate() {
                if sample(q[0], q[1]) < sample(q[2], q[3]) {
                    d[i / 64] |= 1 << (i % 64);
                }
            }
            feats.pts.push((((x as f64 + 0.5) * s - 0.5) as f32, ((y as f64 + 0.5) * s - 0.5) as f32));
            feats.desc.push(d);
        }
        let (nh, nw) = ((level.h as f64 / SCALE) as usize, (level.w as f64 / SCALE) as usize);
        level = resize(&level, nh, nw);
    }
    feats
}

pub fn hamming(a: &Desc, b: &Desc) -> u32 {
    (0..4).map(|i| (a[i] ^ b[i]).count_ones()).sum()
}

/// Brute-force matching with Lowe's ratio test. Returns (index in `a`, index in `b`).
pub fn match_ratio(a: &Features, b: &Features, ratio: f32) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (i, da) in a.desc.iter().enumerate() {
        let (mut d1, mut d2, mut j1) = (u32::MAX, u32::MAX, 0usize);
        for (j, db) in b.desc.iter().enumerate() {
            let d = hamming(da, db);
            if d < d1 {
                d2 = d1;
                d1 = d;
                j1 = j;
            } else if d < d2 {
                d2 = d;
            }
        }
        if d2 != u32::MAX && (d1 as f32) < ratio * d2 as f32 {
            out.push((i, j1));
        }
    }
    out
}
