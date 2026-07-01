//! In-process CPU image-enhancement fast path (R3 Phase 1): a native-Rust
//! replica of `image_enhance_cli.py`'s `--engine cpu` pipeline, run inline in
//! the `imageEnhance` executor instead of spawning the Python bridge.
//!
//! It reproduces the CLI's CPU algorithm — edge-preserving median denoise on
//! the small image, Lanczos3 upscale / box(-ish) downscale, unsharp-mask
//! sharpen, and alpha kept on its own resize track — and emits an identical
//! [`EnhanceReport`] (same field names / semantics), so the node's outputs and
//! downstream consumers are unchanged.
//!
//! Colour management (R3 Phase 2 + CMYK c3): the fast path now also handles
//! high-bit single-channel inputs (`I;16`-style scans are range-scaled to 8-bit
//! by peak, matching the Python `numpy` path) and preserves an embedded ICC
//! profile onto the output PNG for same-colour-model inputs (RGB/RGBA/L/LA),
//! just like the Python bridge.
//!
//! CMYK is handled in-process for **TIFF** sources and **Adobe CMYK / YCCK
//! JPEGs** (an APP14 marker with transform 0 or 2): the raw ink samples and
//! embedded profile are read via [`super::cmyk_decode`] (which undoes the Adobe
//! inversion so JPEG and TIFF samples share the 0 = no ink convention, and for
//! YCCK reconstructs CMYK from the raw planes rather than taking zune's lossy
//! YCCK->RGB that drops the ICC) and colour-managed to sRGB via
//! [`super::cmyk_transform`] (the CMYK profile's A2B LUT, or PIL's naive formula
//! when untagged), matching `image_enhance_cli.py`'s `_cmyk_to_rgb`. CMYK JPEGs
//! without an Adobe marker still defer to `psd::enhance_image` (too rare to
//! validate). Float inputs also still defer (no well-defined 8-bit mapping to
//! reproduce here). For all of those, and on any decode failure, [`try_enhance`]
//! returns `Ok(None)` and the caller falls back to Python.

use std::borrow::Cow;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;
use std::time::Instant;

use image::imageops::{self, FilterType};
use image::{
    ExtendedColorType, GrayImage, ImageBuffer, Luma, Rgb, RgbImage, Rgba, RgbaImage,
};

use super::studio_image::{self, DEFAULT_MAX_DECODE_PIXELS};
use crate::psd::{reject_unsafe_output_name, EnhanceImageResult, EnhanceReport};

/// Resolved node parameters for one enhance run, mirroring the CLI arguments.
pub(super) struct CpuEnhanceParams {
    pub(super) image_path: String,
    pub(super) output_dir: String,
    pub(super) output_name: Option<String>,
    pub(super) mode: Option<String>,
    pub(super) target_bounds: Option<String>,
    pub(super) target_width: i64,
    pub(super) target_height: i64,
    pub(super) target_dpi: i64,
    pub(super) max_pixels: i64,
    pub(super) scale: f64,
    pub(super) denoise_strength: f64,
    pub(super) texture_strength: f64,
    pub(super) preserve_text_logo: bool,
    pub(super) device_requested: String,
    pub(super) precision_requested: String,
}

