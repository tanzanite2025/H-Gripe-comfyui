//! Phase 2 model-backed subject segmentation (ONNX Runtime, `Compute` lane).
//!
//! Implements the [`SubjectSegmenter`](super::subject_segment::SubjectSegmenter)
//! trait with real salient-object / dichotomous-segmentation models run
//! in-process via `ort`. When no model weight is resolvable the card falls back
//! to the deterministic `builtin-cpu` segmenter, so these backends are purely
//! additive.
//!
//! Models are described by a [`ModelSpec`] (input size, normalisation, weight
//! file / env override) so the load + inference path is shared. Three are
//! wired; the prompt-free `auto_*` modes pick a priority list via
//! [`model_segmenter_for_mode`]:
//! - **BiRefNet** (MIT, ~224 MB lite) — higher-quality background removal, the
//!   *downloadable big tier*; `provider: birefnet`.
//! - **U²-Netp** (Apache-2.0, ~4.6 MB) — the lightweight bundled default;
//!   `provider: u2netp`.
//! - **U²-Net human-seg** (Apache-2.0, ~168 MB) — a U²-Net trained on human
//!   segmentation; `auto_person` prefers it so a person matte tracks people
//!   rather than generic saliency. Same architecture / preprocessing as
//!   U²-Netp, so it reuses the shared path unchanged; `provider:
//!   u2net_human_seg`.
//!
//! Each model takes an RGB `1x3xSxS` input and emits a `1x1xSxS` map; the map is
//! min-max normalised then thresholded, so the same post-processing works for
//! raw-logit and sigmoid outputs. Pre/post-processing are pure functions, unit
//! tested without loading a session.
//!
//! Weight resolution per model (`resolve_weight`):
//! 1. the model's env override (e.g. `HGRIPE_BIREFNET_MODEL`) for dev / tests,
//! 2. the captured Tauri resource dir (`<resource_dir>/resources/models/`),
//! 3. next to the executable (`<exe_dir>/resources/models/`),
//! 4. the in-repo `resources/models/` dir (dev runs from a checkout).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use image::{imageops::FilterType, GrayImage, Luma, RgbaImage};
use ort::value::Tensor;
use serde_json::json;

use super::onnx_pool::{cached_session, SharedSession};
use super::subject_segment::{AutoMode, SegmentRequest, SegmentResult, SubjectSegmenter};

const MASK_ON: u8 = 255;
const MASK_OFF: u8 = 0;
const SELECTED_THRESHOLD: u8 = 128;
/// ImageNet normalisation, shared by the wired models.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];
/// Saliency past which a pixel is kept as foreground in the produced matte.
const FOREGROUND_CUTOFF: u8 = 128;

/// How input pixels are scaled to roughly `[0, 1]` before ImageNet normalising.
#[derive(Clone, Copy)]
enum Norm {
    /// Scale by the image's max channel value (`rembg` / U²-Net style).
    MaxChannel,
    /// Rescale by `1/255` (standard ImageNet preprocessing, BiRefNet).
    Rescale255,
}

/// A wired ONNX segmentation model and how to feed it.
#[derive(Clone, Copy)]
struct ModelSpec {
    /// Reported as `provider` in `matte_report`.
    provider: &'static str,
    /// Square input edge the model is trained at.
    input_size: u32,
    /// Weight filename under `resources/models/`.
    file_name: &'static str,
    /// Env var that overrides the weight path (dev / tests).
    env_var: &'static str,
    norm: Norm,
}

const U2NETP: ModelSpec = ModelSpec {
    provider: "u2netp",
    input_size: 320,
    file_name: "u2netp.onnx",
    env_var: "HGRIPE_SUBJECT_MODEL",
    norm: Norm::MaxChannel,
};

const BIREFNET: ModelSpec = ModelSpec {
    provider: "birefnet",
    input_size: 1024,
    file_name: "birefnet_lite.onnx",
    env_var: "HGRIPE_BIREFNET_MODEL",
    norm: Norm::Rescale255,
};

/// U²-Net trained for human segmentation (rembg `u2net_human_seg`). Same
/// architecture / preprocessing as [`U2NETP`], so it rides the shared path; it
/// is only *preferred* for the `auto_person` mode.
const U2NET_HUMAN: ModelSpec = ModelSpec {
    provider: "u2net_human_seg",
    input_size: 320,
    file_name: "u2net_human_seg.onnx",
    env_var: "HGRIPE_PERSON_MODEL",
    norm: Norm::MaxChannel,
};

