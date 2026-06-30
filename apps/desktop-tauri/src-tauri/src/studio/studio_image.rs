//! Shared image-loading hardening for native-Rust Studio cards (the `Compute`
//! executor lane). This is the Rust counterpart of the Python bridge's
//! `_load_rgba` / `_load_mask` helpers: it rejects a decompression bomb *before*
//! allocating the decoded buffer, normalises the colour space / bit depth to a
//! plain 8-bit surface, applies EXIF orientation, and reports the provenance
//! (`source_mode`, `exif_transposed`) so a card's report can mirror the enriched
//! report convention used by the rest of the chain.
//!
//! Every later Rust card should load its pixels through here so the decode
//! guard and colour-space behaviour stay identical across cards.

use std::path::Path;

use image::metadata::Orientation;
use image::{DynamicImage, ExtendedColorType, GrayImage, ImageDecoder, ImageReader, RgbaImage};

/// Default decode budget, aligned with the Python PSD chain
/// (`--max-decode-pixels`). A source whose declared `width * height` exceeds
/// this is rejected before any pixel buffer is allocated.
pub(crate) const DEFAULT_MAX_DECODE_PIXELS: u64 = 96_000_000;

/// Provenance recorded while loading, surfaced into a card's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadMeta {
    /// The source colour mode label (e.g. `RGB`, `RGBA`, `CMYK`, `L`).
    pub(crate) source_mode: String,
    /// Whether a non-identity EXIF orientation was normalised away.
    pub(crate) exif_transposed: bool,
}

/// A decoded RGBA surface plus its provenance.
#[derive(Debug)]
pub(crate) struct LoadedRgba {
    pub(crate) image: RgbaImage,
    pub(crate) meta: LoadMeta,
}

/// Human-readable label for the *source* colour type, so a report can say what
/// was converted from (the decoded surface is always normalised to 8-bit).
fn source_mode_label(color: ExtendedColorType) -> String {
    match color {
        ExtendedColorType::Rgb8 | ExtendedColorType::Rgb16 | ExtendedColorType::Rgb32F => "RGB",
        ExtendedColorType::Rgba8 | ExtendedColorType::Rgba16 | ExtendedColorType::Rgba32F => "RGBA",
        ExtendedColorType::L8 | ExtendedColorType::L16 => "L",
        ExtendedColorType::La8 | ExtendedColorType::La16 => "LA",
        ExtendedColorType::Bgr8 => "RGB",
        ExtendedColorType::Bgra8 => "RGBA",
        ExtendedColorType::Cmyk8 => "CMYK",
        ExtendedColorType::Rgb4 | ExtendedColorType::Rgba4 => "RGBA",
        other => return format!("{other:?}"),
    }
    .to_string()
}

/// Reject a declared size that overflows the decode budget. `max_pixels == 0`
/// disables the guard. This runs *before* the pixels are read, so an attacker
/// cannot force a huge allocation by pointing a card at a decompression bomb.
fn guard_dimensions(path: &Path, width: u32, height: u32, max_pixels: u64) -> Result<(), String> {
    if max_pixels == 0 {
        return Ok(());
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > max_pixels {
        return Err(format!(
            "input image too large to decode safely: {} {}x{} = {} px exceeds the {} px budget",
            path.display(),
            width,
            height,
            pixels,
            max_pixels
        ));
    }
    Ok(())
}

/// Open + decode an image to an 8-bit RGBA surface, guarding the decode size
/// first and normalising colour space / bit depth / EXIF orientation.
pub(crate) fn load_rgba(path: &Path, max_pixels: u64) -> Result<LoadedRgba, String> {
    let (image, meta) = load_dynamic(path, max_pixels)?;
    Ok(LoadedRgba {
        image: image.into_rgba8(),
        meta,
    })
}

/// Open + decode an image to an 8-bit single-channel mask, guarding the decode
/// size first. High-bit-depth mattes are tone-scaled (not clipped) by the
/// `image` crate's luma conversion. (Mask provenance is not surfaced in Phase 1,
/// so only the pixels are returned.)
pub(crate) fn load_mask(path: &Path, max_pixels: u64) -> Result<GrayImage, String> {
    let (image, _meta) = load_dynamic(path, max_pixels)?;
    Ok(image.into_luma8())
}

/// Shared decode path: guard dimensions, read EXIF orientation + source mode,
/// decode, then apply the orientation so downstream pixels are upright.
fn load_dynamic(path: &Path, max_pixels: u64) -> Result<(DynamicImage, LoadMeta), String> {
    let reader = ImageReader::open(path)
        .map_err(|err| format!("failed to open {}: {err}", path.display()))?
        .with_guessed_format()
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|err| format!("failed to decode {}: {err}", path.display()))?;

    let (width, height) = decoder.dimensions();
    guard_dimensions(path, width, height, max_pixels)?;

    let source_mode = source_mode_label(decoder.original_color_type());
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let exif_transposed = orientation != Orientation::NoTransforms;

    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|err| format!("failed to decode {}: {err}", path.display()))?;
    image.apply_orientation(orientation);

    Ok((
        image,
        LoadMeta {
            source_mode,
            exif_transposed,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unique_tmp(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("hgripe_studio_image_{nanos}_{name}"))
    }

    #[test]
    fn guard_rejects_oversized_before_decode() {
        let err = guard_dimensions(
            Path::new("x.png"),
            20_000,
            20_000,
            DEFAULT_MAX_DECODE_PIXELS,
        )
        .unwrap_err();
        assert!(err.contains("too large to decode safely"), "{err}");
    }

    #[test]
    fn guard_disabled_when_budget_zero() {
        assert!(guard_dimensions(Path::new("x.png"), 50_000, 50_000, 0).is_ok());
    }

    #[test]
    fn loads_rgb_png_and_reports_source_mode() {
        let path = unique_tmp("rgb.png");
        let mut img = RgbaImage::new(4, 3);
        for p in img.pixels_mut() {
            *p = image::Rgba([10, 20, 30, 255]);
        }
        DynamicImage::ImageRgba8(img).save(&path).unwrap();

        let loaded = load_rgba(&path, DEFAULT_MAX_DECODE_PIXELS).unwrap();
        assert_eq!(loaded.image.dimensions(), (4, 3));
        // PNG stores the RGBA we wrote; the source mode reflects that.
        assert_eq!(loaded.meta.source_mode, "RGBA");
        assert!(!loaded.meta.exif_transposed);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_rejects_oversized_real_file() {
        let path = unique_tmp("small.png");
        DynamicImage::ImageRgba8(RgbaImage::new(8, 8))
            .save(&path)
            .unwrap();
        // A 1-pixel budget must reject the 64-pixel image before decoding it.
        let err = load_rgba(&path, 1).unwrap_err();
        assert!(err.contains("too large to decode safely"), "{err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn loads_mask_as_single_channel() {
        let path = unique_tmp("mask.png");
        let mut img = GrayImage::new(2, 2);
        img.put_pixel(0, 0, image::Luma([255]));
        DynamicImage::ImageLuma8(img).save(&path).unwrap();

        let mask = load_mask(&path, DEFAULT_MAX_DECODE_PIXELS).unwrap();
        assert_eq!(mask.dimensions(), (2, 2));
        assert_eq!(mask.get_pixel(0, 0).0[0], 255);
        let _ = std::fs::remove_file(&path);
    }
}
