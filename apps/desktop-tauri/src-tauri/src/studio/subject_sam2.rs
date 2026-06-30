//! Phase 2 SAM 2 interactive point-prompt segmentation (ONNX Runtime, `Compute`
//! lane).
//!
//! SAM 2 (Apache-2.0) segments *what the user pointed at* rather than guessing
//! the most salient subject. Unlike the prompt-free salient models in
//! [`super::subject_model`] (BiRefNet / U²-Netp), it consumes the node's
//! click-to-select points, so [`segmenter_for_mode`](super::subject_segment)
//! prefers it only when the request carries point prompts *and* both weights
//! resolve; otherwise the salient / builtin pipeline runs, making this backend
//! purely additive.
//!
//! It is a two-stage model run in-process via `ort`:
//! 1. an **image encoder** (`image` `1x3x1024x1024`) → `image_embed`
//!    `1x256x64x64` plus two high-resolution feature maps, computed once; and
//! 2. a **mask decoder** that turns those embeddings + point prompts into a set
//!    of candidate masks with IoU scores; the highest-IoU mask is kept,
//!    thresholded at logit `0`, and resized to the original image.
//!
//! Weights are the *downloadable big tier* (encoder ~134 MB, decoder ~20 MB);
//! they are never committed to git and are resolved via
//! [`resolve_model_file`](super::subject_model::resolve_model_file)
//! (env override → bundled `resources/models/`). `scripts/fetch-sam2.*` fetch
//! them with a sha256 check.

use std::cmp::Ordering;
use std::path::Path;
use std::sync::Mutex;

use image::{imageops::FilterType, GrayImage, Luma, RgbaImage};
use ort::session::Session;
use ort::value::Tensor;
use serde_json::json;

use super::subject_model::{
    constrain_to_placeholder, coverage, resolve_model_file, selection_bbox,
};
use super::subject_segment::{SegmentRequest, SegmentResult, SubjectSegmenter};

const PROVIDER: &str = "sam2";
/// SAM 2 is trained at 1024x1024.
const INPUT_SIZE: u32 = 1024;
/// ImageNet normalisation; SAM 2 preprocessing rescales `1/255` then normalises.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];
/// Edge of the low-resolution mask the decoder seeds from (a zeroed prior).
const MASK_PRIOR_SIZE: i64 = 256;
const MASK_ON: u8 = 255;
const MASK_OFF: u8 = 0;
/// A positive (foreground) point prompt for the decoder.
const LABEL_FOREGROUND: f32 = 1.0;
/// SAM mask logits above this are foreground.
const MASK_LOGIT_CUTOFF: f32 = 0.0;

const ENCODER_FILE: &str = "sam2_tiny.encoder.onnx";
const DECODER_FILE: &str = "sam2_tiny.decoder.onnx";
const ENCODER_ENV: &str = "HGRIPE_SAM2_ENCODER";
const DECODER_ENV: &str = "HGRIPE_SAM2_DECODER";

/// SAM 2 image encoder + mask decoder held together; `ort::Session::run` takes
/// `&mut self`, so each session is `Mutex`-wrapped to keep the trait's `&self`.
pub(super) struct Sam2Segmenter {
    encoder: Mutex<Session>,
    decoder: Mutex<Session>,
}

impl Sam2Segmenter {
    /// Build a SAM 2 segmenter when *both* the encoder and decoder weights
    /// resolve; `None` otherwise (the caller falls through to the salient /
    /// builtin pipeline).
    pub(super) fn resolve_and_load() -> Option<Self> {
        let encoder = resolve_model_file(ENCODER_ENV, ENCODER_FILE)?;
        let decoder = resolve_model_file(DECODER_ENV, DECODER_FILE)?;
        Self::load(&encoder, &decoder).ok()
    }

    fn load(encoder: &Path, decoder: &Path) -> Result<Self, String> {
        Ok(Self {
            encoder: Mutex::new(load_session(encoder)?),
            decoder: Mutex::new(load_session(decoder)?),
        })
    }
}

fn load_session(path: &Path) -> Result<Session, String> {
    let bytes = std::fs::read(path)
        .map_err(|err| format!("failed to read SAM2 model {}: {err}", path.display()))?;
    Session::builder()
        .and_then(|mut b| b.commit_from_memory(&bytes))
        .map_err(|err| format!("failed to load SAM2 model {}: {err}", path.display()))
}

impl SubjectSegmenter for Sam2Segmenter {
    fn provider(&self) -> &str {
        PROVIDER
    }