/// Run the CPU enhance pipeline in-process. Returns `Ok(Some(result))` on the
/// fast path, or `Ok(None)` when the input cannot be reproduced faithfully
/// in-process (an unmarked CMYK JPEG or float source, or any decode failure)
/// and the caller should defer to the colour-managed Python bridge.
pub(super) fn try_enhance(p: &CpuEnhanceParams) -> Result<Option<EnhanceImageResult>, String> {
    let path = Path::new(&p.image_path);
    if !path.is_file() {
        // Let the Python path surface the canonical "base image not found".
        return Ok(None);
    }

    // Inspect the source colour space (header only). Float still defers, as do
    // the CMYK JPEGs `prepare_source` won't take (unmarked); everything else,
    // including CMYK TIFF and Adobe CMYK / YCCK JPEG, is processed in-process. An
    // embedded ICC profile is carried onto the output only when the colour model
    // is unchanged (RGB/RGBA/L/LA), mirroring the Python path -- a CMYK/high-bit
    // conversion produces sRGB the old profile no longer describes.
    let probe = match studio_image::probe_source(path) {
        Ok(probe) if can_handle_in_process(probe.color) => probe,
        _ => return Ok(None),
    };
    let source_mode = studio_image::source_mode_label(probe.color);
    let icc_profile = if matches!(source_mode.as_str(), "RGB" | "RGBA" | "L" | "LA") {
        probe.icc.clone()
    } else {
        None
    };

    let mode_str = p
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("conservative");
    let (denoise_strength, texture_pref, fallback_scale) = match mode_str {
        "conservative" => (0.3_f64, 0.25_f64, 2.0_f64),
        "texture_rebuild" => (0.15, 0.7, 2.0),
        "print_ready" => (0.2, 0.5, 2.0),
        "custom" => (
            clip01(p.denoise_strength),
            clip01(p.texture_strength),
            p.scale.max(0.01),
        ),
        // Unknown mode: let Python raise the canonical error message.
        _ => return Ok(None),
    };
    let denoise_strength = clip01(denoise_strength);
    let mut texture_strength = clip01(texture_pref);
    if p.preserve_text_logo {
        texture_strength = texture_strength.min(0.4);
    }

    let mut target_w = p.target_width.max(0);
    let mut target_h = p.target_height.max(0);
    if target_w <= 0 && target_h <= 0 {
        let (bw, bh) = target_from_bounds(p.target_bounds.as_deref().unwrap_or(""));
        target_w = bw;
        target_h = bh;
    }
    let max_pixels = p.max_pixels.max(0);

    // Validate the output name up front (same guard as the Python command)
    // before spending time decoding.
    reject_unsafe_output_name(p.output_name.as_deref().unwrap_or(""))?;

    let started = Instant::now();

    let (rgb, alpha) = match prepare_source(path, probe.color)? {
        Some(pair) => pair,
        // A decode failure (or oversized guard) is authoritative on the Python
        // path too; defer so the user sees its canonical message.
        None => return Ok(None),
    };
    let (src_w, src_h) = rgb.dimensions();
    if src_w == 0 || src_h == 0 {
        return Ok(None);
    }

    let (scale, clamped) = resolve_scale(
        src_w,
        src_h,
        target_w,
        target_h,
        fallback_scale,
        max_pixels,
    );
    let out_w = (f64::from(src_w) * scale).round().max(1.0) as u32;
    let out_h = (f64::from(src_h) * scale).round().max(1.0) as u32;
    let downscaling = out_w < src_w || out_h < src_h;

    // CPU pipeline (colour channels only): denoise the small image, resample,
    // then sharpen so restored detail lands on the final grid. Skip the unsharp
    // pass when downscaling -- it would only amplify resampling artefacts.
    let denoised = denoise(&rgb, denoise_strength as f32);
    let resized = resample_rgb(&denoised, out_w, out_h, downscaling);
    let applied_texture = if downscaling { 0.0 } else { texture_strength };
    let sharpened = sharpen(&resized, applied_texture as f32);

    // The alpha rides its own resize track so the matte edge never picks up a
    // denoise / sharpen halo.
    let alpha_resized = resample_gray(&alpha, out_w, out_h, downscaling);
    let out_img = combine_rgba(&sharpened, &alpha_resized);

    let directory = Path::new(&p.output_dir);
    fs::create_dir_all(directory)
        .map_err(|err| format!("failed to create output dir {}: {err}", directory.display()))?;
    let stem = output_stem(p.output_name.as_deref(), &p.image_path);
    let out_path = directory.join(format!("{stem}.png"));
    let target_dpi = p.target_dpi.max(1) as u32;
    write_output_png(&out_path, &out_img, icc_profile.as_deref(), target_dpi)?;

    let elapsed_ms = started.elapsed().as_millis() as i64;
    let scale_factor = round4(f64::from(out_w) / f64::from(src_w));

    let report = EnhanceReport {
        mode: mode_str.to_string(),
        scale_factor,
        source_size: Some([i64::from(src_w), i64::from(src_h)]),
        output_size: Some([i64::from(out_w), i64::from(out_h)]),
        target_size: if target_w > 0 || target_h > 0 {
            Some([target_w, target_h])
        } else {
            None
        },
        target_dpi,
        max_pixels,
        clamped,
        denoise_strength: round4(denoise_strength),
        texture_strength: round4(applied_texture),
        preserve_text_logo: p.preserve_text_logo,
        engine: "cpu".to_string(),
        engine_requested: "cpu".to_string(),
        engine_fallback_reason: None,
        backend_model: None,
        device: None,
        device_requested: p.device_requested.clone(),
        precision: None,
        precision_requested: p.precision_requested.clone(),
        processing_time_ms: elapsed_ms,
    };

    Ok(Some(EnhanceImageResult {
        enhanced_image: out_path.to_string_lossy().to_string(),
        scale_factor,
        enhance_report: report,
    }))
}