/// Highest quality first, then the lightweight default; the caller falls back to
/// `builtin-cpu` when none resolve.
const MODEL_PRIORITY: [ModelSpec; 2] = [BIREFNET, U2NETP];

/// `auto_person` priority: the human-segmentation net first so a person matte
/// tracks people, then the generic salient models, then `builtin-cpu`.
const PERSON_PRIORITY: [ModelSpec; 3] = [U2NET_HUMAN, BIREFNET, U2NETP];

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

/// Resolve a model's weight, or `None` when it is not present anywhere.
fn resolve_weight(spec: &ModelSpec) -> Option<PathBuf> {
    resolve_model_file(spec.env_var, spec.file_name)
}

/// Resolve an ONNX weight file: env override first, then the bundled / in-repo
/// `resources/models/` locations. Shared by the single-file salient models here
/// and the two-file SAM 2 backend in [`super::subject_sam2`].
pub(super) fn resolve_model_file(env_var: &str, file_name: &str) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var(env_var) {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    let rel = Path::new("resources").join("models").join(file_name);
    if let Some(dir) = RESOURCE_DIR.get().cloned().flatten() {
        let bundled = dir.join(&rel);
        if bundled.is_file() {
            return Some(bundled);
        }
    }
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

/// An ONNX salient-object / dichotomous-segmentation model run in-process.
pub(super) struct ModelSegmenter {
    // A warm session shared from the process-wide pool; `Session::run` takes
    // `&mut self`, so the pool wraps it in a `Mutex` and inference serialises
    // through the lock (keeping the trait's `&self` signature).
    session: SharedSession,
    spec: ModelSpec,
}

impl ModelSegmenter {
    fn load(path: &Path, spec: ModelSpec) -> Result<Self, String> {
        let session = cached_session(path)?;
        Ok(Self { session, spec })
    }
}

impl SubjectSegmenter for ModelSegmenter {
    fn provider(&self) -> &str {
        self.spec.provider
    }

    fn segment(&self, request: &SegmentRequest) -> Result<SegmentResult, String> {
        let (width, height) = request.image.dimensions();
        if width == 0 || height == 0 {
            return Err("Subject Mask auto mode needs a non-empty image".to_string());
        }

        let size = self.spec.input_size;
        let input = preprocess(request.image, self.spec);
        let tensor = Tensor::from_array((vec![1_i64, 3, i64::from(size), i64::from(size)], input))
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

        let mut mask = postprocess(saliency, size, width, height);
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

/// Resize to the model's input edge and produce a CHW, ImageNet-normalised
/// `f32` buffer using the model's pixel-scaling convention.
fn preprocess(image: &RgbaImage, spec: ModelSpec) -> Vec<f32> {
    let size = spec.input_size;
    let resized = image::imageops::resize(image, size, size, FilterType::Triangle);
    let scale = match spec.norm {
        Norm::MaxChannel => resized
            .pixels()
            .flat_map(|p| p.0[..3].iter().copied())
            .max()
            .unwrap_or(0)
            .max(1) as f32,
        Norm::Rescale255 => 255.0,
    };
    let plane = (size * size) as usize;
    let mut out = vec![0f32; plane * 3];
    for (i, pixel) in resized.pixels().enumerate() {
        for c in 0..3 {
            let v = pixel.0[c] as f32 / scale;
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

pub(super) fn constrain_to_placeholder(mask: &mut GrayImage, placeholder: &GrayImage) {
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

pub(super) fn selection_bbox(mask: &GrayImage) -> Option<(u32, u32, u32, u32)> {
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

pub(super) fn coverage(mask: &GrayImage) -> f64 {
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

/// Try to build a model-backed segmenter for `mode`, preferring the highest
/// quality wired model whose weight resolves; `None` when none do (the caller
/// then uses the builtin CPU fallback). `auto_person` prefers the
/// human-segmentation net before the generic salient models; other modes use
/// the generic priority.
pub(super) fn model_segmenter_for_mode(mode: AutoMode) -> Option<ModelSegmenter> {
    for spec in priority_for(mode) {
        if let Some(path) = resolve_weight(spec) {
            if let Ok(segmenter) = ModelSegmenter::load(&path, *spec) {
                return Some(segmenter);
            }
        }
    }
    None
}

/// The wired-model priority list for an auto mode: `auto_person` leads with the
/// human-segmentation net; every other mode uses the generic salient priority.
fn priority_for(mode: AutoMode) -> &'static [ModelSpec] {
    match mode {
        AutoMode::Person => &PERSON_PRIORITY,
        _ => &MODEL_PRIORITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_shape_and_normalisation() {
        let image = RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255]));
        let data = preprocess(&image, U2NETP);
        let size = U2NETP.input_size;
        assert_eq!(data.len(), (size * size * 3) as usize);
        // Red plane: (1.0 - mean)/std; green/blue planes: (0 - mean)/std.
        let plane = (size * size) as usize;
        let r = data[0];
        let g = data[plane];
        assert!((r - (1.0 - MEAN[0]) / STD[0]).abs() < 1e-3, "r={r}");
        assert!((g - (0.0 - MEAN[1]) / STD[1]).abs() < 1e-3, "g={g}");
    }

    #[test]
    fn preprocess_norm_differs_by_spec() {
        // A flat mid-grey: MaxChannel scales by the max (128 -> 1.0); Rescale255
        // divides by 255 (128 -> ~0.502). The normalised values must differ.
        let grey = RgbaImage::from_pixel(8, 8, image::Rgba([128, 128, 128, 255]));
        let max_channel = preprocess(&grey, U2NETP)[0];
        let rescale = preprocess(&grey, BIREFNET)[0];
        let expect_max = (1.0 - MEAN[0]) / STD[0];
        let expect_rescale = (128.0 / 255.0 - MEAN[0]) / STD[0];
        assert!((max_channel - expect_max).abs() < 1e-3, "max={max_channel}");
        assert!((rescale - expect_rescale).abs() < 1e-3, "rescale={rescale}");
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
    fn resolve_ignores_missing_env_override() {
        // A bogus path is ignored (not a file); the resolver returns either a
        // real weight or None, never panics.
        std::env::set_var(U2NETP.env_var, "Z:/definitely/missing.onnx");
        let _ = resolve_weight(&U2NETP);
        std::env::remove_var(U2NETP.env_var);
    }

    /// End-to-end real inference for a given model, only when its weight is
    /// resolvable. Skipped otherwise so CI without the weight still passes.
    fn inference_smoke(spec: ModelSpec) {
        let Some(path) = resolve_weight(&spec) else {
            eprintln!("skipping {}: no weight resolvable", spec.provider);
            return;
        };
        let segmenter = ModelSegmenter::load(&path, spec).expect("load model");
        assert_eq!(segmenter.provider(), spec.provider);
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

    #[test]
    fn u2netp_inference_when_weight_present() {
        inference_smoke(U2NETP);
    }

    #[test]
    fn birefnet_inference_when_weight_present() {
        inference_smoke(BIREFNET);
    }

    #[test]
    fn person_mode_prefers_human_seg_net() {
        // auto_person leads with the human-segmentation net before the generic
        // salient models, so a person matte tracks people rather than saliency.
        let person: Vec<&str> = priority_for(AutoMode::Person)
            .iter()
            .map(|s| s.provider)
            .collect();
        assert_eq!(person, ["u2net_human_seg", "birefnet", "u2netp"]);
    }

    #[test]
    fn non_person_modes_keep_generic_priority() {
        // Every other auto mode uses the generic salient priority unchanged
        // (the human-seg net is person-only).
        for mode in [
            AutoMode::Subject,
            AutoMode::Product,
            AutoMode::TransparentObject,
        ] {
            let providers: Vec<&str> = priority_for(mode).iter().map(|s| s.provider).collect();
            assert_eq!(providers, ["birefnet", "u2netp"]);
        }
    }

    #[test]
    fn human_seg_inference_when_weight_present() {
        inference_smoke(U2NET_HUMAN);
    }

    #[test]
    fn warm_pool_reuses_session_across_loads() {
        // Two segmenters built from the same weight must share one warm
        // session (the whole point of step 3: no per-call model reload).
        // Skipped when the weight isn't resolvable, like the inference smokes.
        let Some(path) = resolve_weight(&U2NETP) else {
            eprintln!("skipping warm-pool reuse: no u2netp weight resolvable");
            return;
        };
        let first = ModelSegmenter::load(&path, U2NETP).expect("load first");
        let second = ModelSegmenter::load(&path, U2NETP).expect("load second");
        assert!(
            std::sync::Arc::ptr_eq(&first.session, &second.session),
            "the same weight path must hand back the same warm session"
        );
    }
}
