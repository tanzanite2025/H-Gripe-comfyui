// Front-end, best-effort mask morphology on a downscaled proxy buffer.
//
// The Mask-Edit modal records morphology as *intent* (`MaskOperation` entries)
// and the Rust backend rasterises the authoritative result on run — see the
// note on `MaskOperation` in `types/production.ts` about not re-implementing the
// exact Rust morphology so stored state can't drift. This module is deliberately
// the OTHER thing: a cheap, approximate **preview** of grow / shrink / feather /
// smooth on a small proxy alpha buffer, so a slider drag can show roughly what
// the op will do without a backend round-trip. It is the `preview` lane from
// `docs/cards/editor-resource-model.md` (§ "Four lanes") — advisory only, never
// committed. Nothing here is written back onto the node.
//
// Everything is pure (no canvas / DOM) so the geometry is unit-testable; the
// modal wraps a proxy build + `applyOp` in `PreviewLane` for latest-wins drags
// and does the canvas rasterisation of the result overlay separately.

import type { BrushStroke, EditPaths, MaskOperation } from "../types/production";

/** A single-channel alpha buffer (0..255), row-major `w * h`. */
export interface ProxyMask {
  w: number;
  h: number;
  data: Uint8Array;
}

/** Morphology op ids this module can preview (the amount-taking ones). */
export const PREVIEWABLE_OP_IDS = ["grow", "shrink", "feather", "smooth"] as const;
export type PreviewableOpId = (typeof PREVIEWABLE_OP_IDS)[number];

export function isPreviewableOp(id: string): id is PreviewableOpId {
  return (PREVIEWABLE_OP_IDS as readonly string[]).includes(id);
}

export function createProxyMask(w: number, h: number): ProxyMask {
  return { w: Math.max(1, w | 0), h: Math.max(1, h | 0), data: new Uint8Array(Math.max(1, w | 0) * Math.max(1, h | 0)) };
}

function cloneMask(mask: ProxyMask): ProxyMask {
  return { w: mask.w, h: mask.h, data: new Uint8Array(mask.data) };
}

const at = (mask: ProxyMask, x: number, y: number): number => mask.data[y * mask.w + x];

/** Stamp a filled disc of `value` (clamped to the buffer) centred at cx,cy. */
export function stampDisc(mask: ProxyMask, cx: number, cy: number, radius: number, value: number): void {
  const r = Math.max(0, radius);
  const r2 = (r + 0.5) * (r + 0.5);
  const x0 = Math.max(0, Math.floor(cx - r));
  const x1 = Math.min(mask.w - 1, Math.ceil(cx + r));
  const y0 = Math.max(0, Math.floor(cy - r));
  const y1 = Math.min(mask.h - 1, Math.ceil(cy + r));
  for (let y = y0; y <= y1; y++) {
    const dy = y - cy;
    for (let x = x0; x <= x1; x++) {
      const dx = x - cx;
      if (dx * dx + dy * dy <= r2) mask.data[y * mask.w + x] = value;
    }
  }
}

/** Stamp discs along a polyline so a brush stroke reads as a continuous band. */
function stampStroke(mask: ProxyMask, stroke: BrushStroke, scale: number): void {
  const value = stroke.mode === "subtract" ? 0 : 255;
  const radius = Math.max(1, Math.round(stroke.radius * scale));
  const pts = stroke.points;
  if (pts.length === 0) return;
  if (pts.length === 1) {
    stampDisc(mask, pts[0][0] * scale, pts[0][1] * scale, radius, value);
    return;
  }
  for (let i = 1; i < pts.length; i++) {
    const [ax, ay] = pts[i - 1];
    const [bx, by] = pts[i];
    const x0 = ax * scale;
    const y0 = ay * scale;
    const x1 = bx * scale;
    const y1 = by * scale;
    const dist = Math.hypot(x1 - x0, y1 - y0);
    const steps = Math.max(1, Math.ceil(dist / Math.max(1, radius / 2)));
    for (let s = 0; s <= steps; s++) {
      const tt = s / steps;
      stampDisc(mask, x0 + (x1 - x0) * tt, y0 + (y1 - y0) * tt, radius, value);
    }
  }
}

