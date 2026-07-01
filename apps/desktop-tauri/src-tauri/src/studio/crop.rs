//! The `crop` node executor (the `Compute` lane, in-process native Rust).
//!
//! Crop is the first non-mask edit and validates the unified auto / manual +
//! binding model end-to-end (see `docs/cards/generic-media-card.md`):
//!
//! * **manual** — crop to the editor-drawn box (`crop_box` = `[x, y, w, h]` in
//!   image pixels), the human-spatial-intent lane.
//! * **auto_subject** — *crop to subject*: segment a base matte from the image
//!   with the same `subjectMask` `Compute`-lane segmenter, take its bounding
//!   box and pad it by `margin_pct`, the algorithm-derived lane.
//!
//! An optional `aspect` ratio adjusts the box (centred, clamped to the image)
//! after either lane. It emits the cropped image and a flat `crop_report`
//! mirroring the enriched-report convention used across the chain.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::GrayImage;
use serde::Serialize;
use serde_json::{json, Value};

use super::graph::{
    number_param, optional, studio_output_map, studio_value_to_string, StudioGraphNode,
};
use super::image_buffer;
use super::persist::studio_reject_unsafe_basename;
use super::pixel_ops;
use super::studio_image;
use super::subject_segment::{segmenter_for_mode, AutoMode, SegmentRequest};

/// A pixel counts towards the subject bounding box once it is at least
/// half-opaque (mirrors `subject_mask::SELECTED_THRESHOLD`).
const SELECTED_THRESHOLD: u8 = 128;

/// The flat enriched report mirrored onto the `crop_report` output port.
#[derive(Debug, Serialize)]
struct CropReport {
    mode: String,
    provider: String,
    source_mode: String,
    exif_transposed: bool,
    max_decode_pixels: u64,
    input_size: [u32; 2],
    output_size: [u32; 2],
    /// The applied crop box `[x, y, w, h]` in input-image pixels.
    crop_box: [u32; 4],
    aspect: String,
    margin_pct: f64,
    operations: Vec<Value>,
    processing_time_ms: u128,
}

fn param_or(node: &StudioGraphNode, key: &str, default: &str) -> String {
    match optional(studio_value_to_string(node.params.get(key))) {
        Some(value) => value,
        None => default.to_string(),
    }
}

fn image_stem(path: &str) -> String {
    Path::new(path.trim())
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".to_string())
}

