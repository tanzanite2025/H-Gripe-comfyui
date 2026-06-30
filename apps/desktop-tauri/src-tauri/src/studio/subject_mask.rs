//! The `subjectMask` node executor: the first Studio card whose image
//! processing runs in-process in native Rust (the `Compute` executor lane)
//! rather than shelling out to a `python/bridge` CLI.
//!
//! Phase 1 is CPU-only and deterministic: it builds a subject matte from a base
//! mask (a connected `previous_mask` / `placeholder_mask`, else empty) plus the
//! manual edits carried in `edit_paths` (magic-wand flood fill, brush / eraser
//! strokes), then applies morphology (`grow` / `shrink`, `fill_holes`) and a
//! final feather. It emits the mask / alpha image / cutout triplet and an
//! enriched `matte_report`. The auto-subject model modes are Phase 2 (still on
//! the `Compute` lane, via `ort` / `candle`).

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::{imageops, GrayImage, Luma, Rgba, RgbaImage};
use serde::Serialize;
use serde_json::{json, Value};

use super::graph::{
    studio_output_map, studio_value_to_number, studio_value_to_string, StudioGraphNode,
};
use super::persist::studio_reject_unsafe_basename;
use super::studio_image;
use super::subject_matte;
use super::subject_segment::{segmenter_for_mode, AutoMode, SegmentRequest};

const MASK_ON: u8 = 255;
const MASK_OFF: u8 = 0;
/// A pixel counts as "selected" for coverage / bbox once it is at least
/// half-opaque.
const SELECTED_THRESHOLD: u8 = 128;

/// The flat enriched report mirrored onto the `matte_report` output port. Mirrors
/// the enriched-report convention used across the PSD chain (`source_mode`,
/// `exif_transposed`, `max_decode_pixels`, `processing_time_ms`, triplet
/// completeness).
#[derive(Debug, Serialize)]
struct MatteReport {
    mode: String,
    provider: String,
    source_mode: String,
    exif_transposed: bool,
    max_decode_pixels: u64,
    image_size: [u32; 2],
    mask_coverage: f64,
    detected_subjects: Vec<Value>,
    operations: Vec<Value>,
    triplet: Triplet,
    processing_time_ms: u128,
}

#[derive(Debug, Serialize)]
struct Triplet {
    mask: bool,
    alpha_image: bool,
    cutout_image: bool,
}

fn number_param(node: &StudioGraphNode, key: &str, default: f64) -> f64 {
    match node.params.get(key) {
        Some(value) => studio_value_to_number(Some(value)),
        None => default,
    }
}

fn bool_param(node: &StudioGraphNode, key: &str, default: bool) -> bool {
    node.params
        .get(key)
        .map(super::graph::studio_truthy)
        .unwrap_or(default)
}

