//! Small image-math toolkit: 3x3 matrices, float planes, warping, blur, erode.
//!
//! A warp matrix `M` maps DESTINATION coordinates to SOURCE coordinates
//! (the same meaning as OpenCV's `WARP_INVERSE_MAP`).

pub type Mat3 = [[f64; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

pub fn inv(m: &Mat3) -> Option<Mat3> {
    let c = [
        [m[1][1] * m[2][2] - m[1][2] * m[2][1], m[0][2] * m[2][1] - m[0][1] * m[2][2], m[0][1] * m[1][2] - m[0][2] * m[1][1]],
        [m[1][2] * m[2][0] - m[1][0] * m[2][2], m[0][0] * m[2][2] - m[0][2] * m[2][0], m[0][2] * m[1][0] - m[0][0] * m[1][2]],
        [m[1][0] * m[2][1] - m[1][1] * m[2][0], m[0][1] * m[2][0] - m[0][0] * m[2][1], m[0][0] * m[1][1] - m[0][1] * m[1][0]],
    ];
    let det = m[0][0] * c[0][0] + m[0][1] * c[1][0] + m[0][2] * c[2][0];
    if det.abs() < 1e-18 || !det.is_finite() {
        return None;
    }
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = c[i][j] / det;
        }
    }
    Some(o)
}

pub fn project(m: &Mat3, x: f64, y: f64) -> Option<(f64, f64)> {
    let d = m[2][0] * x + m[2][1] * y + m[2][2];
    if d.abs() < 1e-12 {
        return None;
    }
    Some(((m[0][0] * x + m[0][1] * y + m[0][2]) / d, (m[1][0] * x + m[1][1] * y + m[1][2]) / d))
}

#[derive(Clone, Debug)]
pub struct Plane {
    pub h: usize,
    pub w: usize,
    pub d: Vec<f32>,
}

impl Plane {
    pub fn new(h: usize, w: usize) -> Self {
        Plane { h, w, d: vec![0.0; h * w] }
    }
    pub fn from_fn(h: usize, w: usize, f: impl Fn(usize, usize) -> f32) -> Self {
        let mut d = Vec::with_capacity(h * w);
        for y in 0..h {
            for x in 0..w {
                d.push(f(x, y));
            }
        }
        Plane { h, w, d }
    }
    /// Pixel value, zero outside the plane.
    pub fn at(&self, x: isize, y: isize) -> f32 {
        if x < 0 || y < 0 || x >= self.w as isize || y >= self.h as isize {
            0.0
        } else {
            self.d[y as usize * self.w + x as usize]
        }
    }
    /// Pixel value with the edge pixel repeated outside.
    pub fn clamped(&self, x: isize, y: isize) -> f32 {
        self.d[y.clamp(0, self.h as isize - 1) as usize * self.w + x.clamp(0, self.w as isize - 1) as usize]
    }
}

pub fn bilinear(p: &Plane, x: f64, y: f64) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
    let (xi, yi) = (x0 as isize, y0 as isize);
    let top = p.at(xi, yi) * (1.0 - fx) + p.at(xi + 1, yi) * fx;
    let bot = p.at(xi, yi + 1) * (1.0 - fx) + p.at(xi + 1, yi + 1) * fx;
    top * (1.0 - fy) + bot * fy
}

fn cubic_weights(t: f64) -> [f32; 4] {
    const A: f64 = -0.75; // OpenCV's bicubic
    let k = |x: f64| {
        let x = x.abs();
        if x <= 1.0 {
            ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0
        } else if x < 2.0 {
            ((A * x - 5.0 * A) * x + 8.0 * A) * x - 4.0 * A
        } else {
            0.0
        }
    };
    [k(t + 1.0) as f32, k(t) as f32, k(1.0 - t) as f32, k(2.0 - t) as f32]
}

