# Colour pipeline: working space, bit depth, and the manual / model split

**Status:** Decided (architecture). **Landed (P1–P4):** the canonical
surface is 16-bit ProPhoto for wide-gamut sources with a colour-managed sRGB
egress, and the native manual chain (crop, subject-mask) carries it end-to-end
with 16-bit PNG/TIFF-with-ICC output; P5 (Python parity) remains. See
*Current state* below. This document is the **single source of truth** for
colour space, bit depth, and ICC handling across every card. Where a per-card
spec still describes colour behaviour, it describes the 8-bit sRGB the cards
still consume and points here for the canonical model.

## Goal

Everything the Studio does to a pixel exists to make the result **as close to
the input as possible** for two downstream consumers, which are handled
differently:

- **Manual path** — human colour grading, retouching, clipping/editing, PSD
  production. Wants maximum fidelity: the widest colour, the most precision,
  nothing baked away that a colourist would later want back.
- **Model path** — feeding local ML models and remote API models. These are
  *split* from each other (local vs API are separate egress lanes), but they
  share one property: **they assume sRGB**. SAM2 / BiRefNet / the detail models
  and virtually every image API interpret their input as 8-bit sRGB and are not
  ICC-aware.

## The conflict (why this needed a decision)

"Best for grading" and "what models want" are **not the same pixels**:

- CMYK (and other print/photo) sources contain saturated colours — notably
  cyans and greens — that fall **outside sRGB**. Bake to sRGB and those colours
  are **clipped away and unrecoverable**; a colourist can never pull them back.
- Models, by contrast, are trained on sRGB. Hand them wide-gamut pixels and,
  because they ignore the ICC tag, they read the numbers **as if sRGB** and
  produce wrong colour — degrading the very generation quality we care about.
  APIs are stricter still: they accept only 8-bit sRGB over the wire.

So a single wide-gamut buffer cannot faithfully feed both. The chosen
resolution (**option W**):

> Keep a **wide-gamut, 16-bit canonical working surface** (with an ICC tag) as
> the internal truth for the manual path, and convert to **sRGB only at the
> model egress boundary** (8-bit for APIs, 8/16-bit for local models). The
> sRGB conversion is the *only* place gamut is reduced, and it happens as late
> as possible.

The rejected alternative (**option U**) was a single sRGB-16-bit buffer for
both paths: simpler and truly single-path, but it permanently clips the
out-of-sRGB print colours the manual path is meant to preserve. W was chosen
because manual-grade fidelity is the priority; the cost is one colour-managed
conversion on the model lanes.

## Canonical working surface

| Property | Target | Rationale |
| --- | --- | --- |
| Bit depth | **16-bit** per channel, both paths | Removes 8-bit banding/quantisation in grading; keeps model inputs precise. Unifies the pipeline on one depth (no dual-depth plumbing). |
| Colour | **Wide-gamut RGB** with an ICC tag carried alongside the pixels | Contains the CMYK gamut so print colours survive into grading. |
| Working primaries | **ProPhoto RGB (ROMM)** — decided | Fully contains coated-stock CMYK; at 16-bit its wide primaries do not band. (Adobe RGB (1998) was the conservative alternative; ProPhoto chosen for maximum gamut coverage.) |
| Alpha | Straight (un-premultiplied) 16-bit alpha track, as today | Matches the current independent-alpha handling. |

The canonical surface replaces today's `RgbaImage` (8-bit) as the type the
shared loaders return and the in-process cards operate on.

## Path split

```
                         ┌─────────────────────────────────────────┐
 source (CMYK/RGB/…) ──▶ │ decode + colour-manage into CANONICAL    │
                         │ 16-bit wide-gamut RGB + ICC              │
                         └───────────────┬──────────────────────────┘
                                         │  (single internal truth)
             ┌───────────────────────────┼───────────────────────────┐
             ▼                                                         ▼
  MANUAL PATH (preserve)                                    MODEL EGRESS (convert)
  crop / subject-mask (SAM2) /                              ┌ local model: → sRGB (8/16-bit)
  refine-edge / match-light /                               └ API model:   → sRGB **8-bit** (wire)
  detail / PSD export                                       (ICC-managed convert; only place
  → keep wide-gamut 16-bit + ICC                             gamut is reduced, done as late
  → file output embeds the ICC                               as possible)
```

