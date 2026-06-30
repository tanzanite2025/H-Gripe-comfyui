import { createContext, useContext } from "react";

export interface NodeEditing {
  /** Update a single param of a node (used by inline node-card controls). */
  onParamChange: (nodeId: string, key: string, value: unknown) => void;
  /** Open the shared, reusable Preview (review-gate) modal for a node. */
  openPreview?: (nodeId: string) => void;
  /** Open the on-demand Mask-Edit modal for a node (brush/wand/morphology). */
  openMaskEdit?: (nodeId: string) => void;
}

// Lets memoized node cards edit their own params without threading callbacks
// through `node.data` (which would pollute the serializable graph model).
export const NodeEditingContext = createContext<NodeEditing | null>(null);

export function useNodeEditing(): NodeEditing | null {
  return useContext(NodeEditingContext);
}
