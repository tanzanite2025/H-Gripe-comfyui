//! Colour management for the studio pipeline, layered along a pixel's journey
//! from container to card:
//!
//! - **Ingress** — [`cmyk_decode`]: pull raw device-direction CMYK samples (and
//!   any embedded ICC profile) straight out of TIFF/JPEG containers.
//! - **Canonical surface** — [`cmyk_transform`]: colour-manage profiled CMYK
//!   into the wide 16-bit ProPhoto working space (or 8-bit sRGB / PIL-naive for
//!   the unprofiled contract). The per-bit-depth moxcms `TransformOptions` live
//!   in exactly one place here (`cmyk_transform_options`).
//! - **Egress** — [`working_image`]: the 16-bit `WorkingImage` canvas itself,
//!   widen/narrow, sRGB⇄ProPhoto conversions, and the 8-bit sRGB egress the
//!   cards and models consume.
//!
//! Every moxcms transform the pipeline performs is constructed inside this
//! module; code outside `color/` never touches moxcms options directly. The
//! stage-isolated golden tests (`cmyk_transform_stage_prophoto16_is_golden`,
//! `prophoto16_egress_is_golden_per_stage`) pin each layer independently so a
//! regression names its stage.

pub(crate) mod cmyk_decode;
pub(crate) mod cmyk_transform;
pub(crate) mod working_image;
