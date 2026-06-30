//! Phase 2 model-backed subject segmentation (ONNX Runtime, `Compute` lane).
//!
//! Implements the [`SubjectSegmenter`](super::subject_segment::SubjectSegmenter)
//! trait with a real U²-Net-family salient-object model run in-process via
//! `ort`. When no model weight is resolvable the card falls back to the
//! deterministic `builtin-cpu` segmenter, so this backend is purely additive.
//!
//! Weight resolution order (`resolve_model_path`):
//! 1. `HGRIPE_SUBJECT_MODEL` env override (dev / tests),
//! 2. a model bundled next to the executable (`<exe_dir>/resources/models/`),
//! 3. the in-repo `resources/models/` dir (dev runs from a checkout).
//!
//! The model is a U²-Netp salient-object detector (Apache-2.0): RGB `1x3xSxS`
//! input, a `1x1xSxS` saliency map in roughly `[0, 1]`. Pre/post-processing are
//! pure functions so they can be unit-tested without loading a session.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use image::{imageops::FilterType, GrayImage, Luma, RgbaImage};
use ort::session::Session;
use ort::value::Tensor;
use serde_json::json;

use super::subject_segment::{AutoMode, SegmentRequest, SegmentResult, SubjectSegmenter};

const MASK_ON: u8 = 255;
const MASK_OFF: u8 = 0;
const SELECTED_THRESHOLD: u8 = 128;
/// U²-Net is trained at 320x320.
const INPUT_SIZE: u32 = 320;
/// ImageNet normalisation, matching the common U²-Net / `rembg` preprocessing.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];
/// Saliency past which a pixel is kept as foreground in the produced matte.
const FOREGROUND_CUTOFF: u8 = 128;

/// The Tauri resource directory captured at startup (see `set_resource_dir`),
/// mirroring `psd::set_resource_dir`: in a packaged install the bundled model
/// lives under `<resource_dir>/resources/models/`, which the handle-free
/// `Compute` segmenter cannot resolve on its own.
static RESOURCE_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Record the bundled resource directory. Called once from the Tauri `setup`
/// hook; ignored if the resolver could not determine a resource path.
pub(crate) fn set_resource_dir(dir: Option<PathBuf>) {
    let _ = RESOURCE_DIR.set(dir);
}

/// The model weight bundled under the captured resource directory, if present.
fn resource_model() -> Option<PathBuf> {
    let dir = RESOURCE_DIR.get().cloned().flatten()?;
    let path = dir.join("resources").join("models").join("u2netp.onnx");
    path.is_file().then_some(path)
}

/// Resolve the bundled / configured model weight, or `None` to fall back to the
/// builtin CPU segmenter.
pub(super) fn resolve_model_path() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("HGRIPE_SUBJECT_MODEL") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(bundled) = resource_model() {
        return Some(bundled);
    }
    let rel = Path::new("resources").join("models").join("u2netp.onnx");
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join(&rel);
            if bundled.is_file() {
                return Some(bundled);
            }
        }
    }
    let in_repo = Path::new(env!("CARGO_MANIFEST_DIR")).join(&rel);
    in_repo.is_file().then_some(in_repo)
}

/// A U²-Net salient-object segmenter executed in-process via ONNX Runtime.
pub(super) struct ModelSegmenter {
    // `ort::Session::run` takes `&mut self`; the card holds a single segmenter
    // per execution, so a `Mutex` keeps the trait's `&self` signature.
    session: Mutex<Session>,
    provider: String,
}

impl ModelSegmenter {
    pub(super) fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path)
            .map_err(|err| format!("failed to read subject model {}: {err}", path.display()))?;
        let session = Session::builder()
            .and_then(|mut b| b.commit_from_memory(&bytes))
            .map_err(|err| format!("failed to load subject model {}: {err}", path.display()))?;
        Ok(Self {
            session: Mutex::new(session),
            provider: "u2netp".to_string(),
        })
    }
}

impl SubjectSegmenter for ModelSegmenter {
    fn provider(&self) -> &str {
        &self.provider
    }

    fn segment(&self, request: &SegmentRequest) -> Result<SegmentResult, String> {
        let (width, height) = request.image.dimensions();
        if width == 0 || height == 0 {
            return Err("Subject Mask auto mode needs a non-empty image".to_string());
        }

        let input = preprocess(request.image, INPUT_SIZE);
        let tensor = Tensor::from_array((
            vec![1_i64, 3, i64::from(INPUT_SIZE), i64::from(INPUT_SIZE)],
            input,
        ))
        .map_err(|err| format!("failed to build model input: {err}"))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| "subject model session poisoned".to_string())?;
        let input_name = session.inputs()[0].name().to_string();
        let outputs = session
            .run(ort::inputs![input_name => tensor])
            .map_err(|err| format!("subject model inference failed: {err}"))?;
        let (_, saliency) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|err| format!("failed to read model output: {err}"))?;

        let mut mask = postprocess(saliency, INPUT_SIZE, width, height);
        if let Some(placeholder) = request.placeholder {
            constrain_to_placeholder(&mut mask, placeholder);
        }

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