/// Whether the CPU fast path can faithfully process an input in-process. 8-bit
/// and 16-bit RGB/RGBA/L/LA (including ICC-tagged) are handled; only CMYK and
/// float defer, because a faithful CMYK conversion needs the raw pre-RGB
/// samples the `image` crate discards at decode and a float source has no
/// well-defined 8-bit mapping the Python `numpy` path reproduces here.
fn can_handle_in_process(color: ExtendedColorType) -> bool {
    use ExtendedColorType::*;
    // CMYK is admitted here, but only CMYK TIFF and Adobe CMYK / YCCK JPEG
    // actually take the Rust path (see `prepare_source`); other CMYK JPEGs fall
    // back inside that step. Float sources have no faithful in-process mapping
    // and still defer.
    !matches!(color, Rgb32F | Rgba32F)
}

/// A single-channel high-bit source (`image`'s `L16`, i.e. PIL's `I;16`) is
/// range-scaled to 8-bit by its own peak, matching Python's `_highbit_to_rgb`.
/// Multi-channel 16-bit (`Rgb16`/`Rgba16`) instead takes the high byte, which
/// is exactly what both PIL and `into_rgba8` do, so it rides the generic path.
fn is_single_channel_highbit(color: ExtendedColorType) -> bool {
    matches!(color, ExtendedColorType::L16)
}

/// Decode the source into an 8-bit working RGB image plus its alpha track,
/// applying the colour-space-specific conversion. Returns `Ok(None)` when the
/// decode fails (or the guard trips) so the caller defers to Python.
fn prepare_source(
    path: &Path,
    color: ExtendedColorType,
) -> Result<Option<(RgbImage, GrayImage)>, String> {
    if matches!(color, ExtendedColorType::Cmyk8) {
        // CMYK TIFF and Adobe CMYK / YCCK JPEG are reproduced in-process;
        // `decode_cmyk` returns `Ok(None)` for the sources it won't take
        // faithfully (an unmarked CMYK JPEG), deferring to the Python bridge.
        let raw = match super::cmyk_decode::decode_cmyk(path, DEFAULT_MAX_DECODE_PIXELS) {
            Ok(Some(raw)) => raw,
            _ => return Ok(None),
        };
        if raw.width == 0 || raw.height == 0 {
            return Ok(None);
        }
        let rgb_bytes = super::cmyk_transform::cmyk_to_rgb8(&raw);
        let rgb = match RgbImage::from_raw(raw.width, raw.height, rgb_bytes) {
            Some(img) => img,
            None => return Ok(None),
        };
        // CMYK carries no alpha channel; ride a fully-opaque track.
        let alpha = GrayImage::from_pixel(raw.width, raw.height, Luma([255]));
        return Ok(Some((rgb, alpha)));
    }

    if is_single_channel_highbit(color) {
        let (dynimg, _meta, _icc) = match studio_image::load_dynamic(path, DEFAULT_MAX_DECODE_PIXELS) {
            Ok(loaded) => loaded,
            Err(_) => return Ok(None),
        };
        let gray16 = dynimg.into_luma16();
        let (w, h) = gray16.dimensions();
        if w == 0 || h == 0 {
            return Ok(None);
        }
        let rgb = highbit_gray_to_rgb(&gray16);
        // No alpha channel in a high-bit grey source; ride a fully-opaque track.
        let alpha = GrayImage::from_pixel(w, h, Luma([255]));
        return Ok(Some((rgb, alpha)));
    }

    let loaded = match studio_image::load_rgba(path, DEFAULT_MAX_DECODE_PIXELS) {
        Ok(loaded) => loaded,
        Err(_) => return Ok(None),
    };
    let rgba = loaded.image;
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return Ok(None);
    }
    Ok(Some(split_rgba(&rgba)))
}

