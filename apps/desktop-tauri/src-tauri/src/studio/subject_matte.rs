//! Cascade 3 / Phase 4: continuous **alpha matting** for the `subjectMask` card
//! (ONNX Runtime, `Compute` lane).
//!
//! Segmentation (`subject_segment` / `subject_model` / `subject_sam2`) answers
//! *which pixels are the subject* and yields a hard, binary matte. Matting
//! answers *how opaque each edge pixel is* — the continuous alpha needed for
//! hair, fur, glass and other translucent/whispy edges — and hands a soft matte
//! to `Refine Mask Edge`.
//!
//! The interaction is the classic trimap pipeline:
//! 1. [`trimap_from_mask`] turns the binary matte into a three-level trimap by
//!    eroding it (definite foreground), dilating it (everything past the dilate
//!    is definite background), and marking the band between as *unknown*.
//! 2. an [`AlphaMatter`] resolves the unknown band into continuous alpha.
//!
//! Two backends implement the same trait, chosen by [`matter`]:
//! - **ViTMatte** (`provider: vitmatte`, downloadable big tier ~104 MB) — a real
//!   matting transformer run in-process via `ort`; takes the RGB image *and* the
//!   trimap as one 4-channel `pixel_values` tensor and emits `alphas`.
//! - **`builtin-cpu-matte`** — a deterministic, weight-free fallback that feathers
//!   the binary edge through the unknown band, so the feature works end-to-end
//!   before any weight is present (and keeps CI green without the blob).
//!
//! The weight is never committed to git; it resolves via
//! [`resolve_model_file`](super::subject_model::resolve_model_file) (env override
//! → bundled `resources/models/`). `scripts/fetch-vitmatte.*` fetch it with a
//! sha256 check.

use image::{imageops::FilterType, GrayImage, Luma, RgbaImage};
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;
use std::sync::Mutex;

use super::subject_model::resolve_model_file;

/// Trimap level for *definite foreground* (kept fully opaque).
pub(super) const TRIMAP_FG: u8 = 255;
/// Trimap level for the *unknown* band the matter resolves.
pub(super) const TRIMAP_UNKNOWN: u8 = 128;
/// Trimap level for *definite background* (kept fully transparent).
pub(super) const TRIMAP_BG: u8 = 0;
/// A mask pixel at least half-opaque counts as foreground when building the
/// trimap (mirrors `subject_mask::SELECTED_THRESHOLD`).
const SELECTED_THRESHOLD: u8 = 128;

/// ViTMatte is trained at a multiple of 32; we run at a fixed square edge and
/// resize the alpha back, bounding memory like the other model backends.
const INPUT_SIZE: u32 = 1024;
/// `VitMatteImageProcessor`: rescale `1/255` then normalise the three image
/// channels with mean/std `0.5` (so RGB lands in `[-1, 1]`); the trimap channel
/// is only rescaled `1/255`.
const IMAGE_MEAN: f32 = 0.5;
const IMAGE_STD: f32 = 0.5;

const MODEL_FILE: &str = "vitmatte.onnx";
const MODEL_ENV: &str = "HGRIPE_VITMATTE_MODEL";
const VITMATTE_PROVIDER: &str = "vitmatte";
const BUILTIN_PROVIDER: &str = "builtin-cpu-matte";

/// Resolve continuous alpha for the unknown band of a trimap. ViTMatte and the
/// deterministic builtin fallback both implement this; [`matter`] picks one.
pub(super) trait AlphaMatter {
    /// The id recorded in `matte_report` for the matting op.
    fn provider(&self) -> &str;
    /// Produce a full-resolution continuous-alpha matte from the image and its
    /// trimap (both at the original image size).
    fn matte(&self, image: &RgbaImage, trimap: &GrayImage) -> Result<GrayImage, String>;
}

