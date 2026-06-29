// Tidy auto-layout: place nodes on a grid by their DAG depth. Each topological
// level becomes a column (x increases left→right with data flow); nodes within a
// level are stacked vertically. Pure and deterministic so it is unit-testable;
// the host computes the levels (via topoLevels) and feeds them in.

export interface LayoutOptions {
  /** Horizontal gap between columns (levels). */
  xGap?: number;
  /** Vertical gap between nodes in the same column. */
  yGap?: number;
  /** Top-left origin of the laid-out block. */
  xStart?: number;
  yStart?: number;
}

export type Positions = Map<string, { x: number; y: number }>;

// `levels[i]` holds the node ids at topological depth `i` (already filtered to
// the nodes that should move). Columns are vertically centered against the
// tallest column so the result looks balanced rather than top-aligned.
export function layeredPositions(levels: string[][], opts: LayoutOptions = {}): Positions {
  const xGap = opts.xGap ?? 260;
  const yGap = opts.yGap ?? 140;
  const xStart = opts.xStart ?? 40;
  const yStart = opts.yStart ?? 40;

  const tallest = levels.reduce((max, level) => Math.max(max, level.length), 0);
  const positions: Positions = new Map();

  levels.forEach((level, col) => {
    const offset = ((tallest - level.length) * yGap) / 2;
    level.forEach((id, row) => {
      positions.set(id, { x: xStart + col * xGap, y: yStart + offset + row * yGap });
    });
  });

  return positions;
}
