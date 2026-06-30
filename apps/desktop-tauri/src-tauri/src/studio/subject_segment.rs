//! Phase 2 auto-subject segmentation for the `subjectMask` card.
//!
//! The `auto_*` modes (`auto_subject` / `auto_product` / `auto_person` /
//! `auto_transparent_object`) compute a *base* matte from the image itself
//! rather than starting from an empty / `previous_mask` base. Per the frozen
//! contract these run on the `Compute` lane in-process (no network); the real
//! SAM / RMBG / BiRefNet inference (`ort` / `candle`) lands in a follow-up PR.
//!
//! This module defines the lane-internal [`SubjectSegmenter`] abstraction the
//! card routes those modes through, plus a deterministic, weight-free
//! [`BuiltinCpuSegmenter`] fallback so the modes work end-to-end today. A model
//! backend implements the same trait and is selected by [`segmenter_for_mode`]
//! once weights are available; until then every `auto_*` mode resolves to the
//! builtin fallback.

use std::collections::VecDeque;

use image::{GrayImage, Luma, RgbaImage};
use serde_json::{json, Value};

const MASK_ON: u8 = 255;
const MASK_OFF: u8 = 0;
/// A pixel counts as selected once it is at least half-opaque (mirrors
/// `subject_mask::SELECTED_THRESHOLD`).
const SELECTED_THRESHOLD: u8 = 128;
/// Per-channel colour distance from the estimated background past which a pixel
/// is treated as foreground by the builtin fallback.
const DEFAULT_FOREGROUND_DELTA: i32 = 32;
/// Source-alpha below this counts as "see-through" for the transparent-object
/// mode (glassware, bottles), so it is kept even when its colour matches the
/// background.
const TRANSPARENT_ALPHA_CEILING: u8 = 250;

/// The `auto_*` subject modes. Parsed from the node's `mode` param;
/// `manual_*` / `hybrid` modes are not auto and never reach the segmenter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AutoMode {
    Subject,
    Product,
    Person,
    TransparentObject,
}

impl AutoMode {
    /// Map the `mode` param string to an auto mode, or `None` for the manual /
    /// hybrid modes (which the card handles without a segmenter).
    pub(super) fn from_mode(mode: &str) -> Option<Self> {
        match mode {
            "auto_subject" => Some(Self::Subject),
            "auto_product" => Some(Self::Product),
            "auto_person" => Some(Self::Person),
            "auto_transparent_object" => Some(Self::TransparentObject),
            _ => None,
        }
    }

    /// The label echoed onto each detected subject in `matte_report`.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Subject => "subject",
            Self::Product => "product",
            Self::Person => "person",
            Self::TransparentObject => "transparent_object",
        }
    }
}

/// What the card hands a segmenter: the decoded image plus the optional hints
/// the contract exposes (a PSD placeholder region to stay inside, a text prompt,
/// and point prompts from the node's click-to-select).
pub(super) struct SegmentRequest<'a> {
    pub image: &'a RgbaImage,
    pub mode: AutoMode,
    /// PSD placeholder region; when present the result is constrained to it.
    pub placeholder: Option<&'a GrayImage>,
    pub prompt: Option<&'a str>,
    pub points: &'a [(u32, u32)],
}

/// A segmenter's output: the base matte plus the structured `detected_subjects`
/// entries the report carries.
pub(super) struct SegmentResult {
    pub mask: GrayImage,
    pub detected_subjects: Vec<Value>,
}

/// In-process, network-free subject segmentation on the `Compute` lane. Phase 2
/// model backends (`ort` / `candle`) implement this beside the builtin
/// fallback.
pub(super) trait SubjectSegmenter {
    /// The provider / model id recorded in `matte_report.provider`.
    fn provider(&self) -> &str;
    fn segment(&self, request: &SegmentRequest) -> Result<SegmentResult, String>;
}