pub(super) fn execute_studio_crop(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
    skip_write_ports: &std::collections::HashSet<String>,
) -> Result<BTreeMap<String, Value>, String> {
    let started = Instant::now();

    let image_path = studio_value_to_string(inputs.get("image"));
    if image_path.trim().is_empty() {
        return Err("Crop needs a connected image input".to_string());
    }

    let max_decode_pixels = {
        let configured = number_param(node, "max_decode_pixels", -1.0);
        if configured < 0.0 {
            studio_image::DEFAULT_MAX_DECODE_PIXELS
        } else {
            configured as u64
        }
    };

    // Crop is pure geometry, so it walks the 16-bit canonical surface: a
    // wide-gamut input (or an upstream manual card's published surface) is
    // cropped and re-emitted at full precision. Only the auto-subject
    // segmenter — a model ingress — sees the 8-bit sRGB egress.
    let loaded = studio_image::load_working(Path::new(image_path.trim()), max_decode_pixels)?;
    let image = loaded.image;
    let (width, height) = (image.width, image.height);
    if width == 0 || height == 0 {
        return Err("Crop needs a non-empty image".to_string());
    }

    let mode = param_or(node, "mode", "manual");
    let aspect = param_or(node, "aspect", "free");
    let margin_pct = number_param(node, "margin_pct", 0.0).clamp(0.0, 100.0);
    let mut operations: Vec<Value> = Vec::new();
    let mut provider = "rust-native".to_string();

    // The base box, before the optional aspect-ratio adjustment.
    let mut bbox = if mode == "auto_subject" {
        let auto = AutoMode::from_mode("auto_subject").unwrap_or(AutoMode::Subject);
        let segmenter = segmenter_for_mode(auto, &[]);
        let srgb = image.to_srgb_rgba8();
        let result = segmenter.segment(&SegmentRequest {
            image: &srgb,
            mode: auto,
            placeholder: None,
            prompt: None,
            points: &[],
        })?;
        provider = segmenter.provider().to_string();
        let base = mask_bbox(&result.mask)
            .ok_or_else(|| "Crop auto-subject found no subject in the image".to_string())?;
        operations.push(json!({
            "type": "auto_subject",
            "provider": provider,
            "subject_box": [base.0, base.1, base.2, base.3],
        }));
        pad_box(base, margin_pct, width, height)
    } else {
        parse_crop_box(node.params.get("crop_box"), width, height)
    };

    if aspect != "free" {
        if let Some(ratio) = parse_aspect(&aspect) {
            bbox = fit_aspect(bbox, ratio, width, height);
            operations.push(json!({ "type": "aspect", "ratio": aspect }));
        }
    }

    let (x, y, w, h) = clamp_box(bbox, width, height);
    if w == 0 || h == 0 {
        return Err("Crop box is empty after clamping to the image".to_string());
    }
    operations.push(json!({ "type": "crop", "box": [x, y, w, h] }));

    let cropped = pixel_ops::crop_working(&image, x, y, w, h);

    let output_dir = {
        let configured = studio_value_to_string(node.params.get("output_dir"));
        if configured.trim().is_empty() {
            crate::runtime_paths()?
                .output_dir
                .to_string_lossy()
                .to_string()
        } else {
            configured
        }
    };
    let base = {
        let configured = studio_value_to_string(node.params.get("output_name"));
        if configured.trim().is_empty() {
            format!("{}_crop", image_stem(&image_path))
        } else {
            configured.trim().to_string()
        }
    };
    studio_reject_unsafe_basename(&base)?;

    // The manual output carrier: `png` (default) or 16-bit `tiff`. Both encoders
    // honour the space tag identically — a wide-gamut ProPhoto surface lands as
    // 16-bit with the profile embedded, a plain Srgb surface as the exact 8-bit
    // narrow — so the choice is purely which container the manual pipeline wants.
    let ext = match param_or(node, "format", "png").as_str() {
        "tiff" => "tiff",
        _ => "png",
    };

    let dir = PathBuf::from(&output_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("failed to create output dir {}: {err}", dir.display()))?;
    let out_path = dir.join(format!("{base}.{ext}"));
    // When the `image` output feeds only other in-process compute cards, the
    // file is never read: the consumer loads the decoded surface straight from
    // the buffer. In that case skip the PNG encode+write and publish a
    // *deferred* buffer instead (materialised to `out_path` only if it is later
    // evicted, so any thumbnail / disk fallback still resolves). Otherwise write
    // the file and publish a file-backed buffer as before — preview, the
    // Python-bridge, export and API uploads all read the file. The skip is
    // suppressed if a file already exists at the path, so a stale output can
    // never linger behind the buffer.
    if skip_write_ports.contains("image") && !out_path.exists() {
        image_buffer::publish_working_deferred(
            &out_path,
            &cropped,
            studio_image::png_output_meta(),
        );
    } else {
        studio_image::write_working_output(&out_path, &cropped)?;
        // Hand the decoded crop to the next compute card in memory so it skips
        // the PNG re-decode; the file on disk stays the source of truth for
        // everyone else (preview, Python-bridge cards, export).
        image_buffer::publish_working(&out_path, &cropped, studio_image::png_output_meta());
    }

    let report = CropReport {
        mode,
        provider,
        source_mode: loaded.meta.source_mode.clone(),
        exif_transposed: loaded.meta.exif_transposed,
        max_decode_pixels,
        input_size: [width, height],
        output_size: [w, h],
        crop_box: [x, y, w, h],
        aspect,
        margin_pct,
        operations,
        processing_time_ms: started.elapsed().as_millis(),
    };
    let report = serde_json::to_value(&report)
        .map_err(|err| format!("failed to encode crop_report: {err}"))?;

    Ok(studio_output_map([
        ("image", json!(out_path.to_string_lossy())),
        ("crop_report", report),
    ]))
}

