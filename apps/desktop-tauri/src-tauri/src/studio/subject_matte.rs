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
//! - **`builtin-cpu-matte`** — a deterministic, weight-free fallback that runs a
//!   **guided filter** (He et al., *Guided Image Filtering*) over the unknown
//!   band, using the source image as the guidance signal so the resolved alpha
//!   follows real edges (hair, fur, foliage) instead of a blind Gaussian
//!   feather. It works end-to-end before any weight is present (and keeps CI
//!   green without the blob).
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

/// The builtin guided-filter matte runs at no more than this edge, then
/// upsamples just the soft unknown band back to full resolution (the FG/BG
/// regions stay hard at full res). Bounds memory like the ViTMatte backend.
const BUILTIN_MAX_EDGE: u32 = 2048;
/// Guided-filter regularisation in the normalised `[0, 1]` alpha domain: larger
/// smooths more, smaller keeps finer edge detail. Tuned for whispy edges.
const GUIDED_EPS: f32 = 1e-4;

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

/// A deterministic, weight-free matter. The definite foreground stays fully
/// opaque and the definite background fully transparent; the unknown band is
/// resolved by a **guided filter** that uses the source image as guidance, so
/// the soft alpha tracks real edges (hair, fur) rather than a blind feather.
/// Reproducible continuous alpha without a model.
pub(super) struct BuiltinCpuMatter;

impl AlphaMatter for BuiltinCpuMatter {
    fn provider(&self) -> &str {
        BUILTIN_PROVIDER
    }