/// Choose the segmenter for an auto mode. When the request carries point
/// prompts, the interactive SAM 2 backend is preferred (it segments *what the
/// user clicked*) if both its weights resolve. Otherwise the prompt-free
/// salient model backend (BiRefNet → U²-Netp) is used when a weight resolves,
/// and finally the deterministic builtin CPU fallback. The call site
/// (`subject_mask`) hands the same `SegmentRequest` to whichever is returned.
pub(super) fn segmenter_for_mode(
    mode: AutoMode,
    points: &[(u32, u32)],
) -> Box<dyn SubjectSegmenter> {
    if !points.is_empty() {
        if let Some(sam2) = super::subject_sam2::Sam2Segmenter::resolve_and_load() {
            return Box::new(sam2);
        }
    }
    if let Some(model) = super::subject_model::model_segmenter_for_mode(mode) {
        return Box::new(model);
    }
    Box::new(BuiltinCpuSegmenter)
}

/// A deterministic, weight-free segmenter: estimate the background colour from
/// the image border, mark pixels that differ from it (or, for the transparent
/// mode, that are see-through) as foreground, then keep the single largest
/// connected component so stray specks don't leak into the matte. Same input ⇒
/// same matte, so the `auto_*` modes stay reproducible until a model lands.
pub(super) struct BuiltinCpuSegmenter;

impl SubjectSegmenter for BuiltinCpuSegmenter {
    fn provider(&self) -> &str {
        "builtin-cpu"
    }

    fn segment(&self, request: &SegmentRequest) -> Result<SegmentResult, String> {
        let (width, height) = request.image.dimensions();
        if width == 0 || height == 0 {
            return Err("Subject Mask auto mode needs a non-empty image".to_string());
        }

        let background = estimate_background(request.image);
        let mut foreground = mark_foreground(request.image, request.mode, background);
        if let Some(placeholder) = request.placeholder {
            constrain_to_placeholder(&mut foreground, placeholder);
        }
        let mask = keep_largest_component(&foreground, request.points);

        let detected_subjects = match selection_bbox(&mask) {
            Some((x0, y0, x1, y1)) => vec![json!({
                "label": request.mode.label(),
                "prompt": request.prompt.unwrap_or(""),
                "bbox": [x0, y0, x1 - x0 + 1, y1 - y0 + 1],
                "coverage": coverage(&mask),
                "provider": self.provider(),
            })],
            None => Vec::new(),
        };

        Ok(SegmentResult {
            mask,
            detected_subjects,
        })
    }
}

/// Median per-channel RGB of the one-pixel image border, used as the background
/// estimate. Median (not mean) shrugs off a subject that touches an edge.
fn estimate_background(image: &RgbaImage) -> [u8; 3] {
    let (width, height) = image.dimensions();
    let mut channels: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut push = |x: u32, y: u32| {
        let px = image.get_pixel(x, y).0;
        for c in 0..3 {
            channels[c].push(px[c]);
        }
    };
    for x in 0..width {
        push(x, 0);
        push(x, height - 1);
    }
    for y in 0..height {
        push(0, y);
        push(width - 1, y);
    }
    let mut out = [0u8; 3];
    for c in 0..3 {
        channels[c].sort_unstable();
        out[c] = channels[c].get(channels[c].len() / 2).copied().unwrap_or(0);
    }
    out
}

/// Per-pixel foreground test: colour distance from the background over a
/// threshold, plus (for the transparent mode) any see-through pixel.
fn mark_foreground(image: &RgbaImage, mode: AutoMode, background: [u8; 3]) -> GrayImage {
    let (width, height) = image.dimensions();
    let mut out = GrayImage::from_pixel(width, height, Luma([MASK_OFF]));
    let keep_transparent = matches!(mode, AutoMode::TransparentObject);
    for y in 0..height {
        for x in 0..width {
            let px = image.get_pixel(x, y).0;
            let dist = (0..3)
                .map(|c| (i32::from(px[c]) - i32::from(background[c])).abs())
                .max()
                .unwrap_or(0);
            let see_through = keep_transparent && px[3] < TRANSPARENT_ALPHA_CEILING;
            if dist > DEFAULT_FOREGROUND_DELTA || see_through {
                out.put_pixel(x, y, Luma([MASK_ON]));
            }
        }
    }
    out
}

