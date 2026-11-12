//! VisualFingerprint core. Rust port of the Python reference in `src/vfp/`.
//!
//! Every random stream here must match NumPy's `default_rng` bit for bit.
//! The golden tests in `tests/golden.rs` check this against `tests/golden/golden.json`.

pub mod image;
pub mod keys;
pub mod perceptual;
pub mod rng;
pub mod tardos;
