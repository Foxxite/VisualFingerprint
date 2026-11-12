//! VisualFingerprint core. Rust port of the Python reference in `src/vfp/`.
//!
//! Every random stream here must match NumPy's `default_rng` bit for bit.
//! The golden tests in `tests/golden.rs` check this against `tests/golden/golden.json`.

pub mod audio;
pub mod backend;
pub mod error;
pub mod image;
pub mod imgio;
pub mod keys;
pub mod perceptual;
pub mod pipeline;
pub mod rng;
pub mod strength;
pub mod tardos;
pub mod vault;
pub mod video;
pub mod workspace;

pub use error::{Error, Result};