    fn segment(&self, request: &SegmentRequest) -> Result<SegmentResult, String> {
        let (width, height) = request.image.dimensions();
        if width == 0 || height == 0 {
            return Err("Subject Mask auto mode needs a non-empty image".to_string());
        }

        // Stage 1: encode the resized image once into embeddings + features.
        let pixels = preprocess(request.image);
        let image_tensor = Tensor::from_array((
            vec![1_i64, 3, i64::from(INPUT_SIZE), i64::from(INPUT_SIZE)],
            pixels,
        ))
        .map_err(|err| format!("failed to build SAM2 image input: {err}"))?;

        let (image_embed, high_res_0, high_res_1) = {
            let mut encoder = self
                .encoder
                .lock()
                .map_err(|_| "SAM2 encoder session poisoned".to_string())?;
            let input_name = encoder.inputs()[0].name().to_string();
            let outputs = encoder
                .run(ort::inputs![input_name => image_tensor])
                .map_err(|err| format!("SAM2 image encoding failed: {err}"))?;
            // Copy the borrowed outputs into owned tensors before `outputs`
            // (and the lock) drop, so the decoder can consume them.
            let take = |name: &str| -> Result<Tensor<f32>, String> {
                let (shape, data) = outputs[name]
                    .try_extract_tensor::<f32>()
                    .map_err(|err| format!("failed to read SAM2 encoder output {name}: {err}"))?;
                Tensor::from_array((shape.to_vec(), data.to_vec()))
                    .map_err(|err| format!("failed to rebuild SAM2 tensor {name}: {err}"))
            };
            (
                take("image_embed")?,
                take("high_res_feats_0")?,
                take("high_res_feats_1")?,
            )
        };

        // Stage 2: turn point prompts into a mask. Points arrive in original
        // image space and are scaled into the 1024 encoder space; with none we
        // probe the image centre so the backend never silently no-ops.
        let prompts: Vec<(u32, u32)> = if request.points.is_empty() {
            vec![(width / 2, height / 2)]
        } else {
            request.points.to_vec()
        };
        let num_points = prompts.len() as i64;
        let mut coords = Vec::with_capacity(prompts.len() * 2);
        for &(x, y) in &prompts {
            coords.push(x as f32 * INPUT_SIZE as f32 / width as f32);
            coords.push(y as f32 * INPUT_SIZE as f32 / height as f32);
        }
        let labels = vec![LABEL_FOREGROUND; prompts.len()];

        let point_coords = Tensor::from_array((vec![1_i64, num_points, 2], coords))
            .map_err(|err| format!("failed to build SAM2 point_coords: {err}"))?;
        let point_labels = Tensor::from_array((vec![1_i64, num_points], labels))
            .map_err(|err| format!("failed to build SAM2 point_labels: {err}"))?;
        let mask_prior = vec![0f32; (MASK_PRIOR_SIZE * MASK_PRIOR_SIZE) as usize];
        let mask_input =
            Tensor::from_array((vec![1_i64, 1, MASK_PRIOR_SIZE, MASK_PRIOR_SIZE], mask_prior))
                .map_err(|err| format!("failed to build SAM2 mask_input: {err}"))?;
        let has_mask_input = Tensor::from_array((vec![1_i64], vec![0f32]))
            .map_err(|err| format!("failed to build SAM2 has_mask_input: {err}"))?;

        let mut mask = {
            let mut decoder = self
                .decoder
                .lock()
                .map_err(|_| "SAM2 decoder session poisoned".to_string())?;
            let outputs = decoder
                .run(ort::inputs![
                    "image_embed" => image_embed,
                    "high_res_feats_0" => high_res_0,
                    "high_res_feats_1" => high_res_1,
                    "point_coords" => point_coords,
                    "point_labels" => point_labels,
                    "mask_input" => mask_input,
                    "has_mask_input" => has_mask_input,
                ])
                .map_err(|err| format!("SAM2 mask decoding failed: {err}"))?;
            let (mask_shape, mask_data) = outputs["masks"]
                .try_extract_tensor::<f32>()
                .map_err(|err| format!("failed to read SAM2 masks: {err}"))?;
            let (_, iou) = outputs["iou_predictions"]
                .try_extract_tensor::<f32>()
                .map_err(|err| format!("failed to read SAM2 iou_predictions: {err}"))?;
            best_mask(mask_shape, mask_data, iou, width, height)?
        };

        if let Some(placeholder) = request.placeholder {
            constrain_to_placeholder(&mut mask, placeholder);
        }

        let detected_subjects = match selection_bbox(&mask) {
            Some((x0, y0, x1, y1)) => vec![json!({
                "label": request.mode.label(),
                "prompt": request.prompt.unwrap_or(""),
                "bbox": [x0, y0, x1 - x0 + 1, y1 - y0 + 1],
                "coverage": coverage(&mask),
                "provider": PROVIDER,
            })],
            None => Vec::new(),
        };

        Ok(SegmentResult {
            mask,
            detected_subjects,
        })
    }
}

/// Resize to the encoder edge and produce a CHW, `1/255`-rescaled,
/// ImageNet-normalised `f32` buffer.
fn preprocess(image: &RgbaImage) -> Vec<f32> {
    let resized = image::imageops::resize(image, INPUT_SIZE, INPUT_SIZE, FilterType::Triangle);
    let plane = (INPUT_SIZE * INPUT_SIZE) as usize;
    let mut out = vec![0f32; plane * 3];
    for (i, pixel) in resized.pixels().enumerate() {
        for c in 0..3 {
            let v = pixel.0[c] as f32 / 255.0;
            out[c * plane + i] = (v - MEAN[c]) / STD[c];
        }
    }
    out
}

