import { BaseEdge, getStraightPath, type EdgeProps } from "@xyflow/react";

// The "binding" edge ties a media source card to an edit-result node spawned
// from it (see docs/cards/generic-media-card.md). Unlike a normal workflow
// connection it is drawn as a short, straight accent line so a binding reads
// differently from an ordinary data wire. It is otherwise a regular data edge
// (the executor treats it like any other), so only the rendering differs here.
export function BindingEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  markerEnd,
  style,
}: EdgeProps) {
  const [path] = getStraightPath({ sourceX, sourceY, targetX, targetY });
  return (
    <BaseEdge
      id={id}
      path={path}
      markerEnd={markerEnd}
      style={{ stroke: "#7c5cff", strokeWidth: 2, strokeDasharray: "4 3", ...style }}
    />
  );
}
