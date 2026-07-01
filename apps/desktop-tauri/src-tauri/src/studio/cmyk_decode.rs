//! Raw CMYK sample decode for the in-process enhance path (R3 CMYK step c1).
//!
//! The `image` crate converts CMYK -> RGB and discards the embedded ICC profile
//! at decode time, so `studio_image::load_dynamic` can never see the original
//! ink samples. A faithful ICC CMYK -> sRGB transform (the later c2/c3 work)
//! needs those raw 4-channel samples plus the profile, which we read straight
//! from the container's own decoder here:
//!
//! - **JPEG** via `zune-jpeg`. Two **Adobe** shapes are taken (both store
//!   *inverted* ink, 0 = full ink, which we undo with `255 - v` to land in the
//!   device direction 0 = no ink that matches TIFF Separated and the
//!   `cmyk_transform` input contract):
//!     - **Adobe CMYK** (APP14 transform 0): the output colourspace is pinned to
//!       `CMYK` so the decoder copies the four stored ink channels through
//!       unconverted (zune copies straight through when input and output
//!       colourspace both equal CMYK).
//!     - **Adobe YCCK** (APP14 transform 2): zune only offers YCCK->RGB, which
//!       collapses to three channels and drops the embedded ICC. Instead the
//!       output colourspace is pinned to `YCCK` so the same straight-through
//!       copy hands back the raw Y/Cb/Cr/K planes, and we run the YCCK->CMYK
//!       reconstruction ourselves (libjpeg's `ycck_cmyk_convert`: YCbCr->RGB
//!       then C=255-R, M=255-G, Y=255-B, K passthrough), preserving the ICC.
//!   CMYK JPEGs without an Adobe marker (transform code absent) and any other
//!   JPEG return `None` and stay on the Python fallback — an unmarked CMYK JPEG
//!   is too rare to generate and validate a round-trip for.
//! - **TIFF** via the `tiff` crate when the photometric interpretation is CMYK
//!   (CMYK or CMYK+alpha, 8- or 16-bit). 16-bit is scaled to 8-bit (`v >> 8`)
//!   and any alpha channel is dropped, so the result is always tightly-packed
//!   8-bit CMYK. TIFF Separated is already 0 = no ink, so no inversion is
//!   applied.
//!
//! Wired into `try_enhance` via `cmyk_transform::cmyk_to_rgb8` for **TIFF** and
//! **Adobe CMYK / YCCK JPEG** sources (step c3). The samples this module returns
//! are always in the device direction (0 = no ink); the CMS / naive transform
//! lives in `cmyk_transform`.

use std::path::Path;

use tiff::decoder::{Decoder as TiffDecoder, DecodingResult};
use tiff::tags::Tag;
use tiff::ColorType;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// Raw CMYK samples plus the embedded ICC profile (if any).
///
/// `samples` is tightly packed, 4 bytes per pixel in C, M, Y, K order, row-major
/// (`width * height * 4` bytes). No colour conversion has been applied, but the
/// samples are normalised to the device direction (0 = no ink): a TIFF is taken
/// as-is and an Adobe CMYK / YCCK JPEG has its stored inversion undone here, so
/// the later CMS / naive transform always sees the same convention.
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
/// - `Ok(Some(raw))` — the source is an Adobe CMYK/YCCK JPEG or a CMYK TIFF we
///   could decode, returned in the device direction (0 = no ink).
/// - `Ok(None)` — the source is not a container/colour we handle here (an RGB
///   JPEG, a non-Adobe/unmarked CMYK JPEG, a non-CMYK TIFF, or any other
///   format). The caller should fall back to the existing path.
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

/// Whether `bytes` is a JPEG whose sample data is four-channel CMYK-family
/// (Adobe CMYK or YCCK).
///
/// The `image` crate reports both as `Rgb8` — it converts to RGB on decode and
/// drops the embedded ICC — so [`studio_image::probe_source`] cannot tell them
/// apart from an ordinary RGB JPEG. The enhance probe uses this to route such
/// JPEGs to the CMYK fast path ([`decode_cmyk`] + `cmyk_transform`) instead of
/// the lossy generic decode. `decode_cmyk_jpeg` still has the final say and
/// returns `None` for the CMYK shapes it won't take faithfully (e.g. an
/// unmarked CMYK JPEG), which keeps those on the Python fallback.
pub(crate) fn is_cmyk_family_jpeg(bytes: &[u8]) -> bool {
    if !matches!(sniff(bytes), Some(Container::Jpeg)) {
        return false;
    }
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), DecoderOptions::default());
    if decoder.decode_headers().is_err() {
        return false;
    }
    matches!(
        decoder.input_colorspace(),
        Some(ColorSpace::CMYK) | Some(ColorSpace::YCCK)
    )
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