fn optional(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) fn execute_studio_subject_mask(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let started = Instant::now();

    let image_path = studio_value_to_string(inputs.get("image"));
    if image_path.trim().is_empty() {
        return Err("Subject Mask needs a connected image input".to_string());
    }

    let max_decode_pixels = {
        let configured = number_param(node, "max_decode_pixels", -1.0);
        if configured < 0.0 {
            studio_image::DEFAULT_MAX_DECODE_PIXELS
        } else {
            configured as u64
        }
    };

    let loaded = studio_image::load_rgba(Path::new(image_path.trim()), max_decode_pixels)?;
    let image = loaded.image;
    let (width, height) = image.dimensions();

    let mode = param_or(node, "mode", "hybrid");
    let auto_mode = AutoMode::from_mode(&mode);
    let mut operations: Vec<Value> = Vec::new();
    let mut detected_subjects: Vec<Value> = Vec::new();
    // `rust-native` for the manual / hybrid lanes; an `auto_*` mode reports the
    // segmenter that produced its base matte (today the builtin fallback).
    let mut provider = "rust-native".to_string();

    let placeholder = match optional(studio_value_to_string(inputs.get("placeholder_mask"))) {
        Some(path) => Some(load_mask_sized(&path, width, height, max_decode_pixels)?),
        None => None,
    };

    // Base mask: continue a prior mask; else for an `auto_*` mode segment a base
    // matte from the image; else seed from a PSD placeholder; else start empty
    // (a fully transparent matte is a valid result).
    let mut mask = match optional(studio_value_to_string(inputs.get("previous_mask"))) {
        Some(path) => load_mask_sized(&path, width, height, max_decode_pixels)?,
        None => match auto_mode {
            Some(auto) => {
                let prompt = optional(studio_value_to_string(inputs.get("prompt")));
                let points = parse_point_prompts(inputs.get("edit_paths"));
                let segmenter = segmenter_for_mode(auto, &points);
                let result = segmenter.segment(&SegmentRequest {
                    image: &image,
                    mode: auto,
                    placeholder: placeholder.as_ref(),
                    prompt: prompt.as_deref(),
                    points: &points,
                })?;
                provider = segmenter.provider().to_string();
                detected_subjects = result.detected_subjects;
                operations.push(json!({
                    "type": "auto_segment",
                    "mode": mode,
                    "provider": provider,
                }));
                result.mask
            }
            None => match &placeholder {
                Some(seed) => seed.clone(),
                None => GrayImage::from_pixel(width, height, Luma([MASK_OFF])),
            },
        },
    };

    let wand_tolerance = number_param(node, "wand_tolerance", 24.0).clamp(0.0, 255.0) as i32;

    apply_edit_paths(
        &image,
        &mut mask,
        inputs.get("edit_paths"),
        wand_tolerance,
        &mut operations,
    );

    if bool_param(node, "fill_holes", false) {
        fill_holes(&mut mask);
        operations.push(json!({ "type": "fill_holes" }));
    }

    let grow_px = number_param(node, "grow_px", 0.0) as i32;
    if grow_px > 0 {
        mask = dilate(&mask, grow_px as u32);
        operations.push(json!({ "type": "grow", "px": grow_px }));
    } else if grow_px < 0 {
        mask = erode(&mask, grow_px.unsigned_abs());
        operations.push(json!({ "type": "shrink", "px": grow_px.abs() }));
    }

    // Continuous alpha matting: resolve the binary edge into soft alpha (hair /
    // glass / translucency) via a trimap. Off by default so Phase 1 stays
    // binary + deterministic; behind the flag it runs ViTMatte when its weight
    // resolves, else the deterministic builtin feather fallback.
    if bool_param(node, "alpha_matting", false) {
        let band = number_param(node, "matting_band_px", 12.0).max(0.0) as u32;
        let trimap = subject_matte::trimap_from_mask(&mask, band);
        let matter = subject_matte::matter();
        let matte_provider = matter.provider().to_string();
        mask = matter.matte(&image, &trimap)?;
        operations.push(json!({
            "type": "alpha_matting",
            "provider": matte_provider,
            "band_px": band,
        }));
    }

    let feather_px = number_param(node, "feather_px", 0.0).max(0.0);
    if feather_px > 0.0 {
        mask = imageops::blur(&mask, feather_px as f32);
        operations.push(json!({ "type": "feather", "px": feather_px }));
    }

    let coverage = mask_coverage(&mask);
    let alpha_image = compose_alpha(&image, &mask);
    let cutout = cutout_to_bbox(&alpha_image, &mask);

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
            format!("{}_mask", image_stem(&image_path))
        } else {
            configured.trim().to_string()
        }
    };
    studio_reject_unsafe_basename(&base)?;

    let dir = PathBuf::from(&output_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("failed to create output dir {}: {err}", dir.display()))?;
    let mask_path = dir.join(format!("{base}.png"));
    let alpha_path = dir.join(format!("{base}_alpha.png"));
    let cutout_path = dir.join(format!("{base}_cutout.png"));
    let paths_path = dir.join(format!("{base}_paths.json"));

    save_png(&DynamicGray(&mask), &mask_path)?;
    alpha_image
        .save(&alpha_path)
        .map_err(|err| format!("failed to write {}: {err}", alpha_path.display()))?;
    cutout
        .save(&cutout_path)
        .map_err(|err| format!("failed to write {}: {err}", cutout_path.display()))?;

    let edit_paths_value = normalise_edit_paths(inputs.get("edit_paths"));
    std::fs::write(
        &paths_path,
        serde_json::to_vec_pretty(&edit_paths_value)
            .map_err(|err| format!("failed to encode edit_paths: {err}"))?,
    )
    .map_err(|err| format!("failed to write {}: {err}", paths_path.display()))?;

    let report = MatteReport {
        mode,
        provider,
        source_mode: loaded.meta.source_mode.clone(),
        exif_transposed: loaded.meta.exif_transposed,
        max_decode_pixels,
        image_size: [width, height],
        mask_coverage: coverage,
        detected_subjects,
        operations,
        triplet: Triplet {
            mask: mask_path.is_file(),
            alpha_image: alpha_path.is_file(),
            cutout_image: cutout_path.is_file(),
        },
        processing_time_ms: started.elapsed().as_millis(),
    };
    let report = serde_json::to_value(&report)
        .map_err(|err| format!("failed to encode matte_report: {err}"))?;

    Ok(studio_output_map([
        ("mask", json!(mask_path.to_string_lossy())),
        ("alpha_image", json!(alpha_path.to_string_lossy())),
        ("cutout_image", json!(cutout_path.to_string_lossy())),
        ("matte_report", report),
        ("edit_paths", edit_paths_value),
    ]))
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