/** Fill a marquee `rect` / `ellipse` region (image-space `[x1,y1,x2,y2]`). */
function fillMarquee(mask: ProxyMask, op: MaskOperation, scale: number): void {
  const region = op.region;
  if (!region || region.length < 4) return;
  const x1 = Math.min(region[0], region[2]) * scale;
  const y1 = Math.min(region[1], region[3]) * scale;
  const x2 = Math.max(region[0], region[2]) * scale;
  const y2 = Math.max(region[1], region[3]) * scale;
  const cx = (x1 + x2) / 2;
  const cy = (y1 + y2) / 2;
  const rx = Math.max(0.5, (x2 - x1) / 2);
  const ry = Math.max(0.5, (y2 - y1) / 2);
  const px0 = Math.max(0, Math.floor(x1));
  const px1 = Math.min(mask.w - 1, Math.ceil(x2));
  const py0 = Math.max(0, Math.floor(y1));
  const py1 = Math.min(mask.h - 1, Math.ceil(y2));
  for (let y = py0; y <= py1; y++) {
    for (let x = px0; x <= px1; x++) {
      if (op.type === "ellipse") {
        const nx = (x - cx) / rx;
        const ny = (y - cy) / ry;
        if (nx * nx + ny * ny <= 1) mask.data[y * mask.w + x] = 255;
      } else {
        mask.data[y * mask.w + x] = 255;
      }
    }
  }
}

/** Disc-kernel max filter (grayscale dilation) — grows the mask by `radius` px. */
export function dilate(mask: ProxyMask, radius: number): ProxyMask {
  return rankFilter(mask, radius, true);
}

/** Disc-kernel min filter (grayscale erosion) — shrinks the mask by `radius` px. */
export function erode(mask: ProxyMask, radius: number): ProxyMask {
  return rankFilter(mask, radius, false);
}

function rankFilter(mask: ProxyMask, radius: number, wantMax: boolean): ProxyMask {
  const r = Math.round(radius);
  if (r <= 0) return cloneMask(mask);
  // Precompute the disc offsets once.
  const offsets: [number, number][] = [];
  const r2 = (r + 0.25) * (r + 0.25);
  for (let dy = -r; dy <= r; dy++) {
    for (let dx = -r; dx <= r; dx++) {
      if (dx * dx + dy * dy <= r2) offsets.push([dx, dy]);
    }
  }
  const out = createProxyMask(mask.w, mask.h);
  for (let y = 0; y < mask.h; y++) {
    for (let x = 0; x < mask.w; x++) {
      let best = wantMax ? 0 : 255;
      for (const [dx, dy] of offsets) {
        const nx = x + dx;
        const ny = y + dy;
        if (nx < 0 || ny < 0 || nx >= mask.w || ny >= mask.h) continue;
        const v = at(mask, nx, ny);
        if (wantMax ? v > best : v < best) best = v;
      }
      out.data[y * mask.w + x] = best;
    }
  }
  return out;
}

/** Separable box blur (one pass) — a cheap gaussian-ish feather of the edge. */
function boxBlur(mask: ProxyMask, radius: number): ProxyMask {
  const r = Math.round(radius);
  if (r <= 0) return cloneMask(mask);
  const tmp = createProxyMask(mask.w, mask.h);
  const win = 2 * r + 1;
  // Horizontal pass.
  for (let y = 0; y < mask.h; y++) {
    let sum = 0;
    for (let x = -r; x <= r; x++) sum += at(mask, clamp(x, 0, mask.w - 1), y);
    for (let x = 0; x < mask.w; x++) {
      tmp.data[y * mask.w + x] = Math.round(sum / win);
      const outX = clamp(x - r, 0, mask.w - 1);
      const inX = clamp(x + r + 1, 0, mask.w - 1);
      sum += at(mask, inX, y) - at(mask, outX, y);
    }
  }
  // Vertical pass.
  const out = createProxyMask(mask.w, mask.h);
  for (let x = 0; x < mask.w; x++) {
    let sum = 0;
    for (let y = -r; y <= r; y++) sum += tmp.data[clamp(y, 0, tmp.h - 1) * tmp.w + x];
    for (let y = 0; y < mask.h; y++) {
      out.data[y * mask.w + x] = Math.round(sum / win);
      const outY = clamp(y - r, 0, mask.h - 1);
      const inY = clamp(y + r + 1, 0, mask.h - 1);
      sum += tmp.data[inY * tmp.w + x] - tmp.data[outY * tmp.w + x];
    }
  }
  return out;
}

/** Feather = two box-blur passes (a smoother, gaussian-like soft edge). */
export function feather(mask: ProxyMask, radius: number): ProxyMask {
  if (radius <= 0) return cloneMask(mask);
  return boxBlur(boxBlur(mask, radius), radius);
}

