//! Video watermark: per-frame DCT marks that change every segment, time-offset search, informed detection.

pub mod common;
pub mod detect;
pub mod embed;
pub mod ffio;
pub mod sync;
