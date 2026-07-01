//! CMYK -> sRGB colour transform for the in-process enhance path (R3 CMYK c2).
//!
//! Given the raw 4-channel CMYK samples that `cmyk_decode` pulls straight from
//! the container, produce 8-bit sRGB the same way `image_enhance_cli.py`'s
//! `_cmyk_to_rgb` does:
//!
//! - **With an embedded ICC profile** — run a real profile-to-profile transform
//!   (`moxcms`, the CMYK profile's A2B LUT into sRGB, perceptual intent), mirroring
//!   the CLI's `ImageCms.profileToProfile(img, src, sRGB, outputMode="RGB")`.
//! - **Without a profile** (or if the transform fails for any reason) — fall back
//!   to PIL's naive CMYK->RGB, byte-for-byte: for each channel
//!   `out = (255 - K) - muldiv255(255 - K, ink)`, where `muldiv255(a, b)` is
//!   Pillow's rounding `((t >> 8) + t) >> 8` with `t = a*b + 128`. This matches
//!   `Image.convert("RGB")` exactly (see the tests).
//!
//! This module is **not yet wired into the enhance path** — it is exercised only
//! by its own tests. Wiring (with a Python fallback on any miss) is step c3, and
//! the byte-accurate ΔE regression against the Python output is c4.
//!
//! Like the Python path, the produced sRGB no longer carries the old CMYK
//! profile (`icc_preserved: false`). Adobe-APP14 inverted-ink JPEGs are handled
//! by the caller (c3), not here — this transform trusts its input samples to be
//! in the profile's device direction (0 = no ink).
#![allow(dead_code)]

use moxcms::{
    ColorProfile, DataColorSpace, Layout, RenderingIntent, TransformExecutor, TransformOptions,
};

use super::cmyk_decode::RawCmyk;

/// Convert raw CMYK samples to packed 8-bit sRGB (`width * height * 3` bytes, RGB).
///
/// Infallible: an embedded profile is used when present and usable, otherwise
/// (and on any transform error) the naive PIL formula is applied.
pub(crate) fn cmyk_to_rgb8(raw: &RawCmyk) -> Vec<u8> {
    if let Some(icc) = raw.icc.as_deref() {
        if let Some(rgb) = icc_cmyk_to_rgb(raw, icc) {
            return rgb;
        }
    }
    naive_cmyk_to_rgb(&raw.samples)
}

/// Apply the embedded CMYK ICC profile to reach sRGB. Returns `None` (so the
/// caller falls back to the naive formula) if the profile is not CMYK, cannot be
/// parsed, or the transform fails.
fn icc_cmyk_to_rgb(raw: &RawCmyk, icc: &[u8]) -> Option<Vec<u8>> {
    let src = ColorProfile::new_from_slice(icc).ok()?;
    if src.color_space != DataColorSpace::Cmyk {
        return None;
    }
    let dst = ColorProfile::new_srgb();
    let options = TransformOptions {
        // Match the CLI's `ImageCms.profileToProfile` default (INTENT_PERCEPTUAL).
        rendering_intent: RenderingIntent::Perceptual,
        ..TransformOptions::default()
    };
    // A CMYK 4-channel buffer uses the same packed layout as RGBA; the source
    // profile's colour space marks it as ink, so `Layout::Rgba` is correct here.
    let transform = src
        .create_transform_8bit(Layout::Rgba, &dst, Layout::Rgb, options)
        .ok()?;

    let pixels = raw.width as usize * raw.height as usize;
    if raw.samples.len() != pixels * 4 {
        return None;
    }
    let mut out = vec![0u8; pixels * 3];
    transform.transform(&raw.samples, &mut out).ok()?;
    Some(out)
}

/// Pillow's `MULDIV255`: rounded `a * b / 255`.
#[inline]
fn muldiv255(a: u32, b: u32) -> u32 {
    let t = a * b + 128;
    ((t >> 8) + t) >> 8
}

