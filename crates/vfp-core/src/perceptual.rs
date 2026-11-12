//! Perceptual masking: how much mark each 8x8 block can hide.

/// Strength multiplier for one block, from the ORIGINAL block DCT (row-major 8x8).
pub fn block_mask(coef: &[f32; 64]) -> f32 {
    let dc = coef[0];
    let total: f32 = coef.iter().map(|c| c * c).sum();
    let ac = (total - dc * dc).max(0.0);
    let std = ac.sqrt() / 8.0;
    let texture = (std / 8.0).sqrt();
    let luma = (dc / 8.0 / 60.0).clamp(0.6, 1.0);
    (texture * luma).clamp(0.5, 2.0)
}
