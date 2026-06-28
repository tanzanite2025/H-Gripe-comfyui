import { memo } from "react";
import { NodeResizer, type NodeProps } from "@xyflow/react";

// A resizable container frame. It carries no ports and does no work at run time;
// it just visually groups the nodes parented to it (which move with it). The
// label is edited in the Inspector. Rendered behind its children (see
// orderNodes) so the frame never covers the nodes it contains.
function GroupNodeImpl({ data, selected }: NodeProps) {
  const label = String((data as { params?: { label?: unknown } }).params?.label ?? "Group");
  return (
    <>
      <NodeResizer
        minWidth={160}
        minHeight={120}
        isVisible={!!selected}
        lineClassName="group-resize-line"
        handleClassName="group-resize-handle"
      />
      <div className={`group-node ${selected ? "selected" : ""}`}>
        <div className="group-label">{label}</div>
      </div>
    </>
  );
}

export const GroupNode = memo(GroupNodeImpl);
