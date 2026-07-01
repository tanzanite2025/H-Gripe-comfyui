//! Raw CMYK sample decode for the in-process enhance path (R3 CMYK step c1).
//!
//! The `image` crate converts CMYK -> RGB and discards the embedded ICC profile
//! at decode time, so `studio_image::load_dynamic` can never see the original
//! ink samples. A faithful ICC CMYK -> sRGB transform (the later c2/c3 work)
//! needs those raw 4-channel samples plus the profile, which we read straight
//! from the container's own decoder here:
//!
//! - **JPEG** via `zune-jpeg`, pinning the *output* colourspace to `CMYK` so the
//!   decoder hands back the stored ink samples unconverted (zune copies the four
//!   channels through when input and output colourspace both equal CMYK). Only
//!   sources whose *input* colourspace is CMYK are taken; YCCK / Adobe-RGB JPEGs
//!   (which zune cannot pass through as CMYK) return `None` and stay on the
//!   Python fallback.
//! - **TIFF** via the `tiff` crate when the photometric interpretation is CMYK
//!   (8-bit, 4 samples/pixel).
//!
//! Wired into `try_enhance` via `cmyk_transform::cmyk_to_rgb8` for **TIFF** CMYK
//! (step c3); CMYK JPEGs still defer to Python (the caller gates on container).
//! The Adobe-APP14 inverted-ink convention and the CMS transform are *not*
//! handled here — this module only extracts the samples faithfully; the CMS
//! transform lives in `cmyk_transform`.

use std::path::Path;

use tiff::decoder::{Decoder as TiffDecoder, DecodingResult};
use tiff::tags::Tag;
use tiff::ColorType;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// Raw, unconverted CMYK samples plus the embedded ICC profile (if any).
///
/// `samples` is tightly packed, 4 bytes per pixel in C, M, Y, K order, row-major
/// (`width * height * 4` bytes). The values are exactly what the container's
/// decoder produced — no colour conversion and no Adobe inversion has been
/// applied (that is resolved by the later CMS step).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawCmyk {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) samples: Vec<u8>,
    pub(crate) icc: Option<Vec<u8>>,
}

enum Container {
    Jpeg,
    Tiff,
}

/// Attempt to read raw CMYK samples from `path`, guarding the decode size first.
///
/// Returns:
/// - `Ok(Some(raw))` — the source is a CMYK JPEG or CMYK TIFF we could decode.
/// - `Ok(None)` — the source is not a container/colour we handle here (an RGB
///   JPEG, a YCCK JPEG, a non-CMYK TIFF, or any other format). The caller should
///   fall back to the existing path.
/// - `Err(_)` — the source *is* a CMYK container we recognise but decoding it
///   failed (truncated file, oversize, unsupported bit depth, ...).
pub(crate) fn decode_cmyk(path: &Path, max_pixels: u64) -> Result<Option<RawCmyk>, String> {
    let bytes =
        std::fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    match sniff(&bytes) {
        Some(Container::Jpeg) => decode_cmyk_jpeg(&bytes, max_pixels),
        Some(Container::Tiff) => decode_cmyk_tiff(&bytes, max_pixels),
        None => Ok(None),
    }
}

/// Identify the container by its magic bytes (extension-independent, matching
/// the sniffing the rest of the Rust side already relies on).
fn sniff(bytes: &[u8]) -> Option<Container> {
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Some(Container::Jpeg);
    }
    let is_le_tiff = bytes.starts_with(&[0x49, 0x49, 0x2A, 0x00]);
    let is_be_tiff = bytes.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]);
    if is_le_tiff || is_be_tiff {
        return Some(Container::Tiff);
    }
    None
}

/// Reject a declared size that overflows the decode budget (`max_pixels == 0`
/// disables the guard), mirroring `studio_image::guard_dimensions` so a CMYK
/// source cannot bypass the decompression-bomb guard the other loaders enforce.
fn guard(width: u64, height: u64, max_pixels: u64) -> Result<(), String> {
    if max_pixels != 0 && width.saturating_mul(height) > max_pixels {
        return Err(format!(
            "input image too large to decode safely: {width}x{height} exceeds the {max_pixels} px budget"
        ));
    }
    Ok(())
}

fn decode_cmyk_jpeg(bytes: &[u8], max_pixels: u64) -> Result<Option<RawCmyk>, String> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::CMYK);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|err| format!("failed to read JPEG headers: {err:?}"))?;

    // Only CMYK sources pass through as CMYK; YCCK / RGB defer to Python.
    if decoder.input_colorspace() != Some(ColorSpace::CMYK) {
        return Ok(None);
    }

    let info = decoder
        .info()
        .ok_or_else(|| "JPEG headers decoded but info() was empty".to_string())?;
    let width = u32::from(info.width);
    let height = u32::from(info.height);
    guard(u64::from(width), u64::from(height), max_pixels)?;

    let icc = decoder.icc_profile();
    let samples = decoder
        .decode()
        .map_err(|err| format!("failed to decode CMYK JPEG: {err:?}"))?;

    finish(width, height, samples, icc, "JPEG")
}