/// Tight bounding box `(x, y, w, h)` of the selected (≥ half-opaque) pixels of a
/// mask, or `None` when nothing is selected.
fn mask_bbox(mask: &GrayImage) -> Option<(u32, u32, u32, u32)> {
    let (width, height) = mask.dimensions();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (width, height, 0u32, 0u32);
    let mut found = false;
    for y in 0..height {
        for x in 0..width {
            if mask.get_pixel(x, y).0[0] >= SELECTED_THRESHOLD {
                found = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if found {
        Some((min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
    } else {
        None
    }
}

/// Read the editor-drawn `crop_box` param (`[x, y, w, h]` in image pixels),
/// falling back to the whole image when it is missing or malformed.
fn parse_crop_box(value: Option<&Value>, width: u32, height: u32) -> (i64, i64, i64, i64) {
    let full = (0i64, 0i64, i64::from(width), i64::from(height));
    let Some(Value::Array(items)) = value else {
        return full;
    };
    if items.len() != 4 {
        return full;
    }
    let n = |i: usize| {
        items
            .get(i)
            .and_then(Value::as_f64)
            .map(|v| v.round() as i64)
    };
    match (n(0), n(1), n(2), n(3)) {
        (Some(x), Some(y), Some(w), Some(h)) if w > 0 && h > 0 => (x, y, w, h),
        _ => full,
    }
}

/// Pad a box outward by `margin_pct` of its own width / height, used so the
/// auto subject crop keeps a little breathing room around the subject.
fn pad_box(
    bbox: (u32, u32, u32, u32),
    margin_pct: f64,
    _width: u32,
    _height: u32,
) -> (i64, i64, i64, i64) {
    let (x, y, w, h) = bbox;
    let pad_x = (f64::from(w) * margin_pct / 100.0).round() as i64;
    let pad_y = (f64::from(h) * margin_pct / 100.0).round() as i64;
    (
        i64::from(x) - pad_x,
        i64::from(y) - pad_y,
        i64::from(w) + 2 * pad_x,
        i64::from(h) + 2 * pad_y,
    )
}

/// Parse an `"a:b"` aspect ratio into `a / b`; `None` for malformed / zero.
fn parse_aspect(aspect: &str) -> Option<f64> {
    let (a, b) = aspect.split_once(':')?;
    let a: f64 = a.trim().parse().ok()?;
    let b: f64 = b.trim().parse().ok()?;
    if a > 0.0 && b > 0.0 {
        Some(a / b)
    } else {
        None
    }
}

/// Adjust a box to the target `ratio` (w / h) about its centre, shrinking the
/// over-long axis so the result still fits the original box, then clamped to
/// the image by the caller.
fn fit_aspect(
    bbox: (i64, i64, i64, i64),
    ratio: f64,
    _width: u32,
    _height: u32,
) -> (i64, i64, i64, i64) {
    let (x, y, w, h) = bbox;
    if w <= 0 || h <= 0 {
        return bbox;
    }
    let cx = x as f64 + w as f64 / 2.0;
    let cy = y as f64 + h as f64 / 2.0;
    let current = w as f64 / h as f64;
    let (new_w, new_h) = if current > ratio {
        // too wide: keep height, shrink width
        (h as f64 * ratio, h as f64)
    } else {
        // too tall: keep width, shrink height
        (w as f64, w as f64 / ratio)
    };
    (
        (cx - new_w / 2.0).round() as i64,
        (cy - new_h / 2.0).round() as i64,
        new_w.round() as i64,
        new_h.round() as i64,
    )
}

/// Clamp a (possibly out-of-bounds) box to the image, returning a valid
/// `(x, y, w, h)` in `u32`.
fn clamp_box(bbox: (i64, i64, i64, i64), width: u32, height: u32) -> (u32, u32, u32, u32) {
    let (bx, by, bw, bh) = bbox;
    let x0 = bx.clamp(0, i64::from(width));
    let y0 = by.clamp(0, i64::from(height));
    let x1 = (bx + bw.max(0)).clamp(0, i64::from(width));
    let y1 = (by + bh.max(0)).clamp(0, i64::from(height));
    (
        x0 as u32,
        y0 as u32,
        (x1 - x0).max(0) as u32,
        (y1 - y0).max(0) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, Rgba, RgbaImage};
    use serde_json::json;
    use std::collections::HashSet;

    fn solid_mask(w: u32, h: u32, box_: (u32, u32, u32, u32)) -> GrayImage {
        let mut mask = GrayImage::from_pixel(w, h, Luma([0]));
        let (bx, by, bw, bh) = box_;
        for y in by..by + bh {
            for x in bx..bx + bw {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        mask
    }

    #[test]
    fn bbox_of_a_solid_rectangle_is_that_rectangle() {
        let mask = solid_mask(20, 20, (4, 6, 8, 5));
        assert_eq!(mask_bbox(&mask), Some((4, 6, 8, 5)));
    }

    #[test]
    fn bbox_of_an_empty_mask_is_none() {
        let mask = GrayImage::from_pixel(10, 10, Luma([0]));
        assert_eq!(mask_bbox(&mask), None);
    }

    #[test]
    fn crop_box_param_round_trips_and_falls_back() {
        assert_eq!(
            parse_crop_box(Some(&json!([2, 3, 5, 7])), 100, 100),
            (2, 3, 5, 7)
        );
        // missing / malformed -> whole image
        assert_eq!(parse_crop_box(None, 40, 30), (0, 0, 40, 30));
        assert_eq!(
            parse_crop_box(Some(&json!([1, 2, 0, 7])), 40, 30),
            (0, 0, 40, 30)
        );
    }

    #[test]
    fn pad_box_grows_symmetrically() {
        // 10% of a 100x50 box = 10px / 5px each side
        assert_eq!(
            pad_box((20, 20, 100, 50), 10.0, 1000, 1000),
            (10, 15, 120, 60)
        );
    }

    #[test]
    fn fit_aspect_shrinks_the_over_long_axis() {
        // a 200x100 (2:1) box fit to 1:1 keeps height, shrinks width to 100,
        // re-centred about (100, 50).
        assert_eq!(
            fit_aspect((0, 0, 200, 100), 1.0, 1000, 1000),
            (50, 0, 100, 100)
        );
    }

    #[test]
    fn clamp_box_keeps_the_box_inside_the_image() {
        assert_eq!(clamp_box((-10, -10, 40, 40), 20, 20), (0, 0, 20, 20));
        assert_eq!(clamp_box((5, 5, 100, 100), 20, 20), (5, 5, 15, 15));
    }

    #[test]
    fn auto_subject_crops_to_the_foreground_block() {
        // a dark subject block on a white border-background; the builtin
        // segmenter should isolate it and the crop should tighten onto it.
        let mut img = RgbaImage::from_pixel(40, 40, Rgba([255, 255, 255, 255]));
        for y in 10..30 {
            for x in 8..24 {
                img.put_pixel(x, y, Rgba([10, 10, 10, 255]));
            }
        }
        let dir = std::env::temp_dir().join(format!("hgripe_crop_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("subject.png");
        img.save(&src).unwrap();

        let node = StudioGraphNode {
            id: "crop-1".to_string(),
            kind: "crop".to_string(),
            params: BTreeMap::from([
                ("mode".to_string(), json!("auto_subject")),
                ("margin_pct".to_string(), json!(0)),
                ("output_dir".to_string(), json!(dir.to_string_lossy())),
                ("output_name".to_string(), json!("subject_out")),
            ]),
        };
        let inputs = BTreeMap::from([("image".to_string(), json!(src.to_string_lossy()))]);
        let out = execute_studio_crop(&node, &inputs, &HashSet::new()).unwrap();
        let report = out.get("crop_report").unwrap();
        let size = &report["output_size"];
        // the cropped result is far smaller than the 40x40 source.
        assert!(size[0].as_u64().unwrap() <= 20);
        assert!(size[1].as_u64().unwrap() <= 24);
    }

    #[test]
    fn a_compute_only_image_output_skips_the_png_write() {
        // A plain manual crop whose `image` port is marked skippable: the file
        // is never written, yet the decoded surface is available from the buffer
        // (the downstream compute card and the node thumbnail both read it).
        let img = RgbaImage::from_pixel(20, 20, Rgba([4, 5, 6, 255]));
        let dir = std::env::temp_dir().join(format!(
            "hgripe_crop_skip_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("skip_src.png");
        img.save(&src).unwrap();

        let node = StudioGraphNode {
            id: "crop-skip".to_string(),
            kind: "crop".to_string(),
            params: BTreeMap::from([
                ("mode".to_string(), json!("manual")),
                ("crop_box".to_string(), json!([2, 2, 10, 8])),
                ("output_dir".to_string(), json!(dir.to_string_lossy())),
                ("output_name".to_string(), json!("skip_out")),
            ]),
        };
        let inputs = BTreeMap::from([("image".to_string(), json!(src.to_string_lossy()))]);
        let skip = HashSet::from(["image".to_string()]);
        let out = execute_studio_crop(&node, &inputs, &skip).unwrap();

        let out_path = dir.join("skip_out.png");
        assert!(
            !out_path.exists(),
            "a compute-only image output must not write its PNG"
        );
        // The path is still emitted, and the surface resolves from the buffer.
        let emitted = out.get("image").and_then(|v| v.as_str()).unwrap();
        assert_eq!(emitted, out_path.to_string_lossy());
        let loaded = studio_image::load_rgba(&out_path, 0).expect("crop output loads from buffer");
        assert_eq!(loaded.image.dimensions(), (10, 8));

        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn a_wide_gamut_source_crops_to_a_16bit_prophoto_output() {
        use super::super::working_image::{self, WorkingImage, WorkingSpace};

        let dir = std::env::temp_dir().join(format!(
            "hgripe_crop_prophoto_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // A 6x4 ProPhoto source with per-pixel-distinct 16-bit samples, written
        // through the manual-output encoder (as an upstream card would).
        let src = dir.join("wide_src.png");
        let source = WorkingImage {
            width: 6,
            height: 4,
            pixels: (0..6 * 4 * 4)
                .map(|i| (i as u16).wrapping_mul(2_741).wrapping_add(11))
                .collect(),
            space: WorkingSpace::ProPhoto,
            icc: Some(working_image::prophoto_icc().to_vec()),
        };
        studio_image::write_working_png(&src, &source).unwrap();

        let node = StudioGraphNode {
            id: "crop-wide".to_string(),
            kind: "crop".to_string(),
            params: BTreeMap::from([
                ("mode".to_string(), json!("manual")),
                ("crop_box".to_string(), json!([1, 1, 3, 2])),
                ("output_dir".to_string(), json!(dir.to_string_lossy())),
                ("output_name".to_string(), json!("wide_out")),
            ]),
        };
        let inputs = BTreeMap::from([("image".to_string(), json!(src.to_string_lossy()))]);
        let out = execute_studio_crop(&node, &inputs, &HashSet::new()).unwrap();
        let report = out.get("crop_report").unwrap();
        assert_eq!(report["output_size"], json!([3, 2]));

        // The file on disk is a 16-bit RGBA PNG carrying the ProPhoto profile.
        let out_path = dir.join("wide_out.png");
        let decoder = png::Decoder::new(std::fs::File::open(&out_path).unwrap());
        let reader = decoder.read_info().unwrap();
        let info = reader.info();
        assert_eq!(info.bit_depth, png::BitDepth::Sixteen);
        assert_eq!(
            info.icc_profile.as_deref(),
            Some(working_image::prophoto_icc())
        );

        // The next manual card reads back the exact 16-bit crop window: for
        // each pixel of the 3x2 window at (1,1), all four channels match the
        // source samples — no 8-bit round-trip anywhere in the chain.
        let loaded = studio_image::load_working(&out_path, 0).unwrap();
        assert_eq!(loaded.image.space, WorkingSpace::ProPhoto);
        let mut expected = Vec::new();
        for y in 1..3usize {
            let start = (y * 6 + 1) * 4;
            expected.extend_from_slice(&source.pixels[start..start + 3 * 4]);
        }
        assert_eq!(loaded.image.pixels, expected);

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out_path);
    }

    #[test]
    fn a_tiff_format_crops_to_a_16bit_prophoto_tiff() {
        use super::super::working_image::{self, WorkingImage, WorkingSpace};

        let dir = std::env::temp_dir().join(format!(
            "hgripe_crop_tiff_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let src = dir.join("wide_src.png");
        let source = WorkingImage {
            width: 6,
            height: 4,
            pixels: (0..6 * 4 * 4)
                .map(|i| (i as u16).wrapping_mul(2_741).wrapping_add(11))
                .collect(),
            space: WorkingSpace::ProPhoto,
            icc: Some(working_image::prophoto_icc().to_vec()),
        };
        studio_image::write_working_png(&src, &source).unwrap();

        let node = StudioGraphNode {
            id: "crop-tiff".to_string(),
            kind: "crop".to_string(),
            params: BTreeMap::from([
                ("mode".to_string(), json!("manual")),
                ("crop_box".to_string(), json!([1, 1, 3, 2])),
                ("format".to_string(), json!("tiff")),
                ("output_dir".to_string(), json!(dir.to_string_lossy())),
                ("output_name".to_string(), json!("wide_out")),
            ]),
        };
        let inputs = BTreeMap::from([("image".to_string(), json!(src.to_string_lossy()))]);
        let out = execute_studio_crop(&node, &inputs, &HashSet::new()).unwrap();
        // The emitted path carries the tiff extension, and the file exists.
        let emitted = out.get("image").and_then(|v| v.as_str()).unwrap();
        let out_path = dir.join("wide_out.tiff");
        assert_eq!(emitted, out_path.to_string_lossy());
        assert!(out_path.exists());

        // The next manual card reads back the exact 16-bit crop window off the
        // TIFF — same wide-gamut surface as the PNG carrier, no 8-bit hop.
        let loaded = studio_image::load_working(&out_path, 0).unwrap();
        assert_eq!(loaded.image.space, WorkingSpace::ProPhoto);
        let mut expected = Vec::new();
        for y in 1..3usize {
            let start = (y * 6 + 1) * 4;
            expected.extend_from_slice(&source.pixels[start..start + 3 * 4]);
        }
        assert_eq!(loaded.image.pixels, expected);

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out_path);
    }
}