/// Which Adobe JPEG shape `decode_cmyk_jpeg` recognised, and hence how to turn
/// the decoded planes into device-direction CMYK.
enum JpegCmyk {
    /// Adobe CMYK (transform 0): stored ink samples, only `255 - v` to undo.
    Cmyk,
    /// Adobe YCCK (transform 2): raw Y/Cb/Cr/K planes needing reconstruction.
    Ycck,
}

fn decode_cmyk_jpeg(bytes: &[u8], max_pixels: u64) -> Result<Option<RawCmyk>, String> {
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), DecoderOptions::default());
    decoder
        .decode_headers()
        .map_err(|err| format!("failed to read JPEG headers: {err:?}"))?;

    // Decide whether (and how) to take this JPEG from its input colourspace and
    // the Adobe APP14 transform code. Only the two Adobe shapes are handled;
    // an unmarked CMYK JPEG or anything else defers to the Python bridge.
    let mode = match (decoder.input_colorspace(), adobe_transform(bytes)) {
        (Some(ColorSpace::CMYK), Some(0)) => JpegCmyk::Cmyk,
        (Some(ColorSpace::YCCK), Some(2)) => JpegCmyk::Ycck,
        _ => return Ok(None),
    };

    // Pin the output colourspace to the input so zune copies the samples through
    // unconverted (four channels straight out): CMYK stays CMYK, and YCCK stays
    // YCCK so we get the raw planes plus the intact ICC rather than zune's
    // lossy YCCK->RGB.
    let out_colorspace = match mode {
        JpegCmyk::Cmyk => ColorSpace::CMYK,
        JpegCmyk::Ycck => ColorSpace::YCCK,
    };
    decoder.set_options(decoder.options().jpeg_set_out_colorspace(out_colorspace));

    let info = decoder
        .info()
        .ok_or_else(|| "JPEG headers decoded but info() was empty".to_string())?;
    let width = u32::from(info.width);
    let height = u32::from(info.height);
    guard(u64::from(width), u64::from(height), max_pixels)?;

    let icc = decoder.icc_profile();
    let mut samples = decoder
        .decode()
        .map_err(|err| format!("failed to decode CMYK JPEG: {err:?}"))?;

    match mode {
        // Undo the Adobe inversion so the samples match the device direction
        // (0 = no ink) that TIFF Separated and `cmyk_transform` expect.
        JpegCmyk::Cmyk => {
            for v in &mut samples {
                *v = 255 - *v;
            }
        }
        JpegCmyk::Ycck => ycck_planes_to_device_cmyk(&mut samples),
    }

    finish(width, height, samples, icc, "JPEG")
}

/// Convert raw Adobe YCCK planes (interleaved Y, Cb, Cr, K) in place to
/// device-direction CMYK (0 = no ink).
///
/// libjpeg's `ycck_cmyk_convert` reconstructs the *stored* CMYK as YCbCr->RGB
/// then C=255-R, M=255-G, Y=255-B (K passthrough); undoing the Adobe inversion
/// (`255 - v`) collapses that to device C=R, M=G, Y=B and device K=255-K. Uses
/// the full-range JFIF YCbCr->RGB coefficients.
fn ycck_planes_to_device_cmyk(samples: &mut [u8]) {
    for px in samples.chunks_exact_mut(4) {
        let y = f32::from(px[0]);
        let cb = f32::from(px[1]) - 128.0;
        let cr = f32::from(px[2]) - 128.0;
        let clamp = |v: f32| v.round().clamp(0.0, 255.0) as u8;
        px[0] = clamp(y + 1.402 * cr);
        px[1] = clamp(y - 0.344_136 * cb - 0.714_136 * cr);
        px[2] = clamp(y + 1.772 * cb);
        px[3] = 255 - px[3];
    }
}

