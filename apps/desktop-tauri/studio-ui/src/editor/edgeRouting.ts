// Best-effort orthogonal edge routing that steers the wire's vertical segment
// around node rectangles, so edges don't run straight through unrelated nodes.
// Pure + deterministic for unit testing; the custom edge component feeds in the
// endpoint coords and the obstacle rects (other nodes) from the React Flow store.

export interface Pt {
  x: number;
  y: number;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

// Does a vertical segment at `x` spanning [y0,y1] (inflated by `pad`) overlap
// the rectangle?
function vSegHitsRect(x: number, y0: number, y1: number, r: Rect, pad: number): boolean {
  const lo = Math.min(y0, y1);
  const hi = Math.max(y0, y1);
  return (
    x >= r.x - pad &&
    x <= r.x + r.width + pad &&
    hi >= r.y - pad &&
    lo <= r.y + r.height + pad
  );
}

// Choose the x-coordinate for the wire's vertical segment. Starts at the
// midpoint between the endpoints; if that column would cut through any
// obstacle, shift it just past the blocking rectangles — to whichever side
// (left/right) is the smaller deviation.
export function avoidanceMidX(s: Pt, t: Pt, obstacles: Rect[], pad = 12): number {
  const midX = (s.x + t.x) / 2;
  const blocking = obstacles.filter((r) => vSegHitsRect(midX, s.y, t.y, r, pad));
  if (blocking.length === 0) return midX;

  const rightOf = Math.max(...blocking.map((r) => r.x + r.width)) + pad;
  const leftOf = Math.min(...blocking.map((r) => r.x)) - pad;
  return Math.abs(rightOf - midX) <= Math.abs(leftOf - midX) ? rightOf : leftOf;
}

// Orthogonal polyline: out from the source, across at `midX`, into the target.
export function orthogonalPoints(s: Pt, t: Pt, midX: number): Pt[] {
  return [s, { x: midX, y: s.y }, { x: midX, y: t.y }, t];
}

// SVG path string for a polyline.
export function pointsToPath(points: Pt[]): string {
  return points.map((p, i) => `${i === 0 ? "M" : "L"} ${p.x},${p.y}`).join(" ");
}

// Convenience: full routed path string around the given obstacles.
export function routedPath(s: Pt, t: Pt, obstacles: Rect[], pad = 12): string {
  return pointsToPath(orthogonalPoints(s, t, avoidanceMidX(s, t, obstacles, pad)));
}