fn load_mask_sized(
    path: &str,
    width: u32,
    height: u32,
    max_pixels: u64,
) -> Result<GrayImage, String> {
    let mask = studio_image::load_mask(Path::new(path.trim()), max_pixels)?;
    if mask.dimensions() == (width, height) {
        Ok(mask)
    } else {
        Ok(imageops::resize(
            &mask,
            width,
            height,
            imageops::FilterType::Nearest,
        ))
    }
}

// --- pure mask operations (unit-tested without disk) -----------------------

/// Apply the manual edits recorded in `edit_paths`: magic-wand flood selections,
/// brush / eraser strokes, and a whole-mask invert. Unknown entries are ignored.
fn apply_edit_paths(
    image: &RgbaImage,
    mask: &mut GrayImage,
    edit_paths: Option<&Value>,
    default_tolerance: i32,
    operations: &mut Vec<Value>,
) {
    let Some(value) = parse_edit_paths(edit_paths) else {
        return;
    };

    for op in value
        .get("ops")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match op.get("type").and_then(Value::as_str) {
            Some("wand") => {
                let (Some(x), Some(y)) = (json_u32(op.get("x")), json_u32(op.get("y"))) else {
                    continue;
                };
                let tolerance = op
                    .get("tolerance")
                    .and_then(Value::as_i64)
                    .map(|t| t.clamp(0, 255) as i32)
                    .unwrap_or(default_tolerance);
                wand_select(image, mask, x, y, tolerance);
                operations.push(json!({ "type": "wand", "tolerance": tolerance }));
            }
            Some("invert") => {
                invert(mask);
                operations.push(json!({ "type": "invert" }));
            }
            _ => {}
        }
    }

    for stroke in value
        .get("brush_strokes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let subtract = stroke.get("mode").and_then(Value::as_str) == Some("subtract");
        let radius = stroke
            .get("radius")
            .and_then(Value::as_f64)
            .unwrap_or(8.0)
            .max(0.0) as u32;
        let points = parse_points(stroke.get("points"));
        if points.is_empty() {
            continue;
        }
        stamp_stroke(
            mask,
            &points,
            radius,
            if subtract { MASK_OFF } else { MASK_ON },
        );
        operations.push(json!({
            "type": if subtract { "brush_subtract" } else { "brush_add" },
            "radius": radius,
        }));
    }
}