/// Pick the alpha matter: ViTMatte when its weight resolves, else the
/// deterministic builtin fallback so the feature always works.
pub(super) fn matter() -> Box<dyn AlphaMatter> {
    if let Some(path) = resolve_model_file(MODEL_ENV, MODEL_FILE) {
        if let Ok(vitmatte) = VitMatteMatter::load(&path) {
            return Box::new(vitmatte);
        }
    }
    Box::new(BuiltinCpuMatter)
}

/// Build a three-level trimap from a binary matte: erode by `band` for the
/// definite-foreground core, dilate by `band` for the definite-background
/// exterior, and mark the ring between as unknown. `band == 0` yields a pure
/// pass-through trimap (no unknown ring), so matting is a no-op.
pub(super) fn trimap_from_mask(mask: &GrayImage, band: u32) -> GrayImage {
    let (width, height) = mask.dimensions();
    let inner = super::subject_mask::erode(mask, band);
    let outer = super::subject_mask::dilate(mask, band);
    let mut trimap = GrayImage::from_pixel(width, height, Luma([TRIMAP_BG]));
    for y in 0..height {
        for x in 0..width {
            let level = if inner.get_pixel(x, y).0[0] >= SELECTED_THRESHOLD {
                TRIMAP_FG
            } else if outer.get_pixel(x, y).0[0] >= SELECTED_THRESHOLD {
                TRIMAP_UNKNOWN
            } else {
                TRIMAP_BG
            };
            trimap.put_pixel(x, y, Luma([level]));
        }
    }
    trimap
}

/// ViTMatte matting transformer run in-process via `ort`. `ort::Session::run`
/// takes `&mut self`, so the session is `Mutex`-wrapped to keep the trait's
/// `&self`.
pub(super) struct VitMatteMatter {
    session: Mutex<Session>,
}

impl VitMatteMatter {
    fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path)
            .map_err(|err| format!("failed to read ViTMatte model {}: {err}", path.display()))?;
        let session = Session::builder()
            .and_then(|mut b| b.commit_from_memory(&bytes))
            .map_err(|err| format!("failed to load ViTMatte model {}: {err}", path.display()))?;
        Ok(Self {
            session: Mutex::new(session),
        })
    }
}

impl AlphaMatter for VitMatteMatter {
    fn provider(&self) -> &str {
        VITMATTE_PROVIDER
    }

    fn matte(&self, image: &RgbaImage, trimap: &GrayImage) -> Result<GrayImage, String> {
        let (width, height) = image.dimensions();
        if width == 0 || height == 0 {
            return Err("Subject Mask matting needs a non-empty image".to_string());
        }

        let pixels = preprocess(image, trimap);
        let tensor = Tensor::from_array((
            vec![1_i64, 4, i64::from(INPUT_SIZE), i64::from(INPUT_SIZE)],
            pixels,
        ))
        .map_err(|err| format!("failed to build ViTMatte input: {err}"))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| "ViTMatte session poisoned".to_string())?;
        let input_name = session.inputs()[0].name().to_string();
        let outputs = session
            .run(ort::inputs![input_name => tensor])
            .map_err(|err| format!("ViTMatte inference failed: {err}"))?;
        let (_, alphas) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|err| format!("failed to read ViTMatte output: {err}"))?;

        Ok(postprocess(alphas, INPUT_SIZE, width, height))
    }
}

/// Resize image + trimap to the model edge and pack them into a CHW
/// `[R, G, B, trimap]` 4-channel buffer: RGB rescaled `1/255` and `0.5`/`0.5`
/// normalised to `[-1, 1]`, the trimap channel rescaled `1/255` only.
fn preprocess(image: &RgbaImage, trimap: &GrayImage) -> Vec<f32> {
    let size = INPUT_SIZE;
    let rgb = image::imageops::resize(image, size, size, FilterType::Triangle);
    let tri = image::imageops::resize(trimap, size, size, FilterType::Triangle);
    let plane = (size * size) as usize;
    let mut out = vec![0f32; plane * 4];
    for (i, pixel) in rgb.pixels().enumerate() {
        for c in 0..3 {
            let v = pixel.0[c] as f32 / 255.0;
            out[c * plane + i] = (v - IMAGE_MEAN) / IMAGE_STD;
        }
    }
    for (i, pixel) in tri.pixels().enumerate() {
        out[3 * plane + i] = pixel.0[0] as f32 / 255.0;
    }
    out
}