/// Normalise a high-bit single-channel image down to 8-bit grey replicated to
/// RGB. Mirrors Python's `_highbit_to_rgb`: scale by the actual peak (so a
/// low-key 16-bit scan keeps its tonal range instead of being crushed by a
/// naive `>> 8`), then truncate to 8-bit exactly as `numpy.astype(uint8)` does.
fn highbit_gray_to_rgb(gray: &ImageBuffer<Luma<u16>, Vec<u16>>) -> RgbImage {
    let (w, h) = gray.dimensions();
    let peak = gray.pixels().map(|p| p.0[0]).max().unwrap_or(0) as f64;
    let scale = if peak > 255.0 { 255.0 / peak } else { 1.0 };
    let mut out = RgbImage::new(w, h);
    for (x, y, px) in gray.enumerate_pixels() {
        let v = (f64::from(px.0[0]) * scale).clamp(0.0, 255.0) as u8;
        out.put_pixel(x, y, Rgb([v, v, v]));
    }
    out
}

/// Write the output PNG, embedding the preserved ICC profile (when present) and
/// the target DPI as a `pHYs` chunk, matching the Python bridge's `save`.
fn write_output_png(
    path: &Path,
    img: &RgbaImage,
    icc: Option<&[u8]>,
    dpi: u32,
) -> Result<(), String> {
    let file = File::create(path)
        .map_err(|err| format!("failed to create {}: {err}", path.display()))?;
    let writer = BufWriter::new(file);
    let (width, height) = img.dimensions();
    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    if let Some(icc) = icc {
        encoder.set_icc_profile(Cow::Owned(icc.to_vec()));
    }
    // PNG stores physical resolution in pixels-per-metre; 1 inch = 0.0254 m.
    let ppu = (f64::from(dpi.max(1)) / 0.0254).round().max(1.0) as u32;
    encoder.set_pixel_dims(Some(png::PixelDimensions {
        xppu: ppu,
        yppu: ppu,
        unit: png::Unit::Meter,
    }));
    let mut png_writer = encoder
        .write_header()
        .map_err(|err| format!("failed to write PNG header {}: {err}", path.display()))?;
    png_writer
        .write_image_data(img.as_raw())
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(())
}

fn clip01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// Parse `{x, y, width, height}` placeholder bounds; `(0, 0)` when absent or
/// unparseable so the caller falls back to the preset scale.
fn target_from_bounds(bounds_json: &str) -> (i64, i64) {
    let text = bounds_json.trim();
    if text.is_empty() {
        return (0, 0);
    }
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_) => return (0, 0),
    };
    if !value.is_object() {
        return (0, 0);
    }
    let read = |key: &str| -> i64 {
        value
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
            .round()
            .max(0.0) as i64
    };
    (read("width"), read("height"))
}

/// Pick a uniform upscale factor and whether it was clamped by `max_pixels`.
fn resolve_scale(
    src_w: u32,
    src_h: u32,
    target_w: i64,
    target_h: i64,
    fallback_scale: f64,
    max_pixels: i64,
) -> (f64, bool) {
    let mut scale = if target_w > 0 || target_h > 0 {
        let mut best = f64::MIN;
        if target_w > 0 {
            best = best.max(target_w as f64 / f64::from(src_w));
        }
        if target_h > 0 {
            best = best.max(target_h as f64 / f64::from(src_h));
        }
        best
    } else {
        fallback_scale.max(0.01)
    };

    let mut clamped = false;
    if max_pixels > 0 {
        let out_pixels = (f64::from(src_w) * scale) * (f64::from(src_h) * scale);
        if out_pixels > max_pixels as f64 {
            scale *= (max_pixels as f64 / out_pixels).sqrt();
            clamped = true;
        }
    }
    (scale, clamped)
}

fn split_rgba(img: &RgbaImage) -> (RgbImage, GrayImage) {
    let (w, h) = img.dimensions();
    let mut rgb = RgbImage::new(w, h);
    let mut alpha = GrayImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels() {
        let [r, g, b, a] = px.0;
        rgb.put_pixel(x, y, Rgb([r, g, b]));
        alpha.put_pixel(x, y, Luma([a]));
    }
    (rgb, alpha)
}

fn combine_rgba(rgb: &RgbImage, alpha: &GrayImage) -> RgbaImage {
    let (w, h) = rgb.dimensions();
    let mut out = RgbaImage::new(w, h);
    for (x, y, px) in rgb.enumerate_pixels() {
        let [r, g, b] = px.0;
        let a = alpha.get_pixel(x, y).0[0];
        out.put_pixel(x, y, Rgba([r, g, b, a]));
    }
    out
}

