//! The canonical **working surface** for the Studio colour pipeline: a 16-bit
//! RGBA buffer plus the working colour space it lives in and an optional ICC
//! profile describing it. This is the internal "truth" every card will operate
//! on so the manual path keeps full grading fidelity; the sRGB down-convert is
//! deferred to the model-egress boundary. See
//! [`docs/design/colour-pipeline.md`](../../../../../docs/design/colour-pipeline.md).
//!
//! **Phase 1 (this file): scaffolding only.** The type, the working-space enum,
//! and the lossless-ish conversions to/from the existing 8-bit `RgbaImage` land
//! here with tests, but nothing is rewired yet — the shared loaders and cards
//! still produce 8-bit sRGB. Later phases (P2 wide-gamut, P3 model egress, P4
//! file output) adopt this type. It is deliberately not referenced by
//! production code yet, hence the module-level `dead_code` allowance below; each
//! later phase removes another piece of that scaffolding as it wires the type
//! in.
#![allow(dead_code)]

use image::{Rgba, RgbaImage};

/// The colour space a [`WorkingImage`]'s samples are encoded in. Phase 1 only
/// ever constructs [`WorkingSpace::Srgb`] (pure 8→16-bit widening, no gamut
/// change); [`WorkingSpace::ProPhoto`] is the decided target working space
/// (`docs/design/colour-pipeline.md`) that P2 switches the canonical surface to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkingSpace {
    /// sRGB primaries + transfer. The current pipeline's space.
    Srgb,
    /// ProPhoto RGB (ROMM): wide enough to contain the CMYK gamut at 16-bit.
    /// The target canonical space; not produced until P2.
    ProPhoto,
}

/// A 16-bit, straight-alpha RGBA working surface tagged with its colour space
/// and (optionally) the ICC profile that describes it.
///
/// Samples are laid out row-major as interleaved `[R, G, B, A]` `u16`s, so
/// `pixels.len() == width * height * 4`.
#[derive(Debug, Clone)]
pub(crate) struct WorkingImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Interleaved RGBA16, row-major. Length is `width * height * 4`.
    pub(crate) pixels: Vec<u16>,
    /// The space `pixels` are encoded in.
    pub(crate) space: WorkingSpace,
    /// The ICC profile describing `space`, when one should travel with the
    /// pixels (e.g. onto a manual-path file output). `None` for the implicit
    /// sRGB the current pipeline assumes.
    pub(crate) icc: Option<Vec<u8>>,
}

/// Expand an 8-bit sample to 16-bit by replicating the byte (`v * 257`), which
/// maps `0 -> 0` and `255 -> 65535` exactly and is invertible by [`narrow`].
#[inline]
fn widen(v: u8) -> u16 {
    u16::from(v) * 257
}

/// Round a 16-bit sample back to 8-bit. Exact inverse of [`widen`] on the
/// values it produces, and a correctly-rounded reduction otherwise.
#[inline]
fn narrow(v: u16) -> u8 {
    ((u32::from(v) * 255 + 32_767) / 65_535) as u8
}

impl WorkingImage {
    /// Number of pixels (`width * height`).
    pub(crate) fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Widen an 8-bit RGBA surface into a working image in `space`, carrying the
    /// given ICC profile. This is a pure bit-depth widening: no colour-space
    /// conversion happens, so the caller must pass the space the 8-bit pixels
    /// are already in.
    pub(crate) fn from_rgba8(image: &RgbaImage, space: WorkingSpace, icc: Option<Vec<u8>>) -> Self {
        let (width, height) = image.dimensions();
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for px in image.pixels() {
            pixels.push(widen(px.0[0]));
            pixels.push(widen(px.0[1]));
            pixels.push(widen(px.0[2]));
            pixels.push(widen(px.0[3]));
        }
        WorkingImage {
            width,
            height,
            pixels,
            space,
            icc,
        }
    }

    /// Reduce to an 8-bit RGBA surface (rounded). This does **not** colour-manage
    /// between spaces — it is the plain bit-depth narrowing used where the space
    /// is already what the consumer expects. The model-egress colour convert
    /// (P3) is a separate step layered on top of this.
    pub(crate) fn to_rgba8(&self) -> RgbaImage {
        let mut out = RgbaImage::new(self.width, self.height);
        for (px, chunk) in out.pixels_mut().zip(self.pixels.chunks_exact(4)) {
            *px = Rgba([
                narrow(chunk[0]),
                narrow(chunk[1]),
                narrow(chunk[2]),
                narrow(chunk[3]),
            ]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rgba8() -> RgbaImage {
        let mut img = RgbaImage::new(2, 2);
        img.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 0, Rgba([255, 255, 255, 255]));
        img.put_pixel(0, 1, Rgba([1, 128, 254, 0]));
        img.put_pixel(1, 1, Rgba([64, 32, 16, 200]));
        img
    }

    #[test]
    fn widen_narrow_are_inverse_on_all_bytes() {
        for v in 0u8..=255 {
            assert_eq!(narrow(widen(v)), v, "round-trip failed for {v}");
        }
        assert_eq!(widen(0), 0);
        assert_eq!(widen(255), 65_535);
    }

    #[test]
    fn from_rgba8_then_to_rgba8_is_identity() {
        let src = sample_rgba8();
        let work = WorkingImage::from_rgba8(&src, WorkingSpace::Srgb, None);
        assert_eq!((work.width, work.height), (2, 2));
        assert_eq!(work.pixels.len(), 2 * 2 * 4);
        assert_eq!(work.space, WorkingSpace::Srgb);
        let back = work.to_rgba8();
        assert_eq!(back.dimensions(), src.dimensions());
        for (a, b) in back.pixels().zip(src.pixels()) {
            assert_eq!(a.0, b.0);
        }
    }

    #[test]
    fn carries_icc_and_space() {
        let src = sample_rgba8();
        let icc = vec![1u8, 2, 3, 4];
        let work = WorkingImage::from_rgba8(&src, WorkingSpace::ProPhoto, Some(icc.clone()));
        assert_eq!(work.space, WorkingSpace::ProPhoto);
        assert_eq!(work.icc.as_deref(), Some(icc.as_slice()));
        assert_eq!(work.pixel_count(), 4);
    }

    #[test]
    fn white_widens_to_full_range() {
        let mut img = RgbaImage::new(1, 1);
        img.put_pixel(0, 0, Rgba([255, 255, 255, 255]));
        let work = WorkingImage::from_rgba8(&img, WorkingSpace::Srgb, None);
        assert_eq!(work.pixels, vec![65_535, 65_535, 65_535, 65_535]);
    }
}