/// Clamp the model's `[0, 1]` alpha to 8-bit and resize back to the original
/// image dimensions.
fn postprocess(alphas: &[f32], size: u32, width: u32, height: u32) -> GrayImage {
    let mut small = GrayImage::from_pixel(size, size, Luma([0]));
    for (i, pixel) in small.pixels_mut().enumerate() {
        let v = alphas.get(i).copied().unwrap_or(0.0);
        pixel.0[0] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    image::imageops::resize(&small, width, height, FilterType::Triangle)
}

/// A deterministic, weight-free matter: keep the definite foreground opaque and
/// the definite background transparent, and feather the binary edge through the
/// unknown band by Gaussian-blurring the mask and reading the blurred value
/// there. Soft, reproducible alpha without a model.
pub(super) struct BuiltinCpuMatter;

impl AlphaMatter for BuiltinCpuMatter {
    fn provider(&self) -> &str {
        BUILTIN_PROVIDER
    }

    fn matte(&self, _image: &RgbaImage, trimap: &GrayImage) -> Result<GrayImage, String> {
        let (width, height) = trimap.dimensions();
        if width == 0 || height == 0 {
            return Err("Subject Mask matting needs a non-empty image".to_string());
        }
        // Reconstruct the binary matte the trimap came from (FG ∪ unknown for a
        // soft edge core) and blur it; the blur radius scales with the unknown
        // band so the feather fills it.
        let mut binary = GrayImage::from_pixel(width, height, Luma([0]));
        let mut radius = 1u32;
        for y in 0..height {
            for x in 0..width {
                let level = trimap.get_pixel(x, y).0[0];
                if level == TRIMAP_FG {
                    binary.put_pixel(x, y, Luma([255]));
                }
            }
        }
        // Estimate the unknown-band width from the trimap so the feather covers
        // it (fall back to a 1px blur when there is no unknown ring).
        let unknown = trimap
            .pixels()
            .filter(|p| p.0[0] == TRIMAP_UNKNOWN)
            .count();
        if unknown > 0 {
            radius = ((unknown as f64).sqrt() / 4.0).round().max(1.0) as u32;
        }
        let blurred = image::imageops::blur(&binary, radius as f32);

        let mut out = GrayImage::from_pixel(width, height, Luma([0]));
        for y in 0..height {
            for x in 0..width {
                let alpha = match trimap.get_pixel(x, y).0[0] {
                    TRIMAP_FG => 255,
                    TRIMAP_BG => 0,
                    _ => blurred.get_pixel(x, y).0[0],
                };
                out.put_pixel(x, y, Luma([alpha]));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn block_mask(width: u32, height: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> GrayImage {
        let mut mask = GrayImage::from_pixel(width, height, Luma([0]));
        for y in y0..y1 {
            for x in x0..x1 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        mask
    }

    #[test]
    fn trimap_has_fg_unknown_and_bg() {
        // A centred block; with a band the core stays FG, the rim is unknown,
        // and the far corners are BG.
        let mask = block_mask(20, 20, 6, 6, 14, 14);
        let trimap = trimap_from_mask(&mask, 2);
        assert_eq!(trimap.get_pixel(10, 10).0[0], TRIMAP_FG);
        assert_eq!(trimap.get_pixel(0, 0).0[0], TRIMAP_BG);
        // A pixel just outside the block edge falls in the dilated unknown ring.
        assert_eq!(trimap.get_pixel(5, 10).0[0], TRIMAP_UNKNOWN);
    }

    #[test]
    fn trimap_zero_band_is_pass_through() {
        // band == 0 ⇒ erode == dilate == mask ⇒ no unknown ring.
        let mask = block_mask(10, 10, 3, 3, 7, 7);
        let trimap = trimap_from_mask(&mask, 0);
        assert!(trimap.pixels().all(|p| p.0[0] != TRIMAP_UNKNOWN));
        assert_eq!(trimap.get_pixel(4, 4).0[0], TRIMAP_FG);
        assert_eq!(trimap.get_pixel(0, 0).0[0], TRIMAP_BG);
    }

    #[test]
    fn builtin_matte_keeps_fg_bg_and_softens_unknown() {
        let mask = block_mask(24, 24, 8, 8, 16, 16);
        let trimap = trimap_from_mask(&mask, 3);
        let image = RgbaImage::from_pixel(24, 24, Rgba([120, 120, 120, 255]));
        let alpha = BuiltinCpuMatter.matte(&image, &trimap).unwrap();
        assert_eq!(alpha.dimensions(), (24, 24));
        // Definite FG stays opaque, definite BG transparent.
        assert_eq!(alpha.get_pixel(12, 12).0[0], 255);
        assert_eq!(alpha.get_pixel(0, 0).0[0], 0);
        // Somewhere in the unknown ring the alpha is partial (a soft edge).
        let any_partial = alpha.pixels().any(|p| p.0[0] > 0 && p.0[0] < 255);
        assert!(any_partial, "expected a soft (partial-alpha) edge");
    }

    #[test]
    fn builtin_provider_id_is_stable() {
        assert_eq!(BuiltinCpuMatter.provider(), "builtin-cpu-matte");
    }

    #[test]
    fn preprocess_shape_and_normalisation() {
        let image = RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255]));
        let trimap = GrayImage::from_pixel(8, 8, Luma([TRIMAP_UNKNOWN]));
        let data = preprocess(&image, &trimap);
        let plane = (INPUT_SIZE * INPUT_SIZE) as usize;
        assert_eq!(data.len(), plane * 4);
        // Red channel: (1.0 - 0.5)/0.5 = 1.0; green: (0 - 0.5)/0.5 = -1.0.
        assert!((data[0] - 1.0).abs() < 1e-3, "r={}", data[0]);
        assert!((data[plane] + 1.0).abs() < 1e-3, "g={}", data[plane]);
        // Trimap channel: 128/255 ≈ 0.502, rescaled only (not normalised).
        assert!((data[3 * plane] - 0.50196).abs() < 1e-3, "t={}", data[3 * plane]);
    }

    #[test]
    fn postprocess_clamps_and_resizes() {
        // 2x2 alphas: top row opaque, bottom row transparent. Resize to 4x4.
        let alphas = vec![1.0, 1.0, 0.0, 0.0];
        let alpha = postprocess(&alphas, 2, 4, 4);
        assert_eq!(alpha.dimensions(), (4, 4));
        assert_eq!(alpha.get_pixel(0, 0).0[0], 255);
        assert_eq!(alpha.get_pixel(0, 3).0[0], 0);
    }

    /// Real inference, only when the weight resolves. Skipped otherwise so CI
    /// without the blob still passes.
    #[test]
    fn vitmatte_inference_when_weight_present() {
        let Some(path) = resolve_model_file(MODEL_ENV, MODEL_FILE) else {
            eprintln!("skipping vitmatte: no weight resolvable");
            return;
        };
        let matter = VitMatteMatter::load(&path).expect("load vitmatte");
        assert_eq!(matter.provider(), "vitmatte");
        let mut image = RgbaImage::from_pixel(64, 64, Rgba([120, 120, 120, 255]));
        for y in 20..44 {
            for x in 20..44 {
                image.put_pixel(x, y, Rgba([240, 30, 30, 255]));
            }
        }
        let trimap = trimap_from_mask(&block_mask(64, 64, 20, 20, 44, 44), 6);
        let alpha = matter.matte(&image, &trimap).expect("inference");
        assert_eq!(alpha.dimensions(), (64, 64));
    }
}