/// Scan a JPEG's marker segments for the Adobe APP14 marker and return its
/// colour-transform code (`0` = unknown/CMYK-or-RGB, `1` = YCbCr, `2` = YCCK).
/// Returns `None` when there is no Adobe APP14 marker. Parsing stops at
/// start-of-scan; the marker always precedes it.
fn adobe_transform(bytes: &[u8]) -> Option<u8> {
    // Skip the SOI (`FF D8`).
    let mut i = 2usize;
    while i + 3 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        // A run of 0xFF is fill/padding before the real marker byte; step over
        // one 0xFF and re-read.
        if marker == 0xFF {
            i += 1;
            continue;
        }
        // Standalone markers (SOI/EOI, RSTn, TEM) carry no length payload.
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        // Start-of-scan: entropy-coded data follows; the Adobe marker, if any,
        // has already been seen.
        if marker == 0xDA {
            return None;
        }
        let seg_len = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
        if seg_len < 2 {
            return None;
        }
        let payload_start = i + 4;
        let payload_end = i + 2 + seg_len;
        if payload_end > bytes.len() {
            return None;
        }
        // APP14 with an "Adobe" identifier: 5-byte tag, 2+2+2 version/flags, then
        // the 1-byte transform code (14 bytes total).
        if marker == 0xEE {
            let payload = &bytes[payload_start..payload_end];
            if payload.len() >= 12 && payload.starts_with(b"Adobe") {
                return Some(payload[11]);
            }
        }
        i = payload_end;
    }
    None
}

