import { BaseEdge, useStore, type EdgeProps } from "@xyflow/react";
import { avoidanceMidX, orthogonalPoints, pointsToPath, type Rect } from "./edgeRouting";

// Custom edge that routes its vertical segment around other nodes. Obstacle
// rects come from the React Flow store (measured, absolutely-positioned nodes),
// excluding this edge's own source/target.
export function SmartEdge({
  id,
  source,
  target,
  sourceX,
  sourceY,
  targetX,
  targetY,
  markerEnd,
  style,
}: EdgeProps) {
  const obstacles = useStore((s) => {
    const rects: Rect[] = [];
    for (const [nid, item] of s.nodeLookup) {
      if (nid === source || nid === target) continue;
      // Group frames are large backdrops; skipping them avoids forcing every
      // wire around the whole container.
      if (item.type === "group") continue;
      const pos = item.internals.positionAbsolute;
      const width = item.measured?.width ?? item.width ?? 0;
      const height = item.measured?.height ?? item.height ?? 0;
      if (!width || !height) continue;
      rects.push({ x: pos.x, y: pos.y, width, height });
    }
    return rects;
  });

  const s = { x: sourceX, y: sourceY };
  const t = { x: targetX, y: targetY };
  const path = pointsToPath(orthogonalPoints(s, t, avoidanceMidX(s, t, obstacles)));

  return <BaseEdge id={id} path={path} markerEnd={markerEnd} style={style} />;
}
