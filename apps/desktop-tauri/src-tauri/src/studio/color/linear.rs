//! sRGB transfer-curve (TRC) helpers for linear-light pixel maths.
//!
//! The working space stays gamma-encoded (see `colour-pipeline.md`, open
//! decision 2): only operations whose maths assume light-linear values —
//! today, the enhance resample — decode to linear here, work in `f32`, and
//! re-encode. Averaging gamma-encoded values under-weights bright pixels
//! (a black/white edge resamples to sRGB 128 instead of the photometrically
//! correct 188), which shows up as dark fringing on high-contrast edges.
//!
//! The Python bridge mirrors these exact curves in
//! `python/bridge/linear_light.py`; the goldens in both test suites pin the
//! two engines to the same values.

use std::sync::OnceLock;

/// Decode one 8-bit sRGB sample to linear light in `0.0..=1.0` (IEC 61966-2-1).
pub(crate) fn srgb_u8_to_linear(v: u8) -> f32 {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    let lut = LUT.get_or_init(|| {
        let mut lut = [0.0f32; 256];
        for (i, slot) in lut.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *slot = if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
        }
        lut
    });
    lut[usize::from(v)]
}

/// Encode linear light back to an 8-bit sRGB sample (clamping to `0.0..=1.0`).
pub(crate) fn linear_to_srgb_u8(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.003_130_8 {
        12.92 * l
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trips_all_256_codes() {
        for v in 0..=255u8 {
            assert_eq!(linear_to_srgb_u8(srgb_u8_to_linear(v)), v);
        }
    }

    #[test]
    fn linear_midpoint_encodes_to_188() {
        // The photometric midpoint of black and white: the golden the Python
        // mirror (`linear_light.py`) pins too.
        assert_eq!(linear_to_srgb_u8(0.5), 188);
        assert_eq!(srgb_u8_to_linear(0), 0.0);
        assert!((srgb_u8_to_linear(255) - 1.0).abs() < 1e-6);
    }
}