/// Edge-preserving denoise: blend a 3x3 median-filtered copy back in by
/// `strength`.
fn denoise(img: &RgbImage, strength: f32) -> RgbImage {
    if strength <= 0.0 {
        return img.clone();
    }
    let cleaned = median3x3(img);
    blend(img, &cleaned, strength.clamp(0.0, 1.0))
}

fn median3x3(img: &RgbImage) -> RgbImage {
    let (w, h) = img.dimensions();
    let mut out = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut rs = [0u8; 9];
            let mut gs = [0u8; 9];
            let mut bs = [0u8; 9];
            let mut i = 0;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let nx = (x as i32 + dx).clamp(0, w as i32 - 1) as u32;
                    let ny = (y as i32 + dy).clamp(0, h as i32 - 1) as u32;
                    let p = img.get_pixel(nx, ny).0;
                    rs[i] = p[0];
                    gs[i] = p[1];
                    bs[i] = p[2];
                    i += 1;
                }
            }
            rs.sort_unstable();
            gs.sort_unstable();
            bs.sort_unstable();
            out.put_pixel(x, y, Rgb([rs[4], gs[4], bs[4]]));
        }
    }
    out
}

fn blend(a: &RgbImage, b: &RgbImage, s: f32) -> RgbImage {
    let (w, h) = a.dimensions();
    let mut out = RgbImage::new(w, h);
    for (x, y, pa) in a.enumerate_pixels() {
        let pb = b.get_pixel(x, y).0;
        let mut v = [0u8; 3];
        for c in 0..3 {
            let val = f32::from(pa.0[c]) * (1.0 - s) + f32::from(pb[c]) * s;
            v[c] = val.round().clamp(0.0, 255.0) as u8;
        }
        out.put_pixel(x, y, Rgb(v));
    }
    out
}

fn resample_rgb(img: &RgbImage, out_w: u32, out_h: u32, downscaling: bool) -> RgbImage {
    if (out_w, out_h) == img.dimensions() {
        return img.clone();
    }
    let filter = if downscaling {
        FilterType::Triangle
    } else {
        FilterType::Lanczos3
    };
    imageops::resize(img, out_w, out_h, filter)
}

fn resample_gray(img: &GrayImage, out_w: u32, out_h: u32, downscaling: bool) -> GrayImage {
    if (out_w, out_h) == img.dimensions() {
        return img.clone();
    }
    let filter = if downscaling {
        FilterType::Triangle
    } else {
        FilterType::Lanczos3
    };
    imageops::resize(img, out_w, out_h, filter)
}

/// Restore high-frequency detail via an unsharp mask (PIL `UnsharpMask`,
/// radius 2.0, percent = strength*150, threshold 2).
fn sharpen(img: &RgbImage, strength: f32) -> RgbImage {
    if strength <= 0.0 {
        return img.clone();
    }
    let percent = (strength.clamp(0.0, 1.0) * 150.0).round();
    let amount = percent / 100.0;
    unsharp(img, 2.0, amount, 2)
}

fn unsharp(img: &RgbImage, sigma: f32, amount: f32, threshold: i32) -> RgbImage {
    let blurred = imageops::blur(img, sigma);
    let (w, h) = img.dimensions();
    let mut out = RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels() {
        let bp = blurred.get_pixel(x, y).0;
        let mut v = [0u8; 3];
        for c in 0..3 {
            let orig = i32::from(px.0[c]);
            let diff = orig - i32::from(bp[c]);
            let nv = if diff.abs() > threshold {
                orig as f32 + amount * diff as f32
            } else {
                orig as f32
            };
            v[c] = nv.round().clamp(0.0, 255.0) as u8;
        }
        out.put_pixel(x, y, Rgb(v));
    }
    out
}