/// Flood-fill from a seed, selecting the contiguous region whose colour stays
/// within `tolerance` (max per-channel RGB distance) of the seed colour.
fn wand_select(image: &RgbaImage, mask: &mut GrayImage, seed_x: u32, seed_y: u32, tolerance: i32) {
    let (width, height) = image.dimensions();
    if seed_x >= width || seed_y >= height {
        return;
    }
    let seed = image.get_pixel(seed_x, seed_y).0;
    let mut visited = vec![false; (width * height) as usize];
    let mut queue = VecDeque::new();
    queue.push_back((seed_x, seed_y));
    visited[(seed_y * width + seed_x) as usize] = true;

    while let Some((x, y)) = queue.pop_front() {
        let px = image.get_pixel(x, y).0;
        let dist = (0..3)
            .map(|c| (i32::from(px[c]) - i32::from(seed[c])).abs())
            .max()
            .unwrap_or(0);
        if dist > tolerance {
            continue;
        }
        mask.put_pixel(x, y, Luma([MASK_ON]));
        for (nx, ny) in neighbours(x, y, width, height) {
            let idx = (ny * width + nx) as usize;
            if !visited[idx] {
                visited[idx] = true;
                queue.push_back((nx, ny));
            }
        }
    }
}

fn neighbours(x: u32, y: u32, width: u32, height: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::with_capacity(4);
    if x > 0 {
        out.push((x - 1, y));
    }
    if x + 1 < width {
        out.push((x + 1, y));
    }
    if y > 0 {
        out.push((x, y - 1));
    }
    if y + 1 < height {
        out.push((x, y + 1));
    }
    out
}

/// Stamp filled discs of `radius` along a polyline, writing `value`.
fn stamp_stroke(mask: &mut GrayImage, points: &[(f32, f32)], radius: u32, value: u8) {
    for &(px, py) in points {
        stamp_disc(mask, px, py, radius, value);
    }
}

fn stamp_disc(mask: &mut GrayImage, cx: f32, cy: f32, radius: u32, value: u8) {
    let (width, height) = mask.dimensions();
    let r = radius as i32;
    let cxi = cx.round() as i32;
    let cyi = cy.round() as i32;
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy > r * r {
                continue;
            }
            let x = cxi + dx;
            let y = cyi + dy;
            if x >= 0 && y >= 0 && (x as u32) < width && (y as u32) < height {
                mask.put_pixel(x as u32, y as u32, Luma([value]));
            }
        }
    }
}

fn invert(mask: &mut GrayImage) {
    for p in mask.pixels_mut() {
        p.0[0] = 255 - p.0[0];
    }
}

/// Fill interior holes: flood the background inward from the borders, then any
/// off pixel the flood never reached is an enclosed hole and is turned on.
fn fill_holes(mask: &mut GrayImage) {
    let (width, height) = mask.dimensions();
    let mut reachable = vec![false; (width * height) as usize];
    let mut queue = VecDeque::new();
    let mut seed = |x: u32, y: u32, queue: &mut VecDeque<(u32, u32)>| {
        let idx = (y * width + x) as usize;
        if !reachable[idx] && mask.get_pixel(x, y).0[0] < SELECTED_THRESHOLD {
            reachable[idx] = true;
            queue.push_back((x, y));
        }
    };
    for x in 0..width {
        seed(x, 0, &mut queue);
        seed(x, height - 1, &mut queue);
    }
    for y in 0..height {
        seed(0, y, &mut queue);
        seed(width - 1, y, &mut queue);
    }
    while let Some((x, y)) = queue.pop_front() {
        for (nx, ny) in neighbours(x, y, width, height) {
            let idx = (ny * width + nx) as usize;
            if !reachable[idx] && mask.get_pixel(nx, ny).0[0] < SELECTED_THRESHOLD {
                reachable[idx] = true;
                queue.push_back((nx, ny));
            }
        }
    }
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            if !reachable[idx] && mask.get_pixel(x, y).0[0] < SELECTED_THRESHOLD {
                mask.put_pixel(x, y, Luma([MASK_ON]));
            }
        }
    }
}

