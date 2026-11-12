//! Image watermark: embed and detect on the luma plane with 8x8 block DCT.

pub mod color;
pub mod common;
pub mod detect;
pub mod embed;

/// A BGR image, 8 bits per channel, interleaved (same layout as OpenCV).
#[derive(Clone, Debug, PartialEq)]
pub struct Bgr {
    pub h: usize,
    pub w: usize,
    pub data: Vec<u8>,
}

impl Bgr {
    pub fn new(h: usize, w: usize, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), h * w * 3, "need h*w*3 bytes");
        Bgr { h, w, data }
    }
}
