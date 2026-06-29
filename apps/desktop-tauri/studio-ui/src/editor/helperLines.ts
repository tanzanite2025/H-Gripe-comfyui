import type { Node, NodePositionChange, XYPosition } from "@xyflow/react";

export interface HelperLinesResult {
  // Flow-space coordinates of the alignment guides to draw, if any.
  horizontal?: number;
  vertical?: number;
  // The position the dragged node should snap to (per-axis; undefined = keep).
  snapPosition: Partial<XYPosition>;
}

function dims(node: Node): { width: number; height: number } {
  return {
    width: node.measured?.width ?? node.width ?? 0,
    height: node.measured?.height ?? node.height ?? 0,
  };
}

// Computes alignment guides + a snapped position for a single dragged node by
// comparing its edges (left/right/top/bottom) against every other node. When an
// edge lands within `distance` (flow units) of another node's matching edge, it
// snaps and reports a guide line. Adapted from the React Flow "helper lines"
// example. Pure — no React Flow store access — so it is unit-testable.
export function getHelperLines(
  change: NodePositionChange,
  nodes: Node[],
  distance = 6,
): HelperLinesResult {
  const result: HelperLinesResult = { snapPosition: {} };
  const nodeA = nodes.find((n) => n.id === change.id);
  if (!nodeA || !change.position) return result;

  const a = dims(nodeA);
  const aBounds = {
    left: change.position.x,
    right: change.position.x + a.width,
    top: change.position.y,
    bottom: change.position.y + a.height,
    width: a.width,
    height: a.height,
  };

  let vDist = distance; // best vertical-guide (x-axis) distance so far
  let hDist = distance; // best horizontal-guide (y-axis) distance so far

  for (const nodeB of nodes) {
    if (nodeB.id === nodeA.id) continue;
    // Skip cross-parent comparisons: coordinates are in different frames.
    if ((nodeB.parentId ?? null) !== (nodeA.parentId ?? null)) continue;
    const b = dims(nodeB);
    const bBounds = {
      left: nodeB.position.x,
      right: nodeB.position.x + b.width,
      top: nodeB.position.y,
      bottom: nodeB.position.y + b.height,
    };

    // --- vertical guides (align x) ---
    const leftLeft = Math.abs(aBounds.left - bBounds.left);
    if (leftLeft < vDist) {
      result.snapPosition.x = bBounds.left;
      result.vertical = bBounds.left;
      vDist = leftLeft;
    }
    const rightRight = Math.abs(aBounds.right - bBounds.right);
    if (rightRight < vDist) {
      result.snapPosition.x = bBounds.right - aBounds.width;
      result.vertical = bBounds.right;
      vDist = rightRight;
    }
    const leftRight = Math.abs(aBounds.left - bBounds.right);
    if (leftRight < vDist) {
      result.snapPosition.x = bBounds.right;
      result.vertical = bBounds.right;
      vDist = leftRight;
    }
    const rightLeft = Math.abs(aBounds.right - bBounds.left);
    if (rightLeft < vDist) {
      result.snapPosition.x = bBounds.left - aBounds.width;
      result.vertical = bBounds.left;
      vDist = rightLeft;
    }

    // --- horizontal guides (align y) ---
    const topTop = Math.abs(aBounds.top - bBounds.top);
    if (topTop < hDist) {
      result.snapPosition.y = bBounds.top;
      result.horizontal = bBounds.top;
      hDist = topTop;
    }
    const bottomTop = Math.abs(aBounds.bottom - bBounds.top);
    if (bottomTop < hDist) {
      result.snapPosition.y = bBounds.top - aBounds.height;
      result.horizontal = bBounds.top;
      hDist = bottomTop;
    }
    const bottomBottom = Math.abs(aBounds.bottom - bBounds.bottom);
    if (bottomBottom < hDist) {
      result.snapPosition.y = bBounds.bottom - aBounds.height;
      result.horizontal = bBounds.bottom;
      hDist = bottomBottom;
    }
    const topBottom = Math.abs(aBounds.top - bBounds.bottom);
    if (topBottom < hDist) {
      result.snapPosition.y = bBounds.bottom;
      result.horizontal = bBounds.bottom;
      hDist = topBottom;
    }
  }

  return result;
}
