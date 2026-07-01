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
//! The wrappers are deliberately thin over the `image` crate's optimized
//! `imageops`; the heavy per-pixel passes (morphology, guided filter, matte
//! composite, alpha apply) live next to their algorithms and are already
//! rayon-parallel (item 4).

use image::{imageops, imageops::FilterType, GrayImage, RgbaImage};

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
    fn crop_rgba_extracts_window() {
        let mut img = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 2, Rgba([9, 9, 9, 255]));
        let out = crop_rgba(&img, 1, 2, 2, 1);
        assert_eq!(out.dimensions(), (2, 1));
        assert_eq!(out.get_pixel(0, 0).0, [9, 9, 9, 255]);
        assert_eq!(out.get_pixel(1, 0).0, [0, 0, 0, 255]);
    }
}