/// The output PNG base name: an explicit (already-validated) `output_name`, or
/// a sanitised `<image-stem>_enhanced`.
fn output_stem(output_name: Option<&str>, image_path: &str) -> String {
    if let Some(name) = output_name.map(str::trim).filter(|n| !n.is_empty()) {
        return name.to_string();
    }
    let stem = Path::new(image_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let base = if cleaned.is_empty() {
        "image".to_string()
    } else {
        cleaned
    };
    format!("{base}_enhanced")
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
        std::env::temp_dir().join(format!("hgripe_enhance_cpu_{nanos}_{name}"))
    }

    fn params(image: &str, out_dir: &str) -> CpuEnhanceParams {
        CpuEnhanceParams {
            image_path: image.to_string(),
            output_dir: out_dir.to_string(),
            output_name: None,
            mode: Some("conservative".to_string()),
            target_bounds: None,
            target_width: 0,
            target_height: 0,
            target_dpi: 300,
            max_pixels: 48_000_000,
            scale: 2.0,
            denoise_strength: 0.3,
            texture_strength: 0.25,
            preserve_text_logo: true,
            device_requested: "auto".to_string(),
            precision_requested: "auto".to_string(),
        }
    }

    #[test]
    fn missing_file_defers_to_python() {
        let p = params("does-not-exist.png", ".");
        assert!(try_enhance(&p).unwrap().is_none());
    }

    #[test]
    fn preset_scale_doubles_and_reports_parity() {
        let dir = unique_tmp("preset");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.png");
        RgbaImage::from_pixel(10, 8, Rgba([120, 60, 30, 255]))
            .save(&src)
            .unwrap();

        let p = params(src.to_str().unwrap(), dir.to_str().unwrap());
        let result = try_enhance(&p).unwrap().expect("cpu fast path");

        let report = &result.enhance_report;
        assert_eq!(report.source_size, Some([10, 8]));
        assert_eq!(report.output_size, Some([20, 16]));
        assert_eq!(report.scale_factor, 2.0);
        assert_eq!(report.mode, "conservative");
        assert_eq!(report.engine, "cpu");
        assert!(report.target_size.is_none());
        assert!(!report.clamped);
        // preserve_text_logo caps the conservative 0.25 texture below its cap.
        assert_eq!(report.texture_strength, 0.25);
        assert!(Path::new(&result.enhanced_image).is_file());

        let out = image::open(&result.enhanced_image).unwrap().to_rgba8();
        assert_eq!(out.dimensions(), (20, 16));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn target_bounds_drive_scale_and_size() {
        let dir = unique_tmp("bounds");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.png");
        RgbaImage::from_pixel(20, 20, Rgba([200, 200, 200, 255]))
            .save(&src)
            .unwrap();

        let mut p = params(src.to_str().unwrap(), dir.to_str().unwrap());
        p.target_bounds = Some(r#"{"x":0,"y":0,"width":60,"height":40}"#.to_string());
        let result = try_enhance(&p).unwrap().expect("cpu fast path");

        // Covers the target: max(60/20, 40/20) = 3.0 -> 60x60.
        assert_eq!(result.enhance_report.output_size, Some([60, 60]));
        assert_eq!(result.enhance_report.target_size, Some([60, 40]));
        assert_eq!(result.scale_factor, 3.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn max_pixels_clamps_scale() {
        let dir = unique_tmp("clamp");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.png");
        RgbaImage::from_pixel(100, 100, Rgba([10, 20, 30, 255]))
            .save(&src)
            .unwrap();

        let mut p = params(src.to_str().unwrap(), dir.to_str().unwrap());
        p.mode = Some("custom".to_string());
        p.scale = 4.0; // 400x400 = 160k px
        p.max_pixels = 40_000; // caps to 200x200
        let result = try_enhance(&p).unwrap().expect("cpu fast path");

        assert!(result.enhance_report.clamped);
        assert_eq!(result.enhance_report.output_size, Some([200, 200]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preserve_text_logo_caps_texture() {
        let dir = unique_tmp("cap");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.png");
        RgbaImage::from_pixel(8, 8, Rgba([90, 90, 90, 255]))
            .save(&src)
            .unwrap();

        let mut p = params(src.to_str().unwrap(), dir.to_str().unwrap());
        p.mode = Some("texture_rebuild".to_string()); // texture 0.7
        p.preserve_text_logo = true;
        let capped = try_enhance(&p).unwrap().unwrap();
        assert_eq!(capped.enhance_report.texture_strength, 0.4);

        p.preserve_text_logo = false;
        let uncapped = try_enhance(&p).unwrap().unwrap();
        assert_eq!(uncapped.enhance_report.texture_strength, 0.7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn colour_space_gating() {
        use ExtendedColorType::*;
        // CMYK is admitted here now (CMYK TIFF and Adobe CMYK / YCCK JPEG are
        // processed in-process; unmarked CMYK JPEGs defer inside `prepare_source`).
        // Only float still defers at the gate.
        assert!(can_handle_in_process(Cmyk8));
        assert!(!can_handle_in_process(Rgb32F));
        assert!(!can_handle_in_process(Rgba32F));
        // 8-bit and 16-bit RGB/RGBA/L/LA (ICC-tagged or not) are handled now.
        assert!(can_handle_in_process(Rgb8));
        assert!(can_handle_in_process(Rgba8));
        assert!(can_handle_in_process(L16));
        assert!(can_handle_in_process(Rgb16));
        assert!(can_handle_in_process(Rgba16));
        // Only the single-channel 16-bit source is range-scaled; multi-channel
        // 16-bit rides the generic high-byte path.
        assert!(is_single_channel_highbit(L16));
        assert!(!is_single_channel_highbit(Rgb16));
        assert!(!is_single_channel_highbit(La16));
    }

    #[test]
    fn cmyk_tiff_enhances_in_process() {
        use std::io::Cursor;
        use tiff::encoder::{colortype, TiffEncoder};

        let dir = unique_tmp("cmyk");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.tiff");
        // A flat 4x4 CMYK field, no embedded profile -> PIL's naive formula:
        // (128,64,32,16) -> (119,179,209). A flat field survives denoise /
        // Lanczos / unsharp unchanged, so the enhanced output must land on it.
        let (w, h) = (4u32, 4u32);
        let samples: Vec<u8> = (0..w * h).flat_map(|_| [128u8, 64, 32, 16]).collect();
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            enc.write_image::<colortype::CMYK8>(w, h, &samples).unwrap();
        }
        std::fs::write(&src, buf.into_inner()).unwrap();

        let result = try_enhance(&params(src.to_str().unwrap(), dir.to_str().unwrap()))
            .unwrap()
            .expect("CMYK TIFF should take the in-process path");
        let report = &result.enhance_report;
        assert_eq!(report.engine, "cpu");
        assert_eq!(report.source_size, Some([4, 4]));
        assert_eq!(report.output_size, Some([8, 8])); // conservative 2x
        assert!(Path::new(&result.enhanced_image).is_file());

        let out = image::open(&result.enhanced_image).unwrap().to_rgb8();
        let px = out.get_pixel(4, 4).0;
        assert!((i32::from(px[0]) - 119).abs() <= 12, "R {} vs 119", px[0]);
        assert!((i32::from(px[1]) - 179).abs() <= 12, "G {} vs 179", px[1]);
        assert!((i32::from(px[2]) - 209).abs() <= 12, "B {} vs 209", px[2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cmyk_jpeg_enhances_in_process() {
        // The same PIL-generated Adobe CMYK JPEG fixture the decode tests use:
        // routing it in-process (instead of deferring to Python) is the point of
        // this change. Decode/transform fidelity is asserted in `cmyk_decode`;
        // here we only prove the JPEG now takes the Rust path and inverts the
        // Adobe ink correctly (the no-ink corner reads near-white, not near-black).
        let jpeg: &[u8] = include_bytes!("../../tests/fixtures/cmyk_adobe_app14.jpg");

        let dir = unique_tmp("cmyk_jpeg");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.jpg");
        std::fs::write(&src, jpeg).unwrap();

        let result = try_enhance(&params(src.to_str().unwrap(), dir.to_str().unwrap()))
            .unwrap()
            .expect("an Adobe CMYK JPEG should take the in-process path");
        let report = &result.enhance_report;
        assert_eq!(report.engine, "cpu");
        assert_eq!(report.source_size, Some([32, 32]));
        assert_eq!(report.output_size, Some([64, 64])); // conservative 2x

        // Deep inside the no-ink (white) top-left tile after the 2x upscale.
        let out = image::open(&result.enhanced_image).unwrap().to_rgb8();
        let px = out.get_pixel(8, 8).0;
        assert!(
            px.iter().all(|&v| v >= 240),
            "no-ink corner must stay near-white, got {px:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ycck_jpeg_enhances_in_process() {
        // The Adobe YCCK JPEG fixture (APP14 transform 2). `image` decodes YCCK
        // to RGB and reports it as `Rgb8`, so without the probe's CMYK
        // reclassification this would silently take the generic path; here we
        // prove it reaches the Rust CMYK path and reconstructs the ink correctly
        // (no-ink corner near-white, full-cyan corner cyan). Reconstruction and
        // ICC-preservation fidelity are asserted in `cmyk_decode`.
        let jpeg: &[u8] = include_bytes!("../../tests/fixtures/cmyk_ycck_app14.jpg");

        let dir = unique_tmp("ycck_jpeg");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.jpg");
        std::fs::write(&src, jpeg).unwrap();

        let result = try_enhance(&params(src.to_str().unwrap(), dir.to_str().unwrap()))
            .unwrap()
            .expect("an Adobe YCCK JPEG should take the in-process path");
        let report = &result.enhance_report;
        assert_eq!(report.engine, "cpu");
        assert_eq!(report.source_size, Some([32, 32]));
        assert_eq!(report.output_size, Some([64, 64])); // conservative 2x

        let out = image::open(&result.enhanced_image).unwrap().to_rgb8();
        // No-ink top-left tile stays near-white after the 2x upscale.
        let white = out.get_pixel(8, 8).0;
        assert!(
            white.iter().all(|&v| v >= 240),
            "no-ink corner must stay near-white, got {white:?}"
        );
        // Full-cyan top-right tile: low red, high green/blue (a wrong YCCK
        // reconstruction or inversion collapses this).
        let cyan = out.get_pixel(48, 16).0;
        assert!(
            cyan[0] <= 40 && cyan[1] >= 200 && cyan[2] >= 200,
            "full-cyan corner must read cyan, got {cyan:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn highbit_gray_scales_by_peak() {
        // A low-key 16-bit scan (peak 60000) must keep its tonal range, not be
        // crushed by a naive >> 8; matches Python's numpy peak scaling + trunc.
        let mut gray = ImageBuffer::<Luma<u16>, Vec<u16>>::new(2, 2);
        gray.put_pixel(0, 0, Luma([0]));
        gray.put_pixel(1, 0, Luma([30_000]));
        gray.put_pixel(0, 1, Luma([60_000]));
        gray.put_pixel(1, 1, Luma([15_000]));
        let rgb = highbit_gray_to_rgb(&gray);
        // scale = 255/60000; values truncate toward zero like astype(uint8).
        assert_eq!(rgb.get_pixel(0, 0).0, [0, 0, 0]);
        assert_eq!(rgb.get_pixel(1, 0).0, [127, 127, 127]); // 30000*255/60000=127.5
        assert_eq!(rgb.get_pixel(0, 1).0, [255, 255, 255]);
        assert_eq!(rgb.get_pixel(1, 1).0, [63, 63, 63]); // 15000*255/60000=63.75
    }

    #[test]
    fn highbit_gray_below_255_peak_is_unscaled() {
        let mut gray = ImageBuffer::<Luma<u16>, Vec<u16>>::new(2, 1);
        gray.put_pixel(0, 0, Luma([200]));
        gray.put_pixel(1, 0, Luma([100]));
        let rgb = highbit_gray_to_rgb(&gray);
        assert_eq!(rgb.get_pixel(0, 0).0, [200, 200, 200]);
        assert_eq!(rgb.get_pixel(1, 0).0, [100, 100, 100]);
    }

    #[test]
    fn output_png_embeds_icc_and_dpi() {
        let dir = unique_tmp("icc");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("o.png");
        let icc = vec![9u8, 8, 7, 6, 5, 4, 3, 2, 1];
        let img = RgbaImage::from_pixel(3, 2, Rgba([10, 20, 30, 255]));
        write_output_png(&out, &img, Some(&icc), 300).unwrap();

        let decoder = png::Decoder::new(std::fs::File::open(&out).unwrap());
        let reader = decoder.read_info().unwrap();
        let info = reader.info();
        assert_eq!(
            info.icc_profile.as_deref().map(<[u8]>::to_vec),
            Some(icc.clone())
        );
        let dims = info.pixel_dims.expect("pHYs written");
        assert_eq!(dims.unit, png::Unit::Meter);
        // 300 dpi / 0.0254 m ~= 11811 ppu.
        assert!((11_810..=11_812).contains(&dims.xppu));
        assert_eq!(dims.xppu, dims.yppu);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn highbit_source_takes_fast_path() {
        let dir = unique_tmp("i16");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("scan.png");
        let mut gray = ImageBuffer::<Luma<u16>, Vec<u16>>::new(6, 4);
        for (i, px) in gray.pixels_mut().enumerate() {
            *px = Luma([(i as u16) * 2_000]);
        }
        image::DynamicImage::ImageLuma16(gray).save(&src).unwrap();

        let p = params(src.to_str().unwrap(), dir.to_str().unwrap());
        let result = try_enhance(&p).unwrap().expect("cpu fast path");
        assert_eq!(result.enhance_report.source_size, Some([6, 4]));
        assert_eq!(result.enhance_report.output_size, Some([12, 8]));
        assert!(Path::new(&result.enhanced_image).is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