fn decode_cmyk_tiff(bytes: &[u8], max_pixels: u64) -> Result<Option<RawCmyk>, String> {
    let mut decoder = TiffDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|err| format!("failed to open TIFF: {err}"))?;

    // Take CMYK and CMYK+alpha at 8 or 16 bits per channel. 16-bit is scaled
    // down to the 8-bit working space (`v >> 8`) and any alpha channel is
    // dropped (the enhance path emits opaque sRGB), so the samples always land
    // as tightly-packed 8-bit CMYK. TIFF Separated is already 0 = no ink, so no
    // inversion is applied. Other photometrics, bit depths (32-bit / float) or
    // sample layouts are not ours and defer to the caller.
    let channels: usize = match decoder.colortype() {
        Ok(ColorType::CMYK(8)) | Ok(ColorType::CMYK(16)) => 4,
        Ok(ColorType::CMYKA(8)) | Ok(ColorType::CMYKA(16)) => 5,
        Ok(_) => return Ok(None),
        Err(err) => return Err(format!("failed to read TIFF colortype: {err}")),
    };

    let (width, height) = decoder
        .dimensions()
        .map_err(|err| format!("failed to read TIFF dimensions: {err}"))?;
    guard(u64::from(width), u64::from(height), max_pixels)?;

    let icc = decoder
        .find_tag(Tag::IccProfile)
        .ok()
        .flatten()
        .and_then(|value| value.into_u8_vec().ok());

    // Flatten to one 8-bit sample per channel, whatever bit depth the TIFF used.
    let flat: Vec<u8> = match decoder
        .read_image()
        .map_err(|err| format!("failed to decode CMYK TIFF: {err}"))?
    {
        DecodingResult::U8(buf) => buf,
        DecodingResult::U16(buf) => buf.into_iter().map(|v| (v >> 8) as u8).collect(),
        other => {
            return Err(format!(
                "CMYK TIFF decoded to an unexpected sample type: {other:?}"
            ))
        }
    };

    // Drop the alpha channel when present so downstream always sees 4-channel CMYK.
    let samples = if channels == 5 {
        flat.chunks_exact(5).flat_map(|px| px[..4].to_vec()).collect()
    } else {
        flat
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

    fn cmyk16_tiff(width: u32, height: u32, samples: &[u16]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            enc.write_image::<colortype::CMYK16>(width, height, samples)
                .unwrap();
        }
        buf.into_inner()
    }

    fn cmyka8_tiff(width: u32, height: u32, samples: &[u8]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            enc.write_image::<colortype::CMYKA8>(width, height, samples)
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
    fn decodes_16bit_cmyk_tiff_scaled_to_8bit() {
        // 3 pixels, C M Y K order, 16-bit. `v >> 8` is the 8-bit scale-down.
        let samples16: Vec<u16> = vec![
            0, 0, 0, 0, // no ink -> 0,0,0,0
            0xFFFF, 0x8000, 0x0100, 0x00FF, // -> 255, 128, 1, 0
            0x1234, 0xABCD, 0xFF00, 0x00AB, // -> 0x12, 0xAB, 0xFF, 0x00
        ];
        let path = write_tmp("rt16.tiff", &cmyk16_tiff(3, 1, &samples16));

        let raw = decode_cmyk(&path, DEFAULT_MAX_DECODE_PIXELS)
            .unwrap()
            .expect("a 16-bit CMYK TIFF should decode");
        assert_eq!((raw.width, raw.height), (3, 1));
        let expected: Vec<u8> = samples16.iter().map(|&v| (v >> 8) as u8).collect();
        assert_eq!(raw.samples, expected);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn decodes_cmyk_alpha_tiff_dropping_alpha() {
        // 2 pixels, C M Y K A order (5 samples/pixel); the alpha byte is dropped.
        let samples = vec![
            10, 20, 30, 40, 200, // px0: alpha 200 dropped
            50, 60, 70, 80, 5, // px1: alpha 5 dropped
        ];
        let path = write_tmp("rta.tiff", &cmyka8_tiff(2, 1, &samples));

        let raw = decode_cmyk(&path, DEFAULT_MAX_DECODE_PIXELS)
            .unwrap()
            .expect("a CMYK+alpha TIFF should decode");
        assert_eq!((raw.width, raw.height), (2, 1));
        assert_eq!(raw.samples, vec![10, 20, 30, 40, 50, 60, 70, 80]);
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

    // A PIL-generated 32x32 Adobe CMYK JPEG (APP14 transform 0), a 2x2 grid of
    // flat ink tiles: top-left no-ink, top-right full-cyan, bottom-left full-K,
    // bottom-right mixed (128, 64, 32, 16). q100 + no chroma subsampling keeps
    // the tiles near-lossless. See `scripts/gen_cmyk_jpeg_fixture.py`.
    const ADOBE_CMYK_JPEG: &[u8] = include_bytes!("../../tests/fixtures/cmyk_adobe_app14.jpg");

    // The YCCK sibling of the fixture above (Adobe APP14 transform 2), same 2x2
    // ink tiles, written by `scripts/gen_ycck_jpeg_fixture.py`.
    const YCCK_CMYK_JPEG: &[u8] = include_bytes!("../../tests/fixtures/cmyk_ycck_app14.jpg");

    // Tile-centre (x, y) samples, well inside each 16x16 tile so JPEG block
    // edges don't bleed in, paired with the device inks PIL round-trips them to.
    const TILE_CENTRES: [((u32, u32), [u8; 4]); 4] = [
        ((8, 8), [0, 0, 0, 0]),
        ((24, 8), [255, 0, 0, 0]),
        ((8, 24), [0, 0, 0, 255]),
        ((24, 24), [128, 64, 32, 16]),
    ];

    fn sample_at(raw: &RawCmyk, x: u32, y: u32) -> [u8; 4] {
        let idx = (y as usize * raw.width as usize + x as usize) * 4;
        [
            raw.samples[idx],
            raw.samples[idx + 1],
            raw.samples[idx + 2],
            raw.samples[idx + 3],
        ]
    }

    #[test]
    fn adobe_transform_reads_fixture_marker() {
        assert_eq!(adobe_transform(ADOBE_CMYK_JPEG), Some(0));
    }

    #[test]
    fn adobe_transform_absent_without_app14() {
        // A minimal JPEG-ish stream (SOI, an APP0/JFIF segment, then SOS) with
        // no Adobe APP14 marker must report no transform.
        let bytes = [
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, // APP0, len 4
            0xFF, 0xDA, 0x00, 0x02, // SOS
        ];
        assert_eq!(adobe_transform(&bytes), None);
    }

    #[test]
    fn decodes_adobe_cmyk_jpeg_inverted_to_device_direction() {
        let raw = decode_cmyk_jpeg(ADOBE_CMYK_JPEG, DEFAULT_MAX_DECODE_PIXELS)
            .unwrap()
            .expect("an Adobe CMYK JPEG should decode to raw CMYK samples");
        assert_eq!((raw.width, raw.height), (32, 32));
        assert_eq!(raw.icc, None);

        // After undoing the Adobe inversion, the tile centres land back on the
        // device inks PIL stores (0 = no ink). A missing / wrong inversion would
        // read as 255 - ink and blow past this tolerance.
        for ((x, y), expected) in TILE_CENTRES {
            let got = sample_at(&raw, x, y);
            for ch in 0..4 {
                assert!(
                    (i32::from(got[ch]) - i32::from(expected[ch])).abs() <= 4,
                    "tile ({x},{y}) ch {ch}: {} vs {} (device ink)",
                    got[ch],
                    expected[ch]
                );
            }
        }
    }

    #[test]
    fn adobe_cmyk_jpeg_transforms_to_pil_rgb() {
        use crate::studio::cmyk_transform::cmyk_to_rgb8;

        let raw = decode_cmyk_jpeg(ADOBE_CMYK_JPEG, DEFAULT_MAX_DECODE_PIXELS)
            .unwrap()
            .expect("an Adobe CMYK JPEG should decode");
        let rgb = cmyk_to_rgb8(&raw);
        assert_eq!(rgb.len(), 32 * 32 * 3);

        // sRGB that Pillow's `Image.open(fixture).convert("RGB")` produces at the
        // same tile centres (naive path; the fixture carries no ICC). The cross-
        // decoder IDCT drift is bounded, so compare within a small tolerance --
        // an inversion-direction bug fails this by a wide margin.
        let expected: [((u32, u32), [u8; 3]); 4] = [
            ((8, 8), [255, 255, 255]),
            ((24, 8), [0, 255, 255]),
            ((8, 24), [0, 0, 0]),
            ((24, 24), [119, 179, 209]),
        ];
        for ((x, y), want) in expected {
            let idx = (y as usize * 32 + x as usize) * 3;
            for ch in 0..3 {
                let got = i32::from(rgb[idx + ch]);
                assert!(
                    (got - i32::from(want[ch])).abs() <= 6,
                    "tile ({x},{y}) ch {ch}: rust {got} vs PIL {}",
                    want[ch]
                );
            }
        }
    }

    #[test]
    fn adobe_transform_reads_ycck_fixture_marker() {
        assert_eq!(adobe_transform(YCCK_CMYK_JPEG), Some(2));
    }

    #[test]
    fn cmyk_family_jpeg_detection() {
        // Both Adobe shapes are CMYK-family (the `image` crate hides this by
        // reporting them as RGB), so the probe reclassifies them to CMYK.
        assert!(is_cmyk_family_jpeg(ADOBE_CMYK_JPEG));
        assert!(is_cmyk_family_jpeg(YCCK_CMYK_JPEG));
        // A plain baseline RGB (YCbCr) JPEG is not, and neither is a non-JPEG.
        let mut rgb_jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut rgb_jpeg)
            .encode(&[10, 20, 30, 40, 50, 60], 2, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        assert!(!is_cmyk_family_jpeg(&rgb_jpeg));
        assert!(!is_cmyk_family_jpeg(b"not a jpeg"));
    }

    #[test]
    fn decodes_ycck_jpeg_to_device_direction() {
        let raw = decode_cmyk_jpeg(YCCK_CMYK_JPEG, DEFAULT_MAX_DECODE_PIXELS)
            .unwrap()
            .expect("an Adobe YCCK JPEG should decode to raw CMYK samples");
        assert_eq!((raw.width, raw.height), (32, 32));

        // The YCCK->CMYK reconstruction plus the inversion undo lands the tile
        // centres back on the device inks (0 = no ink). A wrong reconstruction
        // or inversion direction blows past this tolerance. YCbCr chroma rounding
        // is a touch looser than the pure-CMYK path, so allow +/-6.
        for ((x, y), expected) in TILE_CENTRES {
            let got = sample_at(&raw, x, y);
            for ch in 0..4 {
                assert!(
                    (i32::from(got[ch]) - i32::from(expected[ch])).abs() <= 6,
                    "tile ({x},{y}) ch {ch}: {} vs {} (device ink)",
                    got[ch],
                    expected[ch]
                );
            }
        }
    }

    #[test]
    fn ycck_jpeg_transforms_to_pil_rgb() {
        use crate::studio::cmyk_transform::cmyk_to_rgb8;

        let raw = decode_cmyk_jpeg(YCCK_CMYK_JPEG, DEFAULT_MAX_DECODE_PIXELS)
            .unwrap()
            .expect("an Adobe YCCK JPEG should decode");
        let rgb = cmyk_to_rgb8(&raw);
        assert_eq!(rgb.len(), 32 * 32 * 3);

        // sRGB that Pillow's `Image.open(fixture).convert("RGB")` produces at the
        // same tile centres (naive path; the fixture carries no ICC). Compared
        // within a small tolerance -- a YCCK reconstruction or inversion bug
        // fails this by a wide margin.
        let expected: [((u32, u32), [u8; 3]); 4] = [
            ((8, 8), [255, 255, 255]),
            ((24, 8), [1, 255, 255]),
            ((8, 24), [0, 0, 0]),
            ((24, 24), [119, 180, 210]),
        ];
        for ((x, y), want) in expected {
            let idx = (y as usize * 32 + x as usize) * 3;
            for ch in 0..3 {
                let got = i32::from(rgb[idx + ch]);
                assert!(
                    (got - i32::from(want[ch])).abs() <= 6,
                    "tile ({x},{y}) ch {ch}: rust {got} vs PIL {}",
                    want[ch]
                );
            }
        }
    }
}
