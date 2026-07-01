//! The canonical **working surface** for the Studio colour pipeline: a 16-bit
//! RGBA buffer plus the working colour space it lives in and an optional ICC
//! profile describing it. This is the internal "truth" every card will operate
//! on so the manual path keeps full grading fidelity; the sRGB down-convert is
//! deferred to the model-egress boundary. See
//! [`docs/design/colour-pipeline.md`](../../../../../docs/design/colour-pipeline.md).
//!
//! **Phase 2b/3 (current):** the shared loader ([`super::studio_image::load_working`])
//! tags each decoded surface with its *actual* space. Sources that genuinely
//! carry wide-gamut information — CMYK with an embedded ICC profile — are
//! colour-managed straight into 16-bit `ProPhoto`; everything else (plain sRGB
//! images, and unprofiled/naive CMYK whose values are already sRGB-range) stays
//! `Srgb` as a pure 8→16-bit widen. The cards still consume 8-bit sRGB, so
//! [`WorkingImage::to_srgb_rgba8`] is the model/output egress (P3): it
//! colour-manages `ProPhoto → sRGB` when needed and is an exact bit-narrow for
//! `Srgb` (so plain images and naive CMYK stay byte-for-byte, never round-tripped
//! through ProPhoto). The wide-gamut `ProPhoto` surface + its `icc` are what the
//! manual-path 16-bit file output (P4) will consume directly. `icc` is not yet
//! read in production, hence the retained module-level `dead_code` allowance.
#![allow(dead_code)]

use image::{Rgba, RgbaImage};
use moxcms::{ColorProfile, Layout, TransformExecutor, TransformOptions};

/// The colour space a [`WorkingImage`]'s samples are encoded in. The loader
/// tags each surface with its actual space: profiled (wide-gamut) CMYK becomes
/// [`WorkingSpace::ProPhoto`], while plain images and naive CMYK stay
/// [`WorkingSpace::Srgb`]. Egress ([`WorkingImage::to_srgb_rgba8`]) branches on
/// this. See `docs/design/colour-pipeline.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkingSpace {
    /// sRGB primaries + transfer. Egress is an exact bit-narrow.
    Srgb,
    /// ProPhoto RGB (ROMM): wide enough to contain the CMYK gamut at 16-bit.
    /// Egress colour-manages down to sRGB.
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
pub(crate) fn widen(v: u8) -> u16 {
    u16::from(v) * 257
}

/// Round a 16-bit sample back to 8-bit. Exact inverse of [`widen`] on the
/// values it produces, and a correctly-rounded reduction otherwise.
#[inline]
pub(crate) fn narrow(v: u16) -> u8 {
    ((u32::from(v) * 255 + 32_767) / 65_535) as u8
}

/// Colour-manage a packed **sRGB** 8-bit RGB buffer (`pixels * 3` bytes) into a
/// packed **ProPhoto** 16-bit RGB buffer (`pixels * 3` samples) via moxcms.
/// sRGB is a strict subset of ProPhoto, so this is loss-free (every value lands
/// in gamut); the 16-bit target leaves headroom for the wider CMYK gamut that
/// shares this space. Returns `None` on any transform failure so callers can
/// fall back to a plain widen.
pub(crate) fn srgb8_rgb_to_prophoto16(rgb8: &[u8], pixels: usize) -> Option<Vec<u16>> {
    if rgb8.len() != pixels * 3 {
        return None;
    }
    let src = ColorProfile::new_srgb();
    let dst = ColorProfile::new_pro_photo_rgb();
    let transform = src
        .create_transform_16bit(Layout::Rgb, &dst, Layout::Rgb, TransformOptions::default())
        .ok()?;
    let src16: Vec<u16> = rgb8.iter().map(|&v| widen(v)).collect();
    let mut out = vec![0u16; pixels * 3];
    transform.transform(&src16, &mut out).ok()?;
    Some(out)
}

