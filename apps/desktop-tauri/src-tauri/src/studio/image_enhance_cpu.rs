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
//! It deliberately handles only the common 8-bit RGB/RGBA/grey inputs. CMYK,
//! high-bit / float, and ICC-tagged inputs need colour-managed conversion the
//! Python path does faithfully; [`try_enhance`] detects those (and any decode
//! failure) and returns `Ok(None)` so the caller defers to `psd::enhance_image`
//! rather than degrading the result.

use std::fs;
use std::path::Path;
use std::time::Instant;

use image::imageops::{self, FilterType};
use image::{ExtendedColorType, GrayImage, Luma, Rgb, RgbImage, Rgba, RgbaImage};

use super::studio_image::{self, SourceProbe, DEFAULT_MAX_DECODE_PIXELS};
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
/// in-process (CMYK / high-bit / ICC / decode failure) and the caller should
/// defer to the colour-managed Python bridge.
pub(super) fn try_enhance(p: &CpuEnhanceParams) -> Result<Option<EnhanceImageResult>, String> {
    let path = Path::new(&p.image_path);
    if !path.is_file() {
        // Let the Python path surface the canonical "base image not found".
        return Ok(None);
    }

    // Only fast-path colour spaces we can reproduce without a colour-managed
    // conversion; everything else defers to Python.
    match studio_image::probe_source(path) {
        Ok(probe) if can_handle_in_process(&probe) => {}
        _ => return Ok(None),
    }

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

    let loaded = match studio_image::load_rgba(path, DEFAULT_MAX_DECODE_PIXELS) {
        Ok(loaded) => loaded,
        // A decode failure (or oversized guard) is authoritative on the Python
        // path too; defer so the user sees its canonical message.
        Err(_) => return Ok(None),
    };
    let rgba = loaded.image;
    let (src_w, src_h) = rgba.dimensions();
    if src_w == 0 || src_h == 0 {
        return Ok(None);
    }

    let (rgb, alpha) = split_rgba(&rgba);

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
    out_img
        .save(&out_path)
        .map_err(|err| format!("failed to write {}: {err}", out_path.display()))?;

    let elapsed_ms = started.elapsed().as_millis() as i64;
    let scale_factor = round4(f64::from(out_w) / f64::from(src_w));
    let target_dpi = p.target_dpi.max(1) as u32;

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

/// Whether the CPU fast path can faithfully process an input without a
/// colour-managed conversion. CMYK, high-bit / float, and ICC-tagged inputs
/// defer to the Python pipeline.
fn can_handle_in_process(probe: &SourceProbe) -> bool {
    use ExtendedColorType::*;
    if probe.has_icc {
        return false;
    }
    !matches!(
        probe.color,
        Cmyk8 | L16 | La16 | Rgb16 | Rgba16 | Rgb32F | Rgba32F
    )
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
    fn cmyk_source_defers_to_python() {
        // A CMYK JPEG carries colour the naive in-process path would shift.
        let probe = SourceProbe {
            color: ExtendedColorType::Cmyk8,
            has_icc: false,
        };
        assert!(!can_handle_in_process(&probe));
        let icc = SourceProbe {
            color: ExtendedColorType::Rgb8,
            has_icc: true,
        };
        assert!(!can_handle_in_process(&icc));
        let plain = SourceProbe {
            color: ExtendedColorType::Rgba8,
            has_icc: false,
        };
        assert!(can_handle_in_process(&plain));
    }
}