    fn matte(&self, image: &RgbaImage, trimap: &GrayImage) -> Result<GrayImage, String> {
        let (width, height) = trimap.dimensions();
        if width == 0 || height == 0 {
            return Err("Subject Mask matting needs a non-empty image".to_string());
        }

        // Resolve the soft band at a bounded resolution: downscale the guide and
        // trimap, run the guided filter there, then upsample only the soft alpha
        // back. FG/BG stay hard from the full-res trimap below.
        let (sw, sh) = bounded_size(width, height, BUILTIN_MAX_EDGE);
        let small_rgb = resize_rgba(image, sw, sh);
        let small_tri = if (sw, sh) == (width, height) {
            trimap.clone()
        } else {
            // Nearest keeps the three trimap levels crisp (no blended levels).
            image::imageops::resize(trimap, sw, sh, FilterType::Nearest)
        };

        let n = (sw * sh) as usize;
        let mut guide = vec![0f32; n];
        let mut prior = vec![0f32; n];
        let mut unknown = 0usize;
        for (i, (rgb, tri)) in small_rgb.pixels().zip(small_tri.pixels()).enumerate() {
            let [r, g, b, _] = rgb.0;
            // Rec.601 luma in [0, 1] as the single-channel guidance signal.
            guide[i] = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0;
            prior[i] = match tri.0[0] {
                TRIMAP_FG => 1.0,
                TRIMAP_BG => 0.0,
                _ => {
                    unknown += 1;
                    // Neutral prior in the unknown band; the guide pulls it
                    // toward 0/1 along image edges.
                    0.5
                }
            };
        }

        // No unknown ring (e.g. band == 0): the trimap is already the final
        // hard alpha, so skip the filter entirely.
        if unknown == 0 {
            return Ok(harden(trimap));
        }

        // Window radius scales with the (downscaled) band thickness.
        let radius = (((unknown as f64).sqrt() / 4.0).round() as usize).clamp(2, 64);
        let q = guided_filter(&guide, &prior, sw as usize, sh as usize, radius, GUIDED_EPS);

        let mut soft = GrayImage::from_pixel(sw, sh, Luma([0]));
        for (pixel, value) in soft.pixels_mut().zip(q) {
            pixel.0[0] = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        let soft = if (sw, sh) == (width, height) {
            soft
        } else {
            image::imageops::resize(&soft, width, height, FilterType::Triangle)
        };

        // Composite at full res: hard FG/BG from the trimap, guided alpha in the
        // unknown band.
        let mut out = GrayImage::from_pixel(width, height, Luma([0]));
        for y in 0..height {
            for x in 0..width {
                let alpha = match trimap.get_pixel(x, y).0[0] {
                    TRIMAP_FG => 255,
                    TRIMAP_BG => 0,
                    _ => soft.get_pixel(x, y).0[0],
                };
                out.put_pixel(x, y, Luma([alpha]));
            }
        }
        Ok(out)
    }
}

/// Largest size with the same aspect whose longest edge is `<= max_edge`.
fn bounded_size(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max_edge {
        return (width, height);
    }
    let scale = max_edge as f32 / longest as f32;
    let w = ((width as f32 * scale).round() as u32).max(1);
    let h = ((height as f32 * scale).round() as u32).max(1);
    (w, h)
}

fn resize_rgba(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    if image.dimensions() == (width, height) {
        image.clone()
    } else {
        image::imageops::resize(image, width, height, FilterType::Triangle)
    }
}

/// Map a trimap straight to a hard alpha (FG → 255, otherwise 0).
fn harden(trimap: &GrayImage) -> GrayImage {
    let (width, height) = trimap.dimensions();
    let mut out = GrayImage::from_pixel(width, height, Luma([0]));
    for (src, dst) in trimap.pixels().zip(out.pixels_mut()) {
        dst.0[0] = if src.0[0] == TRIMAP_FG { 255 } else { 0 };
    }
    out
}

/// Mean (box) filter over a `width * height` plane with a `(2*radius+1)` square
/// window, normalised per-pixel by the in-bounds count. O(N) via an integral
/// image (f64 accumulation to stay precise on large planes).
fn box_filter(src: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let stride = width + 1;
    let mut integral = vec![0f64; stride * (height + 1)];
    for y in 0..height {
        let mut row = 0f64;
        for x in 0..width {
            row += src[y * width + x] as f64;
            integral[(y + 1) * stride + (x + 1)] = integral[y * stride + (x + 1)] + row;
        }
    }
    let mut out = vec![0f32; width * height];
    for y in 0..height {
        let y0 = y.saturating_sub(radius);
        let y1 = (y + radius + 1).min(height);
        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius + 1).min(width);
            let sum = integral[y1 * stride + x1] - integral[y0 * stride + x1]
                - integral[y1 * stride + x0]
                + integral[y0 * stride + x0];
            let count = ((y1 - y0) * (x1 - x0)) as f64;
            out[y * width + x] = (sum / count) as f32;
        }
    }
    out
}