/// Zero out any foreground pixel outside the placeholder selection.
fn constrain_to_placeholder(foreground: &mut GrayImage, placeholder: &GrayImage) {
    if placeholder.dimensions() != foreground.dimensions() {
        return;
    }
    let (width, height) = foreground.dimensions();
    for y in 0..height {
        for x in 0..width {
            if placeholder.get_pixel(x, y).0[0] < SELECTED_THRESHOLD {
                foreground.put_pixel(x, y, Luma([MASK_OFF]));
            }
        }
    }
}

/// Keep one connected component of the foreground. If point prompts are given,
/// keep every component that any point lands in; otherwise keep the single
/// largest. Deterministic: ties in size resolve to the earlier (top-left) seed.
fn keep_largest_component(foreground: &GrayImage, points: &[(u32, u32)]) -> GrayImage {
    let (width, height) = foreground.dimensions();
    let on = |x: u32, y: u32| foreground.get_pixel(x, y).0[0] >= SELECTED_THRESHOLD;
    let mut label = vec![0u32; (width * height) as usize];
    let mut sizes: Vec<u32> = vec![0]; // index 0 = unlabelled
    let mut next_label = 1u32;

    for sy in 0..height {
        for sx in 0..width {
            let idx = (sy * width + sx) as usize;
            if !on(sx, sy) || label[idx] != 0 {
                continue;
            }
            let mut size = 0u32;
            let mut queue = VecDeque::new();
            label[idx] = next_label;
            queue.push_back((sx, sy));
            while let Some((x, y)) = queue.pop_front() {
                size += 1;
                for (nx, ny) in neighbours(x, y, width, height) {
                    let nidx = (ny * width + nx) as usize;
                    if on(nx, ny) && label[nidx] == 0 {
                        label[nidx] = next_label;
                        queue.push_back((nx, ny));
                    }
                }
            }
            sizes.push(size);
            next_label += 1;
        }
    }

    let mut keep: Vec<bool> = vec![false; next_label as usize];
    let prompted: Vec<u32> = points
        .iter()
        .filter(|&&(x, y)| x < width && y < height)
        .map(|&(x, y)| label[(y * width + x) as usize])
        .filter(|&l| l != 0)
        .collect();
    if !prompted.is_empty() {
        for l in prompted {
            keep[l as usize] = true;
        }
    } else if let Some((largest, _)) = sizes
        .iter()
        .enumerate()
        .skip(1)
        .max_by_key(|&(_, &size)| size)
    {
        keep[largest] = true;
    }

    let mut out = GrayImage::from_pixel(width, height, Luma([MASK_OFF]));
    for y in 0..height {
        for x in 0..width {
            let l = label[(y * width + x) as usize];
            if l != 0 && keep[l as usize] {
                out.put_pixel(x, y, Luma([MASK_ON]));
            }
        }
    }
    out
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

fn coverage(mask: &GrayImage) -> f64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// A grey background with a distinct red block; the segmenter should select
    /// exactly the block.
    fn scene_with_block() -> RgbaImage {
        let mut image = RgbaImage::from_pixel(10, 10, Rgba([120, 120, 120, 255]));
        for y in 3..7 {
            for x in 3..7 {
                image.put_pixel(x, y, Rgba([220, 20, 20, 255]));
            }
        }
        image
    }

    #[test]
    fn from_mode_maps_only_auto_modes() {
        assert_eq!(AutoMode::from_mode("auto_subject"), Some(AutoMode::Subject));
        assert_eq!(
            AutoMode::from_mode("auto_transparent_object"),
            Some(AutoMode::TransparentObject)
        );
        assert_eq!(AutoMode::from_mode("manual_brush"), None);
        assert_eq!(AutoMode::from_mode("hybrid"), None);
    }

    #[test]
    fn builtin_selects_foreground_block() {
        let image = scene_with_block();
        let result = segmenter_for_mode(AutoMode::Subject, &[])
            .segment(&SegmentRequest {
                image: &image,
                mode: AutoMode::Subject,
                placeholder: None,
                prompt: None,
                points: &[],
            })
            .unwrap();
        // Inside the block selected, outside (background) not.
        assert_eq!(result.mask.get_pixel(4, 4).0[0], MASK_ON);
        assert_eq!(result.mask.get_pixel(0, 0).0[0], MASK_OFF);
        assert_eq!(result.detected_subjects.len(), 1);
        let bbox = result.detected_subjects[0]
            .get("bbox")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(bbox.len(), 4);
    }

    #[test]
    fn builtin_provider_id_is_stable() {
        assert_eq!(BuiltinCpuSegmenter.provider(), "builtin-cpu");
    }

    #[test]
    fn keep_largest_drops_smaller_speck() {
        // Two blocks: a big one and a single-pixel speck. Largest-only keeps the
        // big block and drops the speck.
        let mut image = RgbaImage::from_pixel(12, 6, Rgba([120, 120, 120, 255]));
        for y in 1..5 {
            for x in 1..5 {
                image.put_pixel(x, y, Rgba([220, 20, 20, 255]));
            }
        }
        image.put_pixel(10, 3, Rgba([220, 20, 20, 255]));
        let result = BuiltinCpuSegmenter
            .segment(&SegmentRequest {
                image: &image,
                mode: AutoMode::Subject,
                placeholder: None,
                prompt: None,
                points: &[],
            })
            .unwrap();
        assert_eq!(result.mask.get_pixel(2, 2).0[0], MASK_ON);
        assert_eq!(result.mask.get_pixel(10, 3).0[0], MASK_OFF);
    }

    #[test]
    fn point_prompt_keeps_pointed_component() {
        // Two equal blocks; a point in the right block keeps only the right one.
        let mut image = RgbaImage::from_pixel(13, 6, Rgba([120, 120, 120, 255]));
        for y in 1..5 {
            for x in 1..4 {
                image.put_pixel(x, y, Rgba([220, 20, 20, 255]));
            }
            for x in 9..12 {
                image.put_pixel(x, y, Rgba([220, 20, 20, 255]));
            }
        }
        let result = BuiltinCpuSegmenter
            .segment(&SegmentRequest {
                image: &image,
                mode: AutoMode::Subject,
                placeholder: None,
                prompt: None,
                points: &[(10, 3)],
            })
            .unwrap();
        assert_eq!(result.mask.get_pixel(10, 3).0[0], MASK_ON);
        assert_eq!(result.mask.get_pixel(2, 3).0[0], MASK_OFF);
    }

    #[test]
    fn placeholder_constrains_selection() {
        let image = scene_with_block();
        // Placeholder only covers the left half; the block (centre) is partly
        // clipped, and nothing outside the placeholder survives.
        let mut placeholder = GrayImage::from_pixel(10, 10, Luma([MASK_OFF]));
        for y in 0..10 {
            for x in 0..5 {
                placeholder.put_pixel(x, y, Luma([MASK_ON]));
            }
        }
        let result = BuiltinCpuSegmenter
            .segment(&SegmentRequest {
                image: &image,
                mode: AutoMode::Subject,
                placeholder: Some(&placeholder),
                prompt: None,
                points: &[],
            })
            .unwrap();
        // x>=5 is outside the placeholder ⇒ off, even though it is red.
        assert_eq!(result.mask.get_pixel(6, 5).0[0], MASK_OFF);
        assert_eq!(result.mask.get_pixel(4, 5).0[0], MASK_ON);
    }

    #[test]
    fn flat_image_selects_nothing() {
        let image = RgbaImage::from_pixel(8, 8, Rgba([120, 120, 120, 255]));
        let result = BuiltinCpuSegmenter
            .segment(&SegmentRequest {
                image: &image,
                mode: AutoMode::Subject,
                placeholder: None,
                prompt: None,
                points: &[],
            })
            .unwrap();
        assert_eq!(coverage(&result.mask), 0.0);
        assert!(result.detected_subjects.is_empty());
    }
}
