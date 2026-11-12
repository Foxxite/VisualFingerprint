//! BGR <-> YCrCb in OpenCV's 8-bit fixed-point form, so Rust and Python agree to the bit.

use super::Bgr;

const SHIFT: i32 = 14;
const HALF: i32 = 1 << (SHIFT - 1);
const R2Y: i32 = 4899;
const G2Y: i32 = 9617;
const B2Y: i32 = 1868;
const CR_K: i32 = 11682;
const CB_K: i32 = 9241;
const DELTA: i32 = 128 << SHIFT;

fn descale(x: i32) -> i32 {
    (x + HALF) >> SHIFT
}

/// Returns (Y, Cr, Cb) planes, each h*w.
pub fn to_ycrcb(img: &Bgr) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let n = img.h * img.w;
    let (mut y, mut cr, mut cb) = (vec![0u8; n], vec![0u8; n], vec![0u8; n]);
    for i in 0..n {
        let (b, g, r) = (img.data[3 * i] as i32, img.data[3 * i + 1] as i32, img.data[3 * i + 2] as i32);
        let yy = descale(r * R2Y + g * G2Y + b * B2Y);
        y[i] = yy as u8;
        cr[i] = descale((r - yy) * CR_K + DELTA).clamp(0, 255) as u8;
        cb[i] = descale((b - yy) * CB_K + DELTA).clamp(0, 255) as u8;
    }
    (y, cr, cb)
}

pub fn from_ycrcb(h: usize, w: usize, y: &[u8], cr: &[u8], cb: &[u8]) -> Bgr {
    let mut data = vec![0u8; h * w * 3];
    for i in 0..h * w {
        let (yy, c_r, c_b) = (y[i] as i32, cr[i] as i32 - 128, cb[i] as i32 - 128);
        let b = yy + descale(c_b * 29049);
        let g = yy + descale(c_b * -5636 + c_r * -11698);
        let r = yy + descale(c_r * 22987);
        data[3 * i] = b.clamp(0, 255) as u8;
        data[3 * i + 1] = g.clamp(0, 255) as u8;
        data[3 * i + 2] = r.clamp(0, 255) as u8;
    }
    Bgr { h, w, data }
}