/// Guided filter (He et al.): output `q = a * guide + b`, where `a`, `b` are the
/// per-window linear fit of `src` to `guide` with regularisation `eps`. Edges in
/// `guide` (the image luma) are preserved in `q`, which is what pulls the soft
/// alpha along hair/fur boundaries.
fn guided_filter(
    guide: &[f32],
    src: &[f32],
    width: usize,
    height: usize,
    radius: usize,
    eps: f32,
) -> Vec<f32> {
    let mean_i = box_filter(guide, width, height, radius);
    let mean_p = box_filter(src, width, height, radius);
    let prod_ip: Vec<f32> = guide.iter().zip(src).map(|(i, p)| i * p).collect();
    let mean_ip = box_filter(&prod_ip, width, height, radius);
    let prod_ii: Vec<f32> = guide.iter().map(|i| i * i).collect();
    let mean_ii = box_filter(&prod_ii, width, height, radius);

    let mut a = vec![0f32; width * height];
    let mut b = vec![0f32; width * height];
    for k in 0..a.len() {
        let var_i = mean_ii[k] - mean_i[k] * mean_i[k];
        let cov_ip = mean_ip[k] - mean_i[k] * mean_p[k];
        let ak = cov_ip / (var_i + eps);
        a[k] = ak;
        b[k] = mean_p[k] - ak * mean_i[k];
    }
    let mean_a = box_filter(&a, width, height, radius);
    let mean_b = box_filter(&b, width, height, radius);

    let mut q = vec![0f32; width * height];
    for k in 0..q.len() {
        q[k] = mean_a[k] * guide[k] + mean_b[k];
    }
    q
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
    fn builtin_matte_follows_image_edge() {
        // A vertical colour edge at x = 20: dark left, bright right. The mask is
        // the right half, so the unknown band straddles the colour edge. A blind
        // feather would be symmetric about the mask boundary; the guided filter
        // must instead favour the bright (subject) side, so a band pixel on the
        // bright side reads more opaque than one the same distance on the dark side.
        let (w, h) = (40, 20);
        let mut image = RgbaImage::from_pixel(w, h, Rgba([12, 12, 12, 255]));
        for y in 0..h {
            for x in 20..w {
                image.put_pixel(x, y, Rgba([242, 242, 242, 255]));
            }
        }
        let mask = block_mask(w, h, 20, 0, w, h);
        let trimap = trimap_from_mask(&mask, 8);
        // Both sample points sit inside the unknown band (x ∈ [12, 28)).
        assert_eq!(trimap.get_pixel(16, 10).0[0], TRIMAP_UNKNOWN);
        assert_eq!(trimap.get_pixel(24, 10).0[0], TRIMAP_UNKNOWN);

        let alpha = BuiltinCpuMatter.matte(&image, &trimap).unwrap();
        let bright = alpha.get_pixel(24, 10).0[0];
        let dark = alpha.get_pixel(16, 10).0[0];
        assert!(
            bright > dark,
            "guided matte should track the colour edge: bright={bright} dark={dark}"
        );
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

    /// Real ViTMatte inference, only when the weight resolves (set
    /// `HGRIPE_VITMATTE_MODEL` or bundle `resources/models/vitmatte.onnx`).
    /// Skipped otherwise so CI without the blob still passes; the opt-in
    /// `vitmatte-e2e` CI job fetches the weight and runs this. Beyond shape, it
    /// asserts the matte honours the trimap — the definite-foreground core comes
    /// back (near-)opaque and the definite-background corner (near-)transparent,
    /// which is what proves the weight is actually wired through `ort`.
    #[test]
    fn vitmatte_inference_when_weight_present() {
        let Some(path) = resolve_model_file(MODEL_ENV, MODEL_FILE) else {
            eprintln!("skipping vitmatte: no weight resolvable");
            return;
        };
        let matter = VitMatteMatter::load(&path).expect("load vitmatte");
        assert_eq!(matter.provider(), "vitmatte");

        // A red square subject on a grey field; the trimap fixes a definite-FG
        // core, a definite-BG exterior, and an unknown ring between.
        let mut image = RgbaImage::from_pixel(64, 64, Rgba([120, 120, 120, 255]));
        for y in 20..44 {
            for x in 20..44 {
                image.put_pixel(x, y, Rgba([240, 30, 30, 255]));
            }
        }
        let trimap = trimap_from_mask(&block_mask(64, 64, 20, 20, 44, 44), 6);
        // Sanity: the sample points are in the trimap regions we assert on.
        assert_eq!(trimap.get_pixel(32, 32).0[0], TRIMAP_FG);
        assert_eq!(trimap.get_pixel(2, 2).0[0], TRIMAP_BG);

        let alpha = matter.matte(&image, &trimap).expect("inference");
        assert_eq!(alpha.dimensions(), (64, 64));

        let fg = alpha.get_pixel(32, 32).0[0];
        let bg = alpha.get_pixel(2, 2).0[0];
        assert!(fg > 200, "definite-FG core should stay opaque, got {fg}");
        assert!(bg < 55, "definite-BG corner should stay transparent, got {bg}");
        assert!(fg > bg, "matte must separate subject from background");
    }
}