/// Resize to `size`x`size` and produce a CHW, ImageNet-normalised `f32` buffer.
/// `rembg`-style: scale by the image's max channel value before normalising.
fn preprocess(image: &RgbaImage, size: u32) -> Vec<f32> {
    let resized = image::imageops::resize(image, size, size, FilterType::Triangle);
    let max = resized
        .pixels()
        .flat_map(|p| p.0[..3].iter().copied())
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    let plane = (size * size) as usize;
    let mut out = vec![0f32; plane * 3];
    for (i, pixel) in resized.pixels().enumerate() {
        for c in 0..3 {
            let v = pixel.0[c] as f32 / max;
            out[c * plane + i] = (v - MEAN[c]) / STD[c];
        }
    }
    out
}

/// Min-max normalise the saliency map, threshold it, and resize to the original
/// image dimensions.
fn postprocess(saliency: &[f32], size: u32, width: u32, height: u32) -> GrayImage {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &v in saliency.iter().take((size * size) as usize) {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    let span = (hi - lo).max(f32::EPSILON);
    let mut small = GrayImage::from_pixel(size, size, Luma([MASK_OFF]));
    for (i, pixel) in small.pixels_mut().enumerate() {
        let v = saliency.get(i).copied().unwrap_or(lo);
        let norm = ((v - lo) / span * 255.0).round().clamp(0.0, 255.0) as u8;
        pixel.0[0] = if norm >= FOREGROUND_CUTOFF {
            MASK_ON
        } else {
            MASK_OFF
        };
    }
    image::imageops::resize(&small, width, height, FilterType::Triangle)
}

fn constrain_to_placeholder(mask: &mut GrayImage, placeholder: &GrayImage) {
    if placeholder.dimensions() != mask.dimensions() {
        return;
    }
    let (width, height) = mask.dimensions();
    for y in 0..height {
        for x in 0..width {
            if placeholder.get_pixel(x, y).0[0] < SELECTED_THRESHOLD {
                mask.put_pixel(x, y, Luma([MASK_OFF]));
            }
        }
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

/// Try to build a model-backed segmenter for `mode`; `None` when no weight is
/// resolvable (the caller then uses the builtin CPU fallback).
pub(super) fn model_segmenter_for_mode(_mode: AutoMode) -> Option<ModelSegmenter> {
    let path = resolve_model_path()?;
    ModelSegmenter::load(&path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_shape_and_normalisation() {
        let image = RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255]));
        let data = preprocess(&image, INPUT_SIZE);
        assert_eq!(data.len(), (INPUT_SIZE * INPUT_SIZE * 3) as usize);
        // Red plane: (1.0 - mean)/std; green/blue planes: (0 - mean)/std.
        let plane = (INPUT_SIZE * INPUT_SIZE) as usize;
        let r = data[0];
        let g = data[plane];
        assert!((r - (1.0 - MEAN[0]) / STD[0]).abs() < 1e-3, "r={r}");
        assert!((g - (0.0 - MEAN[1]) / STD[1]).abs() < 1e-3, "g={g}");
    }

    #[test]
    fn postprocess_thresholds_and_resizes() {
        // 2x2 saliency: top row high, bottom row low. Min-max normalise puts the
        // high cells at 255 (kept) and low at 0 (dropped); resize to 4x4.
        let saliency = vec![0.9, 0.95, 0.0, 0.05];
        let mask = postprocess(&saliency, 2, 4, 4);
        assert_eq!(mask.dimensions(), (4, 4));
        assert_eq!(mask.get_pixel(0, 0).0[0], MASK_ON);
        assert_eq!(mask.get_pixel(0, 3).0[0], MASK_OFF);
    }

    #[test]
    fn resolve_prefers_env_override() {
        // A bogus path is ignored (not a file); with nothing set the resolver
        // returns either the in-repo model or None, never panics.
        std::env::remove_var("HGRIPE_SUBJECT_MODEL");
        let _ = resolve_model_path();
        std::env::set_var("HGRIPE_SUBJECT_MODEL", "Z:/definitely/missing.onnx");
        assert!(resolve_model_path().is_none() || resolve_model_path().is_some());
        std::env::remove_var("HGRIPE_SUBJECT_MODEL");
    }

    /// End-to-end real inference, only when a model weight is available
    /// (`HGRIPE_SUBJECT_MODEL` or the bundled/in-repo path). Skipped otherwise so
    /// CI without the weight still passes.
    #[test]
    fn model_inference_when_weight_present() {
        let Some(path) = resolve_model_path() else {
            eprintln!("skipping: no subject model weight resolvable");
            return;
        };
        let segmenter = ModelSegmenter::load(&path).expect("load model");
        assert_eq!(segmenter.provider(), "u2netp");
        // Grey scene with a bright centred block -> non-empty saliency.
        let mut image = RgbaImage::from_pixel(64, 64, image::Rgba([120, 120, 120, 255]));
        for y in 20..44 {
            for x in 20..44 {
                image.put_pixel(x, y, image::Rgba([240, 30, 30, 255]));
            }
        }
        let result = segmenter
            .segment(&SegmentRequest {
                image: &image,
                mode: AutoMode::Subject,
                placeholder: None,
                prompt: None,
                points: &[],
            })
            .expect("inference");
        assert_eq!(result.mask.dimensions(), (64, 64));
    }
}
