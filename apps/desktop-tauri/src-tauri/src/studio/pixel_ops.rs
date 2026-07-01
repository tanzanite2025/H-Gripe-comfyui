//! Unified image-buffer geometry ops for native-Rust cards (the `Compute`
//! lane, item 5 of `docs/cards/editor-resource-model.md`).
//!
//! Crop / resize used to be re-spelled ad hoc at every call site
//! (`imageops::crop_imm(..).to_image()`, `imageops::resize(.., Triangle)`, each
//! with its own hand-rolled identity-size short-circuit). This module is the
//! single seam those cards route through so the filter choice and the
//! "already the target size → don't reallocate" fast path stay identical
//! everywhere.
//!
//! The geometry wrappers are deliberately thin over the `image` crate's
//! optimized `imageops`. The module also owns the two full-resolution
//! *composite* passes the compute lane shares — applying a mask as alpha and
//! flattening a trimap band into alpha — which used to be hand-inlined in
//! `subject_mask` / `subject_matte`; both are row-parallel with rayon (item 4).
//! The remaining per-pixel algorithms (morphology, guided filter) still live
//! next to their math.

use image::{imageops, imageops::FilterType, GrayImage, RgbaImage};
use rayon::prelude::*;

/// Resize an RGBA surface to `width`x`height`, cloning instead of resampling
/// when it is already that size. Uses `Triangle` (bilinear) — the filter the
/// matte/model backends up/downscale colour with.
pub(super) fn resize_rgba(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    if image.dimensions() == (width, height) {
        image.clone()
    } else {
        imageops::resize(image, width, height, FilterType::Triangle)
    }
}

/// Resize a single-channel mask to `width`x`height` with an explicit `filter`
/// (callers pick `Nearest` for hard masks, `Triangle` for soft alpha), cloning
/// at identity size.
pub(super) fn resize_gray(
    image: &GrayImage,
    width: u32,
    height: u32,
    filter: FilterType,
) -> GrayImage {
    if image.dimensions() == (width, height) {
        image.clone()
    } else {
        imageops::resize(image, width, height, filter)
    }
}

/// Crop the `(x, y, width, height)` window out of an RGBA surface into an owned
/// image (an immutable view, so the source is untouched).
pub(super) fn crop_rgba(
    image: &RgbaImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> RgbaImage {
    imageops::crop_imm(image, x, y, width, height).to_image()
}

/// Composite a single-channel `mask` into `image` as its alpha channel: the RGB
/// is kept verbatim and each pixel's alpha is replaced by the matching mask
/// sample (the compute lane's "cutout" step). `mask` must cover `image` (same
/// width, at least as tall); rows are independent, so the copy runs in parallel.
pub(super) fn apply_alpha_mask(image: &RgbaImage, mask: &GrayImage) -> RgbaImage {
    let (width, _height) = image.dimensions();
    let w = width as usize;
    let mask_buf = mask.as_raw();
    let mut out = image.clone();
    // `ImageBuffer` derefs to its packed RGBA `[u8]` (4 bytes/px).
    let buf: &mut [u8] = &mut out;
    buf.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let base = y * w;
        for x in 0..w {
            row[x * 4 + 3] = mask_buf[base + x];
        }
    });
    out
}

/// Flatten a `trimap` + a resolved `soft` alpha into a single hard/soft alpha
/// buffer: pixels at `fg_level` become fully opaque (255), pixels at `bg_level`
/// fully transparent (0), and everything in the unknown band takes its value
/// from `soft`. `trimap` and `soft` must share dimensions; filled row-parallel.
pub(super) fn composite_trimap_alpha(
    trimap: &GrayImage,
    soft: &GrayImage,
    fg_level: u8,
    bg_level: u8,
) -> GrayImage {
    let (width, height) = trimap.dimensions();
    let w = width as usize;
    let trimap_buf = trimap.as_raw();
    let soft_buf = soft.as_raw();
    let mut out_buf = vec![0u8; w * height as usize];
    out_buf.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let base = y * w;
        for (x, slot) in row.iter_mut().enumerate() {
            let level = trimap_buf[base + x];
            *slot = if level == fg_level {
                255
            } else if level == bg_level {
                0
            } else {
                soft_buf[base + x]
            };
        }
    });
    GrayImage::from_raw(width, height, out_buf)
        .expect("composite buffer matches trimap dimensions")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, Rgba};

    #[test]
    fn resize_rgba_identity_size_clones_pixels() {
        let mut img = RgbaImage::new(3, 2);
        for (i, p) in img.pixels_mut().enumerate() {
            p.0 = [i as u8, 0, 0, 255];
        }
        let out = resize_rgba(&img, 3, 2);
        assert_eq!(out.dimensions(), (3, 2));
        assert_eq!(out, img);
    }

    #[test]
    fn resize_rgba_changes_dimensions() {
        let img = RgbaImage::from_pixel(4, 4, Rgba([10, 20, 30, 255]));
        let out = resize_rgba(&img, 2, 8);
        assert_eq!(out.dimensions(), (2, 8));
    }

    #[test]
    fn resize_gray_nearest_identity_clones() {
        let mut img = GrayImage::new(2, 2);
        img.put_pixel(0, 0, Luma([255]));
        let out = resize_gray(&img, 2, 2, FilterType::Nearest);
        assert_eq!(out, img);
    }

    #[test]
    fn apply_alpha_mask_replaces_alpha_keeps_rgb() {
        let img = RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255]));
        let mut mask = GrayImage::new(2, 2);
        mask.put_pixel(0, 0, Luma([0]));
        mask.put_pixel(1, 0, Luma([128]));
        mask.put_pixel(0, 1, Luma([200]));
        mask.put_pixel(1, 1, Luma([255]));
        let out = apply_alpha_mask(&img, &mask);
        assert_eq!(out.get_pixel(0, 0).0, [10, 20, 30, 0]);
        assert_eq!(out.get_pixel(1, 0).0, [10, 20, 30, 128]);
        assert_eq!(out.get_pixel(0, 1).0, [10, 20, 30, 200]);
        assert_eq!(out.get_pixel(1, 1).0, [10, 20, 30, 255]);
    }

    #[test]
    fn composite_trimap_alpha_selects_hard_then_soft() {
        // fg -> 255, bg -> 0, unknown -> soft sample.
        let mut trimap = GrayImage::new(3, 1);
        trimap.put_pixel(0, 0, Luma([255])); // fg
        trimap.put_pixel(1, 0, Luma([0])); // bg
        trimap.put_pixel(2, 0, Luma([128])); // unknown
        let mut soft = GrayImage::new(3, 1);
        soft.put_pixel(0, 0, Luma([7])); // ignored (hard fg)
        soft.put_pixel(1, 0, Luma([9])); // ignored (hard bg)
        soft.put_pixel(2, 0, Luma([73])); // used
        let out = composite_trimap_alpha(&trimap, &soft, 255, 0);
        assert_eq!(out.get_pixel(0, 0).0, [255]);
        assert_eq!(out.get_pixel(1, 0).0, [0]);
        assert_eq!(out.get_pixel(2, 0).0, [73]);
    }

    #[test]
    fn crop_rgba_extracts_window() {
        let mut img = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 2, Rgba([9, 9, 9, 255]));
        let out = crop_rgba(&img, 1, 2, 2, 1);
        assert_eq!(out.dimensions(), (2, 1));
        assert_eq!(out.get_pixel(0, 0).0, [9, 9, 9, 255]);
        assert_eq!(out.get_pixel(1, 0).0, [0, 0, 0, 255]);
    }
}
