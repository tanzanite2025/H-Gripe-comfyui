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
use image::{
    DynamicImage, ExtendedColorType, GrayImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage,
};

use super::image_buffer;
use super::working_image::{WorkingImage, WorkingSpace};

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

/// A decoded [`WorkingImage`] (16-bit canonical surface) plus its provenance.
/// This is the carrier the loader now builds first; [`load_rgba`] narrows it to
/// the 8-bit [`LoadedRgba`] the cards still consume (see `load_working`).
#[derive(Debug)]
pub(crate) struct LoadedWorking {
    pub(crate) image: WorkingImage,
    pub(crate) meta: LoadMeta,
}

/// The source colour type and its embedded ICC profile (if any), read from the
/// decoder header without decoding the pixels. The enhance fast path uses this
/// to pick a colour-space-aware decode strategy and to carry the profile onto
/// the output (mirroring the Python path's "preserve ICC when the colour model
/// is unchanged").
#[derive(Debug, Clone)]
pub(crate) struct SourceProbe {
    pub(crate) color: ExtendedColorType,
    pub(crate) icc: Option<Vec<u8>>,
}

/// Read the source colour type + ICC profile from the header only (no pixel
/// decode). Used by the in-process enhance fast path to pick its decode /
/// colour-management strategy and, for a CMYK / float input it still cannot
/// reproduce faithfully, to route back to the Python pipeline.
pub(crate) fn probe_source(path: &Path) -> Result<SourceProbe, String> {
    let reader = ImageReader::open(path)
        .map_err(|err| format!("failed to open {}: {err}", path.display()))?
        .with_guessed_format()
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let format = reader.format();
    let mut decoder = reader
        .into_decoder()
        .map_err(|err| format!("failed to decode {}: {err}", path.display()))?;
    let mut color = decoder.original_color_type();
    let icc = decoder.icc_profile().ok().flatten();

    // The `image` crate reports Adobe CMYK and YCCK JPEGs as `Rgb8` — it
    // converts them to RGB on decode and drops the embedded ICC. Sniff the JPEG
    // ourselves and reclassify those as CMYK so the enhance path routes them to
    // `cmyk_decode` (raw inks + ICC, colour-managed to sRGB) instead of the
    // lossy generic RGB decode. `decode_cmyk` still returns `None` for the CMYK
    // shapes it won't take faithfully, deferring those to Python.
    if format == Some(ImageFormat::Jpeg) && color != ExtendedColorType::Cmyk8 {
        if let Ok(bytes) = std::fs::read(path) {
            if super::cmyk_decode::is_cmyk_family_jpeg(&bytes) {
                color = ExtendedColorType::Cmyk8;
            }
        }
    }

    Ok(SourceProbe { color, icc })
}