/// Separable max filter: grow the matte outward by `radius` px. Also used by
/// [`subject_matte`](super::subject_matte) to build trimaps.
pub(super) fn dilate(mask: &GrayImage, radius: u32) -> GrayImage {
    morphology(mask, radius, true)
}

/// Separable min filter: bite the matte inward by `radius` px. Also used by
/// [`subject_matte`](super::subject_matte) to build trimaps.
pub(super) fn erode(mask: &GrayImage, radius: u32) -> GrayImage {
    morphology(mask, radius, false)
}

fn morphology(mask: &GrayImage, radius: u32, grow: bool) -> GrayImage {
    if radius == 0 {
        return mask.clone();
    }
    let (width, height) = mask.dimensions();
    let r = radius as i32;
    let pick = |acc: u8, v: u8| if grow { acc.max(v) } else { acc.min(v) };

    // Horizontal pass.
    let mut tmp = GrayImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let mut acc = if grow { MASK_OFF } else { MASK_ON };
            for dx in -r..=r {
                let sx = x as i32 + dx;
                if sx >= 0 && (sx as u32) < width {
                    acc = pick(acc, mask.get_pixel(sx as u32, y).0[0]);
                }
            }
            tmp.put_pixel(x, y, Luma([acc]));
        }
    }
    // Vertical pass.
    let mut out = GrayImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let mut acc = if grow { MASK_OFF } else { MASK_ON };
            for dy in -r..=r {
                let sy = y as i32 + dy;
                if sy >= 0 && (sy as u32) < height {
                    acc = pick(acc, tmp.get_pixel(x, sy as u32).0[0]);
                }
            }
            out.put_pixel(x, y, Luma([acc]));
        }
    }
    out
}

fn mask_coverage(mask: &GrayImage) -> f64 {
    let total = mask.pixels().len();
    if total == 0 {
        return 0.0;
    }
    let on = mask
        .pixels()
        .filter(|p| p.0[0] >= SELECTED_THRESHOLD)
        .count();
    on as f64 / total as f64
}

fn compose_alpha(image: &RgbaImage, mask: &GrayImage) -> RgbaImage {
    let (width, height) = image.dimensions();
    let mut out = image.clone();
    for y in 0..height {
        for x in 0..width {
            let a = mask.get_pixel(x, y).0[0];
            let mut px = out.get_pixel(x, y).0;
            px[3] = a;
            out.put_pixel(x, y, Rgba(px));
        }
    }
    out
}

fn cutout_to_bbox(alpha_image: &RgbaImage, mask: &GrayImage) -> RgbaImage {
    match selection_bbox(mask) {
        Some((x0, y0, x1, y1)) => {
            imageops::crop_imm(alpha_image, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image()
        }
        // Empty selection: a valid 1x1 transparent cutout (never panic).
        None => RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 0])),
    }
}

fn selection_bbox(mask: &GrayImage) -> Option<(u32, u32, u32, u32)> {
    let (width, height) = mask.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut any = false;
    for y in 0..height {
        for x in 0..width {
            if mask.get_pixel(x, y).0[0] >= SELECTED_THRESHOLD {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    any.then_some((x0, y0, x1, y1))
}

// --- edit_paths parsing ----------------------------------------------------

fn parse_edit_paths(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::Object(_)) => value.cloned(),
        Some(Value::String(text)) if !text.trim().is_empty() => {
            serde_json::from_str::<Value>(text).ok()
        }
        _ => None,
    }
}

/// The object echoed onto the `edit_paths` output / written to disk: the parsed
/// input when present, else an empty versioned envelope.
fn normalise_edit_paths(value: Option<&Value>) -> Value {
    parse_edit_paths(value)
        .unwrap_or_else(|| json!({ "version": 1, "paths": [], "brush_strokes": [] }))
}

/// Optional point prompts for the auto-subject segmenter, read from a top-level
/// `points` array on `edit_paths` (`[[x, y], ...]` or `[{ "x", "y" }, ...]`).
/// Absent ⇒ no prompts (the segmenter falls back to its largest component).
fn parse_point_prompts(edit_paths: Option<&Value>) -> Vec<(u32, u32)> {
    let Some(value) = parse_edit_paths(edit_paths) else {
        return Vec::new();
    };
    value
        .get("points")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| match item {
            Value::Array(pair) if pair.len() >= 2 => {
                Some((json_u32(Some(&pair[0]))?, json_u32(Some(&pair[1]))?))
            }
            Value::Object(_) => Some((json_u32(item.get("x"))?, json_u32(item.get("y"))?)),
            _ => None,
        })
        .collect()
}