pub fn bicubic(p: &Plane, x: f64, y: f64) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (wx, wy) = (cubic_weights(x - x0), cubic_weights(y - y0));
    let (xi, yi) = (x0 as isize, y0 as isize);
    let mut s = 0.0;
    for (j, wyj) in wy.iter().enumerate() {
        let mut row = 0.0;
        for (i, wxi) in wx.iter().enumerate() {
            row += wxi * p.at(xi - 1 + i as isize, yi - 1 + j as isize);
        }
        s += wyj * row;
    }
    s
}

/// dst(x, y) = src(M (x, y)); pixels that map outside the source are 0.
pub fn warp_plane(src: &Plane, m: &Mat3, dh: usize, dw: usize, cubic: bool) -> Plane {
    let mut out = Plane::new(dh, dw);
    for y in 0..dh {
        for x in 0..dw {
            if let Some((sx, sy)) = project(m, x as f64, y as f64) {
                out.d[y * dw + x] = if cubic { bicubic(src, sx, sy) } else { bilinear(src, sx, sy) };
            }
        }
    }
    out
}

/// Which destination pixels land inside the source (nearest-neighbour rule).
/// `src_mask` limits the source area; `None` means the whole source.
pub fn warp_inside(sh: usize, sw: usize, src_mask: Option<&[bool]>, m: &Mat3, dh: usize, dw: usize) -> Vec<bool> {
    let mut out = vec![false; dh * dw];
    for y in 0..dh {
        for x in 0..dw {
            if let Some((sx, sy)) = project(m, x as f64, y as f64) {
                let (ix, iy) = (sx.round(), sy.round());
                if ix >= 0.0 && iy >= 0.0 && ix < sw as f64 && iy < sh as f64 {
                    let idx = iy as usize * sw + ix as usize;
                    out[y * dw + x] = src_mask.is_none_or(|m| m[idx]);
                }
            }
        }
    }
    out
}

/// Erode a bool mask with a k x k box. Outside the image counts as true (like OpenCV's default).
pub fn erode(mask: &[bool], h: usize, w: usize, k: usize) -> Vec<bool> {
    let r = (k / 2) as isize;
    let pass = |src: &[bool], horizontal: bool| -> Vec<bool> {
        let mut out = vec![true; h * w];
        for y in 0..h {
            for x in 0..w {
                let mut ok = true;
                for d in -r..=r {
                    let (xx, yy) = if horizontal { (x as isize + d, y as isize) } else { (x as isize, y as isize + d) };
                    if xx >= 0 && yy >= 0 && xx < w as isize && yy < h as isize && !src[yy as usize * w + xx as usize] {
                        ok = false;
                        break;
                    }
                }
                out[y * w + x] = ok;
            }
        }
        out
    };
    pass(&pass(mask, true), false)
}

/// Separable Gaussian blur with repeated edges.
pub fn gaussian(p: &Plane, sigma: f64, radius: usize) -> Plane {
    let k: Vec<f32> = {
        let raw: Vec<f64> = (-(radius as isize)..=radius as isize).map(|i| (-((i * i) as f64) / (2.0 * sigma * sigma)).exp()).collect();
        let s: f64 = raw.iter().sum();
        raw.iter().map(|v| (v / s) as f32).collect()
    };
    let r = radius as isize;
    let mut tmp = Plane::new(p.h, p.w);
    for y in 0..p.h {
        for x in 0..p.w {
            tmp.d[y * p.w + x] = (-r..=r).map(|d| k[(d + r) as usize] * p.clamped(x as isize + d, y as isize)).sum();
        }
    }
    let mut out = Plane::new(p.h, p.w);
    for y in 0..p.h {
        for x in 0..p.w {
            out.d[y * p.w + x] = (-r..=r).map(|d| k[(d + r) as usize] * tmp.clamped(x as isize, y as isize + d)).sum();
        }
    }
    out
}