/// PIL's naive CMYK -> RGB (`Image.convert("RGB")`), byte-for-byte.
fn naive_cmyk_to_rgb(cmyk: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(cmyk.len() / 4 * 3);
    for px in cmyk.chunks_exact(4) {
        let nk = 255 - u32::from(px[3]);
        out.push((nk - muldiv255(nk, u32::from(px[0]))) as u8);
        out.push((nk - muldiv255(nk, u32::from(px[1]))) as u8);
        out.push((nk - muldiv255(nk, u32::from(px[2]))) as u8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWOP_ICC: &str = r"C:\Windows\System32\spool\drivers\color\RSWOP.icm";

    fn raw(width: u32, samples: Vec<u8>, icc: Option<Vec<u8>>) -> RawCmyk {
        RawCmyk {
            width,
            height: 1,
            samples,
            icc,
        }
    }

    #[test]
    fn naive_matches_pil_convert_rgb() {
        // (C, M, Y, K) -> (R, G, B) reference values taken from Pillow's
        // `Image.new("CMYK").convert("RGB")` (Pillow 12.3).
        let cases: &[([u8; 4], [u8; 3])] = &[
            ([0, 0, 0, 0], [255, 255, 255]),
            ([255, 0, 0, 0], [0, 255, 255]),
            ([0, 255, 0, 0], [255, 0, 255]),
            ([0, 0, 255, 0], [255, 255, 0]),
            ([0, 0, 0, 255], [0, 0, 0]),
            ([255, 255, 255, 255], [0, 0, 0]),
            ([128, 64, 32, 16], [119, 179, 209]),
            ([200, 100, 50, 25], [50, 140, 185]),
            ([255, 255, 255, 0], [0, 0, 0]),
            ([10, 20, 30, 40], [207, 198, 190]),
        ];
        for (cmyk, expected) in cases {
            let got = naive_cmyk_to_rgb(cmyk);
            assert_eq!(&got[..], &expected[..], "naive mismatch for {cmyk:?}");
        }
    }

    #[test]
    fn no_profile_uses_naive() {
        let samples = vec![128, 64, 32, 16];
        let out = cmyk_to_rgb8(&raw(1, samples.clone(), None));
        assert_eq!(out, naive_cmyk_to_rgb(&samples));
    }

    #[test]
    fn invalid_profile_falls_back_to_naive() {
        let samples = vec![200, 100, 50, 25];
        let out = cmyk_to_rgb8(&raw(1, samples.clone(), Some(b"not an icc profile".to_vec())));
        assert_eq!(out, naive_cmyk_to_rgb(&samples));
    }

    #[test]
    fn icc_transform_matches_littlecms_reference() {
        // Skip when the OS SWOP CMYK profile isn't installed (non-Windows CI).
        let icc = match std::fs::read(SWOP_ICC) {
            Ok(bytes) => bytes,
            Err(_) => return,
        };

        // (C, M, Y, K) patches and the sRGB `ImageCms.profileToProfile` produces
        // through the same RSWOP profile at perceptual intent (Pillow 12.3).
        let patches: [[u8; 4]; 7] = [
            [0, 0, 0, 0],
            [255, 0, 0, 0],
            [0, 255, 0, 0],
            [0, 0, 255, 0],
            [0, 0, 0, 255],
            [128, 64, 32, 16],
            [200, 100, 50, 25],
        ];
        let reference: [[u8; 3]; 7] = [
            [255, 255, 255],
            [0, 159, 215],
            [232, 39, 131],
            [255, 241, 20],
            [24, 24, 23],
            [135, 152, 171],
            [78, 115, 140],
        ];

        let samples: Vec<u8> = patches.iter().flatten().copied().collect();
        let out = cmyk_to_rgb8(&raw(patches.len() as u32, samples.clone(), Some(icc)));
        assert_eq!(out.len(), patches.len() * 3);

        // The profile must actually have been applied, not silently naive: SWOP
        // cyan is nothing like naive cyan (0, 255, 255).
        let naive = naive_cmyk_to_rgb(&samples);
        assert_ne!(out, naive, "ICC path unexpectedly fell back to naive");

        // Engine-independent structural checks.
        assert!(out[0] >= 250 && out[1] >= 250 && out[2] >= 250, "no-ink must stay white");
        let black = &out[4 * 3..5 * 3];
        assert!(black.iter().all(|&v| v <= 45), "full-K must stay near black");

        // Loose parity with the littleCMS reference; c4 tightens this to a ΔE
        // regression once the observed moxcms output is captured.
        const TOL: i32 = 30;
        for (i, expected) in reference.iter().enumerate() {
            for ch in 0..3 {
                let got = out[i * 3 + ch] as i32;
                let want = expected[ch] as i32;
                assert!(
                    (got - want).abs() <= TOL,
                    "patch {i} ch {ch}: moxcms {got} vs littleCMS {want} exceeds ±{TOL}"
                );
            }
        }
    }
}
