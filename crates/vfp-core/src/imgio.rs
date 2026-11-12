//! Read and write image files as BGR arrays.

use crate::error::{err, Result};
use crate::image::Bgr;
use image::{ImageDecoder, ImageReader};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// Decode any supported image. The EXIF orientation is applied (OpenCV does the same).
pub fn read_image(path: &Path) -> Result<Bgr> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut decoder = reader.into_decoder().map_err(|e| crate::error::Error(format!("cannot read image {}: {e}", path.display())))?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut dynimg = image::DynamicImage::from_decoder(decoder)?;
    dynimg.apply_orientation(orientation);
    let rgb = dynimg.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut data = Vec::with_capacity(w * h * 3);
    for px in rgb.pixels() {
        data.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    Ok(Bgr::new(h, w, data))
}

/// Write by file extension. JPEG uses quality 95. Other formats are lossless.
pub fn write_image(path: &Path, img: &Bgr) -> Result<()> {
    let mut rgb = image::RgbImage::new(img.w as u32, img.h as u32);
    for (i, px) in rgb.pixels_mut().enumerate() {
        *px = image::Rgb([img.data[3 * i + 2], img.data[3 * i + 1], img.data[3 * i]]);
    }
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => {
            let file = std::io::BufWriter::new(std::fs::File::create(path)?);
            image::codecs::jpeg::JpegEncoder::new_with_quality(file, 95).encode_image(&rgb)?;
        }
        "png" | "bmp" | "tif" | "tiff" => rgb.save(path)?,
        other => return err(format!("cannot write images as .{other}")),
    }
    Ok(())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