/// Colour-manage a packed **ProPhoto** 16-bit RGB buffer (`pixels * 3` samples)
/// down to packed **sRGB** 8-bit RGB (`pixels * 3` bytes) via moxcms — the
/// model/output egress. ProPhoto values outside the sRGB gamut are clipped by
/// the transform. Returns `None` on any transform failure.
pub(crate) fn prophoto16_rgb_to_srgb8(rgb16: &[u16], pixels: usize) -> Option<Vec<u8>> {
    if rgb16.len() != pixels * 3 {
        return None;
    }
    let src = ColorProfile::new_pro_photo_rgb();
    let dst = ColorProfile::new_srgb();
    let transform = src
        .create_transform_16bit(Layout::Rgb, &dst, Layout::Rgb, TransformOptions::default())
        .ok()?;
    let mut out16 = vec![0u16; pixels * 3];
    transform.transform(rgb16, &mut out16).ok()?;
    Some(out16.iter().map(|&v| narrow(v)).collect())
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
    /// is already what the consumer expects. [`WorkingImage::to_srgb_rgba8`] is
    /// the colour-managed egress layered on top of this.
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

    /// Build an opaque ProPhoto working surface from a packed ProPhoto **16-bit
    /// RGB** buffer (`width * height * 3` samples). Alpha is set fully opaque;
    /// the space is [`WorkingSpace::ProPhoto`]. Used by the CMYK→ProPhoto path.
    pub(crate) fn from_prophoto_rgb16(
        width: u32,
        height: u32,
        rgb16: &[u16],
        icc: Option<Vec<u8>>,
    ) -> Self {
        let count = width as usize * height as usize;
        debug_assert_eq!(rgb16.len(), count * 3);
        let mut pixels = Vec::with_capacity(count * 4);
        for chunk in rgb16.chunks_exact(3) {
            pixels.push(chunk[0]);
            pixels.push(chunk[1]);
            pixels.push(chunk[2]);
            pixels.push(u16::MAX);
        }
        WorkingImage {
            width,
            height,
            pixels,
            space: WorkingSpace::ProPhoto,
            icc,
        }
    }

    /// Colour-managed egress to the 8-bit **sRGB** RGBA surface the cards consume.
    ///
    /// - [`WorkingSpace::Srgb`]: an exact bit-narrow ([`to_rgba8`](Self::to_rgba8)),
    ///   so plain images and naive CMYK reach the cards byte-for-byte, never
    ///   round-tripped through a wider space.
    /// - [`WorkingSpace::ProPhoto`]: `ProPhoto → sRGB` via moxcms for the colour
    ///   channels, with alpha carried straight (never colour-managed). Falls back
    ///   to a plain narrow if the transform fails.
    pub(crate) fn to_srgb_rgba8(&self) -> RgbaImage {
        match self.space {
            WorkingSpace::Srgb => self.to_rgba8(),
            WorkingSpace::ProPhoto => {
                let count = self.pixel_count();
                let mut rgb16 = Vec::with_capacity(count * 3);
                for chunk in self.pixels.chunks_exact(4) {
                    rgb16.push(chunk[0]);
                    rgb16.push(chunk[1]);
                    rgb16.push(chunk[2]);
                }
                match prophoto16_rgb_to_srgb8(&rgb16, count) {
                    Some(rgb8) => {
                        let mut out = RgbaImage::new(self.width, self.height);
                        for (px, (rgb, src)) in out
                            .pixels_mut()
                            .zip(rgb8.chunks_exact(3).zip(self.pixels.chunks_exact(4)))
                        {
                            *px = Rgba([rgb[0], rgb[1], rgb[2], narrow(src[3])]);
                        }
                        out
                    }
                    None => self.to_rgba8(),
                }
            }
        }
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

    #[test]
    fn srgb_prophoto_round_trip_is_near_identity() {
        // sRGB is a subset of ProPhoto, so sRGB -> ProPhoto16 -> sRGB8 must come
        // back within a rounding step. This bounds the egress error the cards
        // see for anything that *did* travel through ProPhoto.
        let rgb8: Vec<u8> = vec![0, 0, 0, 255, 255, 255, 1, 128, 254, 64, 32, 16, 200, 100, 50, 12, 240, 33];
        let px = rgb8.len() / 3;
        let wide = srgb8_rgb_to_prophoto16(&rgb8, px).expect("srgb->prophoto");
        assert_eq!(wide.len(), px * 3);
        let back = prophoto16_rgb_to_srgb8(&wide, px).expect("prophoto->srgb");
        assert_eq!(back.len(), rgb8.len());
        for (i, (&got, &want)) in back.iter().zip(rgb8.iter()).enumerate() {
            assert!(
                (i32::from(got) - i32::from(want)).abs() <= 3,
                "channel {i}: round-trip {got} vs {want} exceeds ±3",
            );
        }
    }

    #[test]
    fn prophoto_egress_narrows_alpha_and_manages_colour() {
        // White in ProPhoto must egress to white in sRGB, and alpha must be
        // carried straight (never colour-managed).
        let rgb16 = vec![65_535u16, 65_535, 65_535];
        let mut work = WorkingImage::from_prophoto_rgb16(1, 1, &rgb16, None);
        work.pixels[3] = widen(128); // set a non-opaque alpha
        let out = work.to_srgb_rgba8();
        let px = out.get_pixel(0, 0).0;
        assert!(px[0] >= 253 && px[1] >= 253 && px[2] >= 253, "white must stay white");
        assert_eq!(px[3], 128, "alpha must narrow straight, not be colour-managed");
    }

    #[test]
    fn srgb_space_egress_is_exact_bit_narrow() {
        // An Srgb-tagged surface must reach the cards byte-for-byte (identical to
        // the plain narrow) - no ProPhoto round-trip for plain images / naive CMYK.
        let src = sample_rgba8();
        let work = WorkingImage::from_rgba8(&src, WorkingSpace::Srgb, None);
        let egress = work.to_srgb_rgba8();
        let narrowed = work.to_rgba8();
        assert_eq!(egress.into_raw(), narrowed.into_raw());
    }
}