fn parse_points(value: Option<&Value>) -> Vec<(f32, f32)> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Array(pair) if pair.len() >= 2 => {
                Some((json_f32(Some(&pair[0]))?, json_f32(Some(&pair[1]))?))
            }
            Value::Object(_) => Some((json_f32(item.get("x"))?, json_f32(item.get("y"))?)),
            _ => None,
        })
        .collect()
}

fn json_f32(value: Option<&Value>) -> Option<f32> {
    value.and_then(Value::as_f64).map(|n| n as f32)
}

fn json_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_f64)
        .filter(|n| *n >= 0.0)
        .map(|n| n as u32)
}

// --- PNG save helper -------------------------------------------------------

/// A thin wrapper so a `GrayImage` saves through the same `.save()` path as the
/// RGBA surfaces without an extra `DynamicImage` clone elsewhere.
struct DynamicGray<'a>(&'a GrayImage);

fn save_png(gray: &DynamicGray, path: &Path) -> Result<(), String> {
    gray.0
        .save(path)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> StudioGraphNode {
        StudioGraphNode {
            id: "n1".to_string(),
            kind: "subjectMask".to_string(),
            params: BTreeMap::new(),
        }
    }

    fn solid(width: u32, height: u32, value: u8) -> GrayImage {
        GrayImage::from_pixel(width, height, Luma([value]))
    }

    #[test]
    fn rejects_missing_image_input() {
        let err = execute_studio_subject_mask(&node(), &BTreeMap::new()).unwrap_err();
        assert!(err.contains("connected image input"), "{err}");
    }

    #[test]
    fn rejects_blank_image_input() {
        let mut inputs = BTreeMap::new();
        inputs.insert("image".to_string(), json!("   "));
        let err = execute_studio_subject_mask(&node(), &inputs).unwrap_err();
        assert!(err.contains("connected image input"), "{err}");
    }

    #[test]
    fn wand_selects_contiguous_same_colour_region() {
        // Left half red, right half blue; seeding the left selects only the left.
        let mut image = RgbaImage::new(4, 2);
        for y in 0..2 {
            for x in 0..4 {
                let colour = if x < 2 {
                    Rgba([200, 0, 0, 255])
                } else {
                    Rgba([0, 0, 200, 255])
                };
                image.put_pixel(x, y, colour);
            }
        }
        let mut mask = solid(4, 2, MASK_OFF);
        wand_select(&image, &mut mask, 0, 0, 20);
        for y in 0..2 {
            assert_eq!(mask.get_pixel(0, y).0[0], MASK_ON);
            assert_eq!(mask.get_pixel(1, y).0[0], MASK_ON);
            assert_eq!(mask.get_pixel(2, y).0[0], MASK_OFF);
            assert_eq!(mask.get_pixel(3, y).0[0], MASK_OFF);
        }
    }

    #[test]
    fn brush_stroke_adds_and_eraser_subtracts() {
        let mut mask = solid(9, 9, MASK_OFF);
        stamp_stroke(&mut mask, &[(4.0, 4.0)], 2, MASK_ON);
        assert_eq!(mask.get_pixel(4, 4).0[0], MASK_ON);
        stamp_stroke(&mut mask, &[(4.0, 4.0)], 2, MASK_OFF);
        assert_eq!(mask.get_pixel(4, 4).0[0], MASK_OFF);
    }

    #[test]
    fn dilate_grows_and_erode_shrinks() {
        let mut mask = solid(7, 7, MASK_OFF);
        stamp_disc(&mut mask, 3.0, 3.0, 1, MASK_ON);
        let before = mask_coverage(&mask);
        let grown = dilate(&mask, 1);
        assert!(mask_coverage(&grown) > before);
        let shrunk = erode(&grown, 1);
        assert!(mask_coverage(&shrunk) < mask_coverage(&grown));
    }

    #[test]
    fn fill_holes_closes_enclosed_gap() {
        // A 5x5 on-block with a single off pixel in the centre.
        let mut mask = solid(5, 5, MASK_ON);
        mask.put_pixel(2, 2, Luma([MASK_OFF]));
        fill_holes(&mut mask);
        assert_eq!(mask.get_pixel(2, 2).0[0], MASK_ON);
    }

    #[test]
    fn fill_holes_leaves_open_background() {
        let mut mask = solid(5, 5, MASK_OFF);
        fill_holes(&mut mask);
        assert_eq!(mask_coverage(&mask), 0.0);
    }

    #[test]
    fn empty_selection_yields_transparent_cutout() {
        let image = RgbaImage::from_pixel(4, 4, Rgba([10, 20, 30, 255]));
        let mask = solid(4, 4, MASK_OFF);
        let alpha = compose_alpha(&image, &mask);
        assert_eq!(alpha.get_pixel(0, 0).0[3], 0);
        let cutout = cutout_to_bbox(&alpha, &mask);
        assert_eq!(cutout.dimensions(), (1, 1));
        assert_eq!(cutout.get_pixel(0, 0).0[3], 0);
        assert_eq!(mask_coverage(&mask), 0.0);
    }

    #[test]
    fn invert_flips_mask() {
        let mut mask = solid(2, 2, MASK_OFF);
        invert(&mut mask);
        assert_eq!(mask.get_pixel(0, 0).0[0], MASK_ON);
    }

    #[test]
    fn parses_brush_points_from_pairs_and_objects() {
        let value = json!({
            "brush_strokes": [
                { "mode": "add", "radius": 1, "points": [[1, 1], {"x": 2, "y": 2}] }
            ]
        });
        let mut mask = solid(5, 5, MASK_OFF);
        let mut ops = Vec::new();
        apply_edit_paths(
            &RgbaImage::from_pixel(5, 5, Rgba([0, 0, 0, 255])),
            &mut mask,
            Some(&value),
            24,
            &mut ops,
        );
        assert_eq!(mask.get_pixel(1, 1).0[0], MASK_ON);
        assert_eq!(mask.get_pixel(2, 2).0[0], MASK_ON);
        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn normalise_edit_paths_defaults_to_versioned_envelope() {
        let value = normalise_edit_paths(None);
        assert_eq!(value.get("version").and_then(Value::as_i64), Some(1));
    }

    #[test]
    fn auto_mode_segments_base_and_reports_provider() {
        // A grey scene with a red block; auto_subject should segment the block
        // as the base matte, report the builtin provider, and list one subject.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("hgripe_subject_auto_{nanos}"));
        std::fs::create_dir_all(&root).unwrap();
        let image_path = root.join("scene.png");
        let mut image = RgbaImage::from_pixel(12, 12, Rgba([120, 120, 120, 255]));
        for y in 4..8 {
            for x in 4..8 {
                image.put_pixel(x, y, Rgba([220, 20, 20, 255]));
            }
        }
        image.save(&image_path).unwrap();

        let mut params = BTreeMap::new();
        params.insert("mode".to_string(), json!("auto_subject"));
        params.insert(
            "output_dir".to_string(),
            json!(root.to_string_lossy().to_string()),
        );
        params.insert("output_name".to_string(), json!("scene_mask"));
        let node = StudioGraphNode {
            id: "n1".to_string(),
            kind: "subjectMask".to_string(),
            params,
        };
        let mut inputs = BTreeMap::new();
        inputs.insert(
            "image".to_string(),
            json!(image_path.to_string_lossy().to_string()),
        );

        let out = execute_studio_subject_mask(&node, &inputs).unwrap();
        let report = out.get("matte_report").unwrap();
        // An auto mode reports the segmenter that produced the base matte: the
        // builtin fallback, or a model backend (u2netp / birefnet) when a weight
        // resolves. All are valid; the point is it is no longer manual
        // `rust-native`.
        let provider = report.get("provider").and_then(Value::as_str).unwrap();
        assert!(
            matches!(provider, "builtin-cpu" | "u2netp" | "birefnet"),
            "unexpected auto provider {provider}"
        );
        assert_eq!(
            report.get("mode").and_then(Value::as_str),
            Some("auto_subject")
        );
        let subjects = report
            .get("detected_subjects")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(subjects.len(), 1);
        let coverage = report.get("mask_coverage").and_then(Value::as_f64).unwrap();
        assert!(coverage > 0.0 && coverage <= 1.0, "coverage={coverage}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn manual_mode_keeps_rust_native_provider() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("hgripe_subject_manual_{nanos}"));
        std::fs::create_dir_all(&root).unwrap();
        let image_path = root.join("scene.png");
        RgbaImage::from_pixel(6, 6, Rgba([100, 100, 100, 255]))
            .save(&image_path)
            .unwrap();

        let mut params = BTreeMap::new();
        params.insert("mode".to_string(), json!("manual_brush"));
        params.insert(
            "output_dir".to_string(),
            json!(root.to_string_lossy().to_string()),
        );
        params.insert("output_name".to_string(), json!("scene_mask"));
        let node = StudioGraphNode {
            id: "n1".to_string(),
            kind: "subjectMask".to_string(),
            params,
        };
        let mut inputs = BTreeMap::new();
        inputs.insert(
            "image".to_string(),
            json!(image_path.to_string_lossy().to_string()),
        );

        let out = execute_studio_subject_mask(&node, &inputs).unwrap();
        let report = out.get("matte_report").unwrap();
        assert_eq!(
            report.get("provider").and_then(Value::as_str),
            Some("rust-native")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn end_to_end_writes_triplet_and_report() {
        // A real round-trip through the executor against a temp image + output
        // dir, asserting the triplet is written and the report shape is intact.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("hgripe_subject_mask_{nanos}"));
        std::fs::create_dir_all(&root).unwrap();
        let image_path = root.join("scene.png");
        let mut image = RgbaImage::new(6, 6);
        for p in image.pixels_mut() {
            *p = Rgba([100, 100, 100, 255]);
        }
        image.save(&image_path).unwrap();

        let mut params = BTreeMap::new();
        params.insert(
            "output_dir".to_string(),
            json!(root.to_string_lossy().to_string()),
        );
        params.insert("output_name".to_string(), json!("scene_mask"));
        let node = StudioGraphNode {
            id: "n1".to_string(),
            kind: "subjectMask".to_string(),
            params,
        };

        let mut inputs = BTreeMap::new();
        inputs.insert(
            "image".to_string(),
            json!(image_path.to_string_lossy().to_string()),
        );
        inputs.insert(
            "edit_paths".to_string(),
            json!({
                "ops": [{ "type": "wand", "x": 0, "y": 0, "tolerance": 30 }]
            }),
        );

        let out = execute_studio_subject_mask(&node, &inputs).unwrap();
        assert!(root.join("scene_mask.png").is_file());
        assert!(root.join("scene_mask_alpha.png").is_file());
        assert!(root.join("scene_mask_cutout.png").is_file());

        let report = out.get("matte_report").unwrap();
        assert_eq!(
            report.get("provider").and_then(Value::as_str),
            Some("rust-native")
        );
        assert_eq!(
            report
                .get("image_size")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        let triplet = report.get("triplet").unwrap();
        assert_eq!(triplet.get("mask").and_then(Value::as_bool), Some(true));
        // The whole image is one flat colour, so the wand selects everything.
        assert!(report.get("mask_coverage").and_then(Value::as_f64).unwrap() > 0.9);

        let _ = std::fs::remove_dir_all(&root);
    }
}