/// Human-readable label for the *source* colour type, so a report can say what
/// was converted from (the decoded surface is always normalised to 8-bit).
pub(crate) fn source_mode_label(color: ExtendedColorType) -> String {
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

/// The provenance a freshly-written 8-bit RGBA output PNG would report if it
/// were reloaded: the surface is already normalised, so the source mode is
/// plain `RGBA` and there is no EXIF orientation to undo. Compute cards publish
/// their RGBA outputs to [`image_buffer`] with this meta so a cache hit mirrors
/// what a disk decode of the written file would produce.
pub(crate) fn png_output_meta() -> LoadMeta {
    LoadMeta {
        source_mode: "RGBA".to_string(),
        exif_transposed: false,
    }
}

/// Open + decode an image to an 8-bit RGBA surface, guarding the decode size
/// first and normalising colour space / bit depth / EXIF orientation.
///
/// A compute card upstream may have already published this exact surface to the
/// in-process [`image_buffer`] cache; a fresh hit there skips the file read and
/// decode entirely and is otherwise indistinguishable from decoding the PNG.
pub(crate) fn load_rgba(path: &Path, max_pixels: u64) -> Result<LoadedRgba, String> {
    if let Some(hit) = image_buffer::lookup_rgba(path, max_pixels) {
        return Ok(hit);
    }
    let LoadedWorking { image, meta } = load_working(path, max_pixels)?;
    Ok(LoadedRgba {
        image: image.to_rgba8(),
        meta,
    })
}

/// Decode a source into the canonical 16-bit [`WorkingImage`] carrier (the
/// cold, un-cached path — [`load_rgba`] handles the in-process cache before
/// calling here). Phase 2 keeps the surface in the `Srgb` working space and
/// only widens 8→16-bit (no gamut change), so narrowing it back reproduces the
/// previous 8-bit result exactly; the carrier additionally holds the source ICC
/// so later phases can consume it instead of dropping it.
pub(crate) fn load_working(path: &Path, max_pixels: u64) -> Result<LoadedWorking, String> {
    // CMYK-family sources (Adobe CMYK / YCCK JPEG, CMYK TIFF): the `image` crate
    // would decode them to RGB and silently discard the embedded ICC, so every
    // native card that loaded through here (crop, subject mask, ...) got
    // colour-shifted pixels. Decode the raw inks + profile ourselves and
    // colour-manage to sRGB, mirroring the enhance fast path. `decode_cmyk`
    // returns `None` for non-CMYK sources and the CMYK shapes it won't take
    // faithfully (an unmarked CMYK JPEG); both, and any decode error, fall
    // through to the generic decode below (unchanged behaviour).
    if let Ok(Some(raw)) = super::cmyk_decode::decode_cmyk(path, max_pixels) {
        if let Some(loaded) = cmyk_to_working(&raw) {
            return Ok(loaded);
        }
    }
    let (image, meta, icc) = load_dynamic(path, max_pixels)?;
    Ok(LoadedWorking {
        image: WorkingImage::from_rgba8(&image.into_rgba8(), WorkingSpace::Srgb, icc),
        meta,
    })
}

/// Build an opaque 16-bit `Srgb` [`WorkingImage`] from raw CMYK samples
/// colour-managed to sRGB (CMYK carries no alpha, so the alpha track is fully
/// opaque). Returns `None` on an empty or malformed buffer so the caller falls
/// back to the generic decode.
fn cmyk_to_working(raw: &super::cmyk_decode::RawCmyk) -> Option<LoadedWorking> {
    if raw.width == 0 || raw.height == 0 {
        return None;
    }
    let rgb = super::cmyk_transform::cmyk_to_rgb8(raw);
    let expected = raw.width as usize * raw.height as usize * 3;
    if rgb.len() != expected {
        return None;
    }
    let mut out = RgbaImage::new(raw.width, raw.height);
    for (px, chunk) in out.pixels_mut().zip(rgb.chunks_exact(3)) {
        *px = image::Rgba([chunk[0], chunk[1], chunk[2], 255]);
    }
    Some(LoadedWorking {
        image: WorkingImage::from_rgba8(&out, WorkingSpace::Srgb, None),
        meta: LoadMeta {
            source_mode: "CMYK".to_string(),
            exif_transposed: false,
        },
    })
}

/// Open + decode an image to an 8-bit single-channel mask, guarding the decode
/// size first. High-bit-depth mattes are tone-scaled (not clipped) by the
/// `image` crate's luma conversion. (Mask provenance is not surfaced in Phase 1,
/// so only the pixels are returned.)
///
/// Like [`load_rgba`], a mask published upstream to [`image_buffer`] is served
/// from memory on a fresh hit rather than re-decoded from disk.
pub(crate) fn load_mask(path: &Path, max_pixels: u64) -> Result<GrayImage, String> {
    if let Some(hit) = image_buffer::lookup_gray(path, max_pixels) {
        return Ok(hit);
    }
    let (image, _meta, _icc) = load_dynamic(path, max_pixels)?;
    Ok(image.into_luma8())
}

/// Shared decode path: guard dimensions, read EXIF orientation + source mode,
/// decode, then apply the orientation so downstream pixels are upright.
///
/// Exposed to the enhance fast path, which needs the native (pre-`into_rgba8`)
/// surface to range-scale a high-bit single-channel source itself rather than
/// let the default 8-bit conversion truncate its tonal range.
pub(crate) fn load_dynamic(
    path: &Path,
    max_pixels: u64,
) -> Result<(DynamicImage, LoadMeta, Option<Vec<u8>>), String> {
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
    // Read the embedded ICC off the header before `from_decoder` consumes the
    // decoder, so the working-surface carrier can hold it (the generic 8-bit
    // return still drops it, matching current behaviour).
    let icc = decoder.icc_profile().ok().flatten();

    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|err| format!("failed to decode {}: {err}", path.display()))?;
    image.apply_orientation(orientation);

    Ok((
        image,
        LoadMeta {
            source_mode,
            exif_transposed,
        },
        icc,
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

    #[test]
    fn load_rgba_prefers_a_published_buffer() {
        let path = unique_tmp("published_rgba.png");
        // On disk: red. A published green buffer must shadow it, proving the
        // loader served the in-process buffer without re-decoding the PNG.
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(3, 2, image::Rgba([255, 0, 0, 255])))
            .save(&path)
            .unwrap();
        image_buffer::publish_rgba(
            &path,
            &RgbaImage::from_pixel(3, 2, image::Rgba([0, 255, 0, 255])),
            png_output_meta(),
        );

        let loaded = load_rgba(&path, DEFAULT_MAX_DECODE_PIXELS).unwrap();
        assert_eq!(loaded.image.get_pixel(0, 0).0, [0, 255, 0, 255]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_mask_prefers_a_published_buffer() {
        let path = unique_tmp("published_mask.png");
        DynamicImage::ImageLuma8(GrayImage::from_pixel(2, 2, image::Luma([10])))
            .save(&path)
            .unwrap();
        image_buffer::publish_gray(&path, &GrayImage::from_pixel(2, 2, image::Luma([200])));

        let mask = load_mask(&path, DEFAULT_MAX_DECODE_PIXELS).unwrap();
        assert_eq!(mask.get_pixel(0, 0).0[0], 200);
        let _ = std::fs::remove_file(&path);
    }

    // The `image` crate decodes Adobe CMYK and YCCK JPEGs to RGB (dropping the
    // ICC) and reports them as `Rgb8`; the probe must reclassify both as CMYK so
    // the enhance path takes them through `cmyk_decode` instead. A regression
    // here silently routes CMYK JPEGs back through the lossy generic decode.
    #[test]
    fn probes_adobe_cmyk_jpeg_as_cmyk() {
        let path = unique_tmp("adobe_cmyk.jpg");
        std::fs::write(
            &path,
            include_bytes!("../../tests/fixtures/cmyk_adobe_app14.jpg"),
        )
        .unwrap();
        let probe = probe_source(&path).unwrap();
        assert_eq!(probe.color, ExtendedColorType::Cmyk8);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn probes_ycck_jpeg_as_cmyk() {
        let path = unique_tmp("ycck.jpg");
        std::fs::write(
            &path,
            include_bytes!("../../tests/fixtures/cmyk_ycck_app14.jpg"),
        )
        .unwrap();
        let probe = probe_source(&path).unwrap();
        assert_eq!(probe.color, ExtendedColorType::Cmyk8);
        let _ = std::fs::remove_file(&path);
    }

    // A native card (crop, subject mask, ...) loading a CMYK JPEG must get
    // colour-managed sRGB, not the `image` crate's lossy CMYK->RGB with the ICC
    // dropped. The tile centres must land on the sRGB Pillow produces (naive
    // path; the fixture carries no ICC), the source mode must read `CMYK`, and
    // the alpha track must be fully opaque.
    #[test]
    fn load_rgba_colour_manages_cmyk_jpeg() {
        let path = unique_tmp("enhance_cmyk.jpg");
        std::fs::write(
            &path,
            include_bytes!("../../tests/fixtures/cmyk_adobe_app14.jpg"),
        )
        .unwrap();

        let loaded = load_rgba(&path, DEFAULT_MAX_DECODE_PIXELS).unwrap();
        assert_eq!(loaded.image.dimensions(), (32, 32));
        assert_eq!(loaded.meta.source_mode, "CMYK");

        let expected: [((u32, u32), [u8; 3]); 4] = [
            ((8, 8), [255, 255, 255]),
            ((24, 8), [0, 255, 255]),
            ((8, 24), [0, 0, 0]),
            ((24, 24), [119, 179, 209]),
        ];
        for ((x, y), want) in expected {
            let px = loaded.image.get_pixel(x, y).0;
            assert_eq!(px[3], 255, "alpha must be opaque at ({x},{y})");
            for ch in 0..3 {
                assert!(
                    (i32::from(px[ch]) - i32::from(want[ch])).abs() <= 6,
                    "tile ({x},{y}) ch {ch}: rust {} vs PIL {}",
                    px[ch],
                    want[ch]
                );
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    // The loader now decodes into the 16-bit `WorkingImage` carrier first, then
    // narrows to 8-bit. Widening (`* 257`) is exactly invertible, so the narrowed
    // surface must be byte-identical to what `load_rgba` returns; the carrier
    // also reports the right dimensions and the `Srgb` working space.
    #[test]
    fn load_working_widens_to_16bit_and_narrows_identically() {
        let path = unique_tmp("working_rgb.png");
        let mut img = RgbaImage::new(4, 3);
        let mut n = 0u8;
        for p in img.pixels_mut() {
            *p = image::Rgba([n, n.wrapping_add(50), n.wrapping_add(100), 255]);
            n = n.wrapping_add(7);
        }
        DynamicImage::ImageRgba8(img).save(&path).unwrap();

        let work = load_working(&path, DEFAULT_MAX_DECODE_PIXELS).unwrap();
        assert_eq!((work.image.width, work.image.height), (4, 3));
        assert_eq!(work.image.pixels.len(), 4 * 3 * 4);
        assert_eq!(work.image.space, WorkingSpace::Srgb);

        let narrowed = work.image.to_rgba8();
        let loaded = load_rgba(&path, DEFAULT_MAX_DECODE_PIXELS).unwrap();
        assert_eq!(narrowed, loaded.image);
        assert_eq!(loaded.meta.source_mode, "RGBA");
        let _ = std::fs::remove_file(&path);
    }

    // A CMYK source is colour-managed into the 16-bit `Srgb` carrier; narrowing
    // reproduces the same sRGB the pre-carrier path produced (no gamut change in
    // this phase), and the provenance still reads `CMYK`.
    #[test]
    fn load_working_colour_manages_cmyk_to_16bit_srgb() {
        let path = unique_tmp("working_cmyk.jpg");
        std::fs::write(
            &path,
            include_bytes!("../../tests/fixtures/cmyk_adobe_app14.jpg"),
        )
        .unwrap();

        let work = load_working(&path, DEFAULT_MAX_DECODE_PIXELS).unwrap();
        assert_eq!((work.image.width, work.image.height), (32, 32));
        assert_eq!(work.image.space, WorkingSpace::Srgb);
        assert_eq!(work.meta.source_mode, "CMYK");

        let narrowed = work.image.to_rgba8();
        let loaded = load_rgba(&path, DEFAULT_MAX_DECODE_PIXELS).unwrap();
        assert_eq!(narrowed, loaded.image);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn probes_plain_rgb_jpeg_as_rgb() {
        let path = unique_tmp("rgb.jpg");
        DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([200, 120, 60])))
            .save_with_format(&path, ImageFormat::Jpeg)
            .unwrap();
        let probe = probe_source(&path).unwrap();
        assert_eq!(probe.color, ExtendedColorType::Rgb8);
        let _ = std::fs::remove_file(&path);
    }
}