fn decode_cmyk_tiff(bytes: &[u8], max_pixels: u64) -> Result<Option<RawCmyk>, String> {
    let mut decoder = TiffDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|err| format!("failed to open TIFF: {err}"))?;

    match decoder.colortype() {
        Ok(ColorType::CMYK(8)) => {}
        // A non-CMYK (or non-8-bit CMYK) TIFF is not ours to handle here.
        Ok(_) => return Ok(None),
        Err(err) => return Err(format!("failed to read TIFF colortype: {err}")),
    }

    let (width, height) = decoder
        .dimensions()
        .map_err(|err| format!("failed to read TIFF dimensions: {err}"))?;
    guard(u64::from(width), u64::from(height), max_pixels)?;

    let icc = decoder
        .find_tag(Tag::IccProfile)
        .ok()
        .flatten()
        .and_then(|value| value.into_u8_vec().ok());

    let samples = match decoder
        .read_image()
        .map_err(|err| format!("failed to decode CMYK TIFF: {err}"))?
    {
        DecodingResult::U8(buf) => buf,
        other => {
            return Err(format!(
                "CMYK TIFF decoded to an unexpected sample type: {other:?}"
            ))
        }
    };

    finish(width, height, samples, icc, "TIFF")
}

/// Validate the sample count against `width * height * 4` and wrap the result.
fn finish(
    width: u32,
    height: u32,
    samples: Vec<u8>,
    icc: Option<Vec<u8>>,
    kind: &str,
) -> Result<Option<RawCmyk>, String> {
    let expected = width as usize * height as usize * 4;
    if samples.len() != expected {
        return Err(format!(
            "CMYK {kind} produced {} samples, expected {expected} ({width}x{height}x4)",
            samples.len()
        ));
    }
    Ok(Some(RawCmyk {
        width,
        height,
        samples,
        icc,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::studio_image::DEFAULT_MAX_DECODE_PIXELS;
    use std::io::Cursor;
    use std::path::PathBuf;
    use tiff::encoder::{colortype, TiffEncoder};

    fn write_tmp(name: &str, bytes: &[u8]) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("hgripe_cmyk_{nanos}_{name}"));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn cmyk8_tiff(width: u32, height: u32, samples: &[u8]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            enc.write_image::<colortype::CMYK8>(width, height, samples)
                .unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn decodes_cmyk_tiff_samples_faithfully() {
        // 4 pixels, C M Y K order — a mix of ink extremes.
        let samples = vec![
            0, 0, 0, 0, // no ink
            255, 0, 0, 0, // full cyan
            0, 255, 0, 0, // full magenta
            0, 0, 200, 255, // yellow + black
        ];
        let path = write_tmp("rt.tiff", &cmyk8_tiff(4, 1, &samples));

        let raw = decode_cmyk(&path, DEFAULT_MAX_DECODE_PIXELS)
            .unwrap()
            .expect("a CMYK TIFF should decode to raw CMYK samples");
        assert_eq!((raw.width, raw.height), (4, 1));
        // Samples come back byte-for-byte, unconverted.
        assert_eq!(raw.samples, samples);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn non_cmyk_tiff_defers_to_caller() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            enc.write_image::<colortype::RGB8>(2, 1, &[1, 2, 3, 4, 5, 6])
                .unwrap();
        }
        let path = write_tmp("rgb.tiff", &buf.into_inner());

        let got = decode_cmyk(&path, DEFAULT_MAX_DECODE_PIXELS).unwrap();
        assert!(got.is_none(), "an RGB TIFF must defer (Ok(None))");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn non_image_defers_to_caller() {
        let path = write_tmp("notimg.bin", b"this is not an image at all");
        let got = decode_cmyk(&path, DEFAULT_MAX_DECODE_PIXELS).unwrap();
        assert!(got.is_none(), "a non-image file must defer (Ok(None))");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn guard_rejects_oversized_cmyk_before_decode() {
        let path = write_tmp("big.tiff", &cmyk8_tiff(4, 1, &[0u8; 16]));
        // A 1-pixel budget must reject the 4-pixel CMYK image.
        let err = decode_cmyk(&path, 1).unwrap_err();
        assert!(err.contains("too large to decode safely"), "{err}");
        let _ = std::fs::remove_file(&path);
    }
}
