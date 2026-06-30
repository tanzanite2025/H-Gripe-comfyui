// Editor execution lanes — the cost/latency class an editor op runs in.
//
// As the manual editor ("small-PS popup") grows many tools, each one declares
// the lane it runs in so the runtime can route work without blocking the UI or
// over-subscribing the GPU. This is the front-end half of the model frozen in
// `docs/cards/editor-resource-model.md` (§ "Four lanes"); the Rust job-queue +
// warm-pool half lands separately.

/**
 * Where an op's real work runs and what its latency budget is:
 *
 * - `interactive` (< ~16-100 ms): shown instantly on the webview canvas with no
 *   backend round-trip — brush/eraser strokes, marquee shapes, vector paths.
 * - `preview` (~100 ms-1 s): cheap, cancellable, latest-wins compute on a
 *   downscaled proxy — geometry / morphology (grow, shrink, feather, …).
 * - `render` (heavy, full-resolution): the committed backend pipeline, model
 *   inference or real-pixel work gated behind the GPU queue — matting, SAM 2
 *   points, colour-flood wand.
 */
export type ExecLane = "interactive" | "preview" | "render";

/** All lanes, ordered cheapest → heaviest. */
export const EXEC_LANES: readonly ExecLane[] = ["interactive", "preview", "render"] as const;

/** Lane that touches the GPU/backend and must be queued (one render at a time). */
export function isHeavyLane(lane: ExecLane): boolean {
  return lane === "render";
}

/** Lane that may run off the global run lock as a single-slot, latest-wins job. */
export function isPreviewLane(lane: ExecLane): boolean {
  return lane === "preview";
}