/// Shrink a plane to at most `max_px` pixels (low-pass first). Returns the small plane and the
/// matrix that maps full-size coordinates to small coordinates (identity if no shrink).
pub fn downscale(p: &Plane, max_px: usize) -> (Plane, Mat3) {
    if p.h * p.w <= max_px {
        return (p.clone(), IDENTITY);
    }
    let f = ((p.h * p.w) as f64 / max_px as f64).sqrt();
    let (nh, nw) = (((p.h as f64 / f).round() as usize).max(1), ((p.w as f64 / f).round() as usize).max(1));
    let (fx, fy) = (p.w as f64 / nw as f64, p.h as f64 / nh as f64);
    let b = gaussian(p, 0.5 * fx.max(fy), (1.5 * fx.max(fy)).ceil() as usize + 1);
    let small = Plane::from_fn(nh, nw, |x, y| bilinear(&b, (x as f64 + 0.5) * fx - 0.5, (y as f64 + 0.5) * fy - 0.5));
    (small, [[1.0 / fx, 0.0, 0.5 / fx - 0.5], [0.0, 1.0 / fy, 0.5 / fy - 0.5], [0.0, 0.0, 1.0]])
}

/// Shrink with area averaging (like OpenCV `INTER_AREA`), 8-bit in and out.
pub fn resize_area_u8(src: &[u8], sh: usize, sw: usize, dh: usize, dw: usize) -> Vec<u8> {
    // weights of each destination pixel over the source pixels it covers
    let axis = |sn: usize, dn: usize| -> Vec<Vec<(usize, f32)>> {
        let scale = sn as f64 / dn as f64;
        (0..dn)
            .map(|i| {
                let (a, b) = (i as f64 * scale, (i + 1) as f64 * scale);
                let (k0, k1) = (a.floor() as usize, (b.ceil() as usize).min(sn));
                let mut w: Vec<(usize, f32)> =
                    (k0..k1).map(|k| (k, ((b.min((k + 1) as f64)) - a.max(k as f64)).max(0.0) as f32)).collect();
                let tot: f32 = w.iter().map(|x| x.1).sum();
                for x in w.iter_mut() {
                    x.1 /= tot;
                }
                w
            })
            .collect()
    };
    let (wx, wy) = (axis(sw, dw), axis(sh, dh));
    // horizontal pass
    let mut tmp = vec![0f32; sh * dw];
    for y in 0..sh {
        for (x, ws) in wx.iter().enumerate() {
            tmp[y * dw + x] = ws.iter().map(|&(k, w)| src[y * sw + k] as f32 * w).sum();
        }
    }
    let mut out = vec![0u8; dh * dw];
    for (y, ws) in wy.iter().enumerate() {
        for x in 0..dw {
            let v: f32 = ws.iter().map(|&(k, w)| tmp[k * dw + x] * w).sum();
            out[y * dw + x] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Bicubic resize of a float plane with repeated edges (like OpenCV `INTER_CUBIC`).
pub fn resize_cubic(src: &[f32], sh: usize, sw: usize, dh: usize, dw: usize) -> Vec<f32> {
    let (sx, sy) = (sw as f64 / dw as f64, sh as f64 / dh as f64);
    let mut out = vec![0f32; dh * dw];
    for y in 0..dh {
        let fy = (y as f64 + 0.5) * sy - 0.5;
        let (y0, wy) = (fy.floor(), cubic_weights(fy - fy.floor()));
        for x in 0..dw {
            let fx = (x as f64 + 0.5) * sx - 0.5;
            let (x0, wx) = (fx.floor(), cubic_weights(fx - fx.floor()));
            let mut s = 0f32;
            for (j, wyj) in wy.iter().enumerate() {
                let yy = (y0 as isize - 1 + j as isize).clamp(0, sh as isize - 1) as usize;
                let mut row = 0f32;
                for (i, wxi) in wx.iter().enumerate() {
                    let xx = (x0 as isize - 1 + i as isize).clamp(0, sw as isize - 1) as usize;
                    row += wxi * src[yy * sw + xx];
                }
                s += wyj * row;
            }
            out[y * dw + x] = s;
        }
    }
    out
}