/** Morphological open (erode→dilate) then close (dilate→erode): despeckle + fill nicks. */
export function smooth(mask: ProxyMask, radius: number): ProxyMask {
  const r = Math.max(1, Math.round(radius));
  const opened = dilate(erode(mask, r), r);
  return erode(dilate(opened, r), r);
}

/** Invert the whole mask (255 - v). */
export function invert(mask: ProxyMask): ProxyMask {
  const out = createProxyMask(mask.w, mask.h);
  for (let i = 0; i < mask.data.length; i++) out.data[i] = 255 - mask.data[i];
  return out;
}

/**
 * Fill interior holes: threshold at 128, flood-fill "outside" from the border,
 * then any background pixel the flood never reached is an enclosed hole → 255.
 */
export function fillHoles(mask: ProxyMask): ProxyMask {
  const { w, h } = mask;
  const bg = new Uint8Array(w * h); // 1 where thresholded background
  for (let i = 0; i < mask.data.length; i++) bg[i] = mask.data[i] < 128 ? 1 : 0;
  const outside = new Uint8Array(w * h);
  const stack: number[] = [];
  const push = (x: number, y: number) => {
    if (x < 0 || y < 0 || x >= w || y >= h) return;
    const idx = y * w + x;
    if (bg[idx] && !outside[idx]) {
      outside[idx] = 1;
      stack.push(idx);
    }
  };
  for (let x = 0; x < w; x++) {
    push(x, 0);
    push(x, h - 1);
  }
  for (let y = 0; y < h; y++) {
    push(0, y);
    push(w - 1, y);
  }
  while (stack.length) {
    const idx = stack.pop()!;
    const x = idx % w;
    const y = (idx / w) | 0;
    push(x - 1, y);
    push(x + 1, y);
    push(x, y - 1);
    push(x, y + 1);
  }
  const out = cloneMask(mask);
  for (let i = 0; i < out.data.length; i++) {
    if (bg[i] && !outside[i]) out.data[i] = 255; // enclosed hole
  }
  return out;
}

/**
 * Apply one recorded op to the proxy mask. `radius` is already in *proxy*
 * pixels (the modal scales the image-space `amount` by the proxy ratio).
 * `wand` needs the real image and is a no-op on the proxy.
 */
export function applyOp(mask: ProxyMask, type: string, radius: number): ProxyMask {
  switch (type) {
    case "grow":
      return dilate(mask, radius);
    case "shrink":
      return erode(mask, radius);
    case "feather":
      return feather(mask, radius);
    case "smooth":
      return smooth(mask, radius);
    case "invert":
      return invert(mask);
    case "fill_holes":
      return fillHoles(mask);
    default:
      return cloneMask(mask); // wand / rect / ellipse handled elsewhere or need pixels
  }
}

export interface ProxyBuildOptions {
  /** Target proxy width in px (height derives from the image aspect). */
  proxyWidth?: number;
}

const DEFAULT_PROXY_WIDTH = 320;

/**
 * Rasterise the committed edits (brush strokes + marquee regions + queued
 * morphology, in order) into a downscaled proxy mask. `wand` ops are skipped
 * (they need the source pixels). This is the base a pending previewed op is
 * then applied on top of.
 */
export function buildProxyMask(
  edits: EditPaths,
  dims: { w: number; h: number },
  options: ProxyBuildOptions = {},
): { mask: ProxyMask; scale: number } {
  const proxyWidth = Math.max(16, Math.min(options.proxyWidth ?? DEFAULT_PROXY_WIDTH, dims.w || DEFAULT_PROXY_WIDTH));
  const scale = proxyWidth / Math.max(1, dims.w);
  const w = Math.max(1, Math.round((dims.w || proxyWidth) * scale));
  const h = Math.max(1, Math.round((dims.h || proxyWidth) * scale));
  let mask = createProxyMask(w, h);
  for (const stroke of edits.brush_strokes) stampStroke(mask, stroke, scale);
  for (const op of edits.operations) {
    if (op.type === "rect" || op.type === "ellipse") {
      fillMarquee(mask, op, scale);
    } else if (op.type === "wand") {
      // Needs the real image; not previewable on the proxy.
    } else {
      const radius = op.amount != null ? Math.round(op.amount * scale) : 0;
      mask = applyOp(mask, op.type, radius);
    }
  }
  return { mask, scale };
}

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}