/// Pick the highest-IoU candidate from the decoder's multi-mask output,
/// threshold its logits, and resize to the original image dimensions. The
/// decoder emits `masks` as `[1, num_masks, mh, mw]` and `iou` as
/// `[1, num_masks]`.
fn best_mask(
    mask_shape: &[i64],
    mask_data: &[f32],
    iou: &[f32],
    width: u32,
    height: u32,
) -> Result<GrayImage, String> {
    if mask_shape.len() != 4 {
        return Err(format!("unexpected SAM2 masks rank {}", mask_shape.len()));
    }
    let num_masks = mask_shape[1].max(0) as usize;
    let mh = mask_shape[2].max(0) as u32;
    let mw = mask_shape[3].max(0) as u32;
    let plane = (mh * mw) as usize;
    if num_masks == 0 || plane == 0 {
        return Err("SAM2 produced an empty mask".to_string());
    }

    let best = (0..num_masks)
        .max_by(|&a, &b| {
            iou.get(a)
                .copied()
                .unwrap_or(f32::NEG_INFINITY)
                .partial_cmp(&iou.get(b).copied().unwrap_or(f32::NEG_INFINITY))
                .unwrap_or(Ordering::Equal)
        })
        .unwrap_or(0);
    let offset = best * plane;

    let mut small = GrayImage::from_pixel(mw, mh, Luma([MASK_OFF]));
    for (i, pixel) in small.pixels_mut().enumerate() {
        let logit = mask_data
            .get(offset + i)
            .copied()
            .unwrap_or(f32::NEG_INFINITY);
        pixel.0[0] = if logit > MASK_LOGIT_CUTOFF {
            MASK_ON
        } else {
            MASK_OFF
        };
    }
    // Nearest keeps the prompt mask crisply binary through the upscale.
    Ok(image::imageops::resize(
        &small,
        width,
        height,
        FilterType::Nearest,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn best_mask_picks_highest_iou_and_resizes() {
        // Two 2x2 candidate masks. Candidate 0 is empty; candidate 1 has its
        // top row positive. The higher IoU (candidate 1) must be chosen and
        // resized (nearest) to 4x4: top half on, bottom half off.
        let masks = vec![
            -1.0, -1.0, -1.0, -1.0, // candidate 0 (all background)
            5.0, 5.0, -5.0, -5.0, // candidate 1 (top row foreground)
        ];
        let shape = [1_i64, 2, 2, 2];
        let iou = [0.1_f32, 0.9];
        let mask = best_mask(&shape, &masks, &iou, 4, 4).unwrap();
        assert_eq!(mask.dimensions(), (4, 4));
        assert_eq!(mask.get_pixel(0, 0).0[0], MASK_ON);
        assert_eq!(mask.get_pixel(3, 3).0[0], MASK_OFF);
    }

    #[test]
    fn best_mask_rejects_empty() {
        assert!(best_mask(&[1, 0, 0, 0], &[], &[], 4, 4).is_err());
    }

    #[test]
    fn preprocess_shape_and_rescale() {
        let image = RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255]));
        let data = preprocess(&image);
        assert_eq!(data.len(), (INPUT_SIZE * INPUT_SIZE * 3) as usize);
        let plane = (INPUT_SIZE * INPUT_SIZE) as usize;
        // Red plane: (1.0 - mean)/std; green plane: (0 - mean)/std.
        assert!((data[0] - (1.0 - MEAN[0]) / STD[0]).abs() < 1e-3);
        assert!((data[plane] - (0.0 - MEAN[1]) / STD[1]).abs() < 1e-3);
    }

    /// Real two-stage inference, only when both weights resolve. Skipped
    /// otherwise so CI without the weights still passes.
    #[test]
    fn sam2_inference_when_weights_present() {
        let Some(segmenter) = Sam2Segmenter::resolve_and_load() else {
            eprintln!("skipping sam2: encoder/decoder weights not resolvable");
            return;
        };
        assert_eq!(segmenter.provider(), "sam2");
        // Grey scene with a bright centred block; a point inside it should
        // yield a non-empty, full-resolution mask.
        let mut image = RgbaImage::from_pixel(64, 64, Rgba([120, 120, 120, 255]));
        for y in 20..44 {
            for x in 20..44 {
                image.put_pixel(x, y, Rgba([240, 30, 30, 255]));
            }
        }
        let result = segmenter
            .segment(&SegmentRequest {
                image: &image,
                mode: super::super::subject_segment::AutoMode::Subject,
                placeholder: None,
                prompt: None,
                points: &[(32, 32)],
            })
            .expect("sam2 inference");
        assert_eq!(result.mask.dimensions(), (64, 64));
    }
}