- **Manual path** operates on and preserves the canonical surface end to end.
  File outputs stay wide-gamut and **embed the ICC** (16-bit PNG or TIFF); no
  lossy sRGB bake. `icc_preserved: true` for these outputs.
- **Model egress** is the one boundary that converts canonical → sRGB, colour
  managed (not a naive drop). Local and API lanes stay split; the API lane
  additionally quantises to 8-bit for transport.
- **CMYK sources** decode raw inks + embedded ICC (`cmyk_decode`) and are
  transformed **into the canonical wide-gamut surface** — *not* into sRGB as
  today. The naive fallback (no usable profile) is likewise retargeted.

## Current state (implemented today)

The **canonical working surface is now 16-bit ProPhoto for wide-gamut sources**;
the cards still consume 8-bit sRGB via a colour-managed egress. Accurate as of
this document:

- `studio_image::load_working` decodes into the canonical 16-bit `WorkingImage`
  (P1 #188, P2 #189), tagging each surface with its *actual* space:
  - **Profiled CMYK** (embedded CMYK ICC) → colour-managed straight into **16-bit
    ProPhoto** (`cmyk_transform::cmyk_to_prophoto16`), so inks outside sRGB
    survive the load instead of being clipped.
  - **Plain images and unprofiled/naive CMYK** → **`Srgb`** (pure 8→16-bit
    widen); no wide-gamut information to preserve.
- `studio_image::load_rgba` still returns an **8-bit `RgbaImage`** for the cards
  via `WorkingImage::to_srgb_rgba8` (the model/output egress, P3): it
  colour-manages **ProPhoto → sRGB** when needed, and is an **exact bit-narrow**
  for `Srgb` — so plain images and naive CMYK reach the cards **byte-for-byte**
  (the pinned cross-language naive contract is untouched), and only profiled
  CMYK changes (a small ΔE vs the old direct CMYK→sRGB, now routed through
  ProPhoto). `image_buffer` caches the 8-bit egress result as before.
- CMYK (TIFF / Adobe JPEG / YCCK JPEG / unmarked JPEG) is decoded raw + ICC by
  `cmyk_decode`. ICC interpolation is tetrahedral (PR #185). ProPhoto↔sRGB uses
  the same moxcms engine (gamma-1.8 `new_pro_photo_rgb`); the source CMYK
  profile is not re-embedded on the sRGB egress (`icc_preserved: false`).
- High-bit and float sources are still tone-scaled to 8-bit at the egress; the
  16-bit ProPhoto surface + its ICC are what the manual-path file output (P4)
  will consume directly.
- Outputs are 8-bit sRGB PNG. Non-CMYK RGB/RGBA/L/LA re-embed their own ICC
  (`icc_preserved: true`); CMYK does not.

CMYK **decode coverage** is complete (16-bit + alpha TIFF #183, shared-loader
routing #184, tetrahedral ICC #185, unmarked CMYK JPEG #186). The **wide-gamut
working space** (P1–P3) and the manual-path 16-bit chain (P4a–P4e) have now
landed; what remains is P5 (Python-bridge parity).

## Phased implementation plan

Design-first; each phase is an independently reviewable, CI-gated PR.

- **P0 — CMYK decode coverage.** ✅ Done (#180–#186). Raw inks + ICC for every
  CMYK container, routed through the shared loader.
- **P1 — canonical surface type (plumbing, no gamut change).** ✅ Done (#188).
  16-bit RGBA + ICC + `WorkingSpace` surface type; pure widening, behaviour
  unchanged.
- **P2 — thread the carrier through the loader (behaviour-preserving).** ✅ Done
  (#189). `load_working` decodes into the 16-bit carrier (still `Srgb`) and
  `load_rgba` narrows back, byte-for-byte identical.
- **P2b+P3 — switch canonical to wide-gamut + model egress.** ✅ Done (this PR).
  Profiled CMYK is colour-managed into **16-bit ProPhoto**; the card/model/output
  boundary converts **ProPhoto → sRGB** (`to_srgb_rgba8`). `Srgb`-tagged sources
  (plain images, naive CMYK) egress as an exact bit-narrow, so only genuinely
  wide-gamut sources pay the round-trip and the byte-exact naive contract holds.
  Shipped together because switching the space without the egress would
  mis-colour every card output. TRC stays gamma-encoded (linear-light deferred).
- **P4 — manual-path 16-bit chain + file output.** Decided (乙): the manual
  chain carries the 16-bit `WorkingImage` end-to-end — `image_buffer` caches the
  canonical surface and 16-bit PNG / TIFF encoders embed the ICC on the manual
  outputs (`icc_preserved: true`) — while the preview / Python-bridge / API
  boundaries keep the 8-bit sRGB egress. Staged as independently reviewable PRs:
  - **P4a (landed):** `image_buffer` gains a `WorkingImage` carrier —
    `publish_working` / `lookup_working` serve the native 16-bit surface to the
    manual chain (`load_working` consults the cache), while `lookup_rgba` /
    `lookup_dynamic` / eviction-materialisation egress it to 8-bit sRGB so every
    existing consumer is unchanged. Purely additive; nothing publishes a
    16-bit surface yet.
  - **P4b (landed):** crop walks the 16-bit canonical surface end-to-end: it
    loads via `load_working` (buffer-aware), crops the `WorkingImage`
    geometrically, publishes the 16-bit surface, and writes the manual output
    through `write_working_png` — an `Srgb` surface lands as the exact 8-bit
    PNG written before (byte-identical), a `ProPhoto` surface as **16-bit RGBA
    PNG with the ProPhoto profile embedded** (`icc_preserved: true`), which the
    loader recognises on reload and rebuilds at full precision. The
    auto-subject segmenter (model ingress) and the thumbnail fallback keep the
    sRGB egress.
  - **P4c (landed):** 16-bit TIFF (with ICC) encoder. `write_working_output`
    dispatches on the output path's extension — `.tif` / `.tiff` →
    `write_working_tiff`, everything else → `write_working_png` — and both
    honour the space tag identically (an `Srgb` surface writes the exact 8-bit
    narrow, a `ProPhoto` surface writes 16-bit RGBA with the ProPhoto profile
    embedded in the TIFF `IccProfile` (34675) tag, which the loader recognises
    on reload). crop gains a `format` param (`png` default / `tiff`).
  - **P4d (landed):** subject-mask walks the 16-bit canonical surface for its
    RGBA products. `subject_mask` loads via `load_working` (buffer-aware) and
    composites the mask into the 16-bit surface as alpha
    (`pixel_ops::apply_alpha_mask_working`), so the `alpha_image` / `cutout`
    outputs egress through `write_working_output` — an `Srgb` surface lands as
    the exact 8-bit PNG written before (byte-identical), a `ProPhoto` surface
    as 16-bit RGBA PNG with the profile embedded (`icc_preserved: true`) — and
    the native surface is published to the buffer for the next compute card.
    The `mask` output stays 8-bit gray, and every model / analysis ingress
    (auto segmenter, the matter, wand-select, morphology) keeps the 8-bit sRGB
    egress (`to_srgb_rgba8`), consistent with P3. `refineMaskEdge` is a
    Python-bridge card, so its pixel work is reconciled in P5, not here.
  - **P4e (close-out):** no further native card work remained. The only
    native-Rust manual pixel cards are crop (P4b/P4c) and subject-mask (P4d);
    the other manual cards — `matchLightColor`, `detailWatchdog`,
    `refineMaskEdge`, `imageEnhance` — are python-bridge cards whose pixel
    work lives in the CLI, so their 16-bit contract is reconciled in **P5**.
    The native `imageEnhance` cpu fast path is deliberately excluded too: its
    contract is byte-identical parity with the Python cpu engine, so it must
    move in lock-step with the bridge in P5, not ahead of it.
- **P5 — Python-bridge parity.** Reconcile / retire the Python path's 8-bit
  sRGB behaviour so the two engines agree on the new contract.

## Open decisions

1. **Working-space primaries:** ~~ProPhoto RGB vs Adobe RGB (1998)~~ —
   **decided: ProPhoto RGB (ROMM)** for maximum gamut coverage.
2. **TRC:** gamma-encoded working space (minimal behavioural change from today)
   vs linear-light (more correct resampling/compositing math, but changes
   results and cost). Recommendation: keep gamma-encoded for P1–P2, revisit
   linear as a separate change.
3. **Manual file container:** 16-bit PNG (simple, ICC via iCCP) vs TIFF
   (can also carry CMYK directly). PNG recommended unless a card needs CMYK
   round-trip.
4. **Local-model bit depth:** 8-bit vs 16-bit sRGB per model — depends on each
   model's actual input contract; decide per integration in P3.

## Related

- `docs/cards/image-enhance.md` — per-card colour handling (current 8-bit
  sRGB); defers here for the target.
- `docs/implementation-status.md` — initiative tracking.
