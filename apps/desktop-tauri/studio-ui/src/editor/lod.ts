// Zoom level-of-detail (LOD): below this zoom the node cards collapse to just a
// title bar (hiding inline params / thumbnails / error blocks). Keeps large
// graphs legible and cheap to render when zoomed out to survey the whole graph.
export const LOD_ZOOM_THRESHOLD = 0.55;

/** Should a node render in collapsed (title-only) form at this zoom level? */
export function isLodActive(zoom: number, threshold = LOD_ZOOM_THRESHOLD): boolean {
  return zoom < threshold;
}
