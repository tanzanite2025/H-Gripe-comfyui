import { createContext, useContext } from "react";

export interface NodeEditing {
  /** Update a single param of a node (used by inline node-card controls). */
  onParamChange: (nodeId: string, key: string, value: unknown) => void;
  /** Open the shared, reusable Preview (review-gate) modal for a node. */
  openPreview?: (nodeId: string) => void;
  /** Open the on-demand Mask-Edit modal for a node (brush/wand/morphology). */
  openMaskEdit?: (nodeId: string) => void;
  /** Open the on-demand Crop-Edit modal for a crop node (manual box / auto). */
  openCropEdit?: (nodeId: string) => void;
  /**
   * Spawn a bound edit node of `editKind` from a media source card: create the
   * node to the right, wire a `binding` edge from the source's `image` output
   * to the new node's `image` input, select it, and open its editor.
   * See docs/cards/generic-media-card.md.
   */
  addBoundEdit?: (sourceId: string, editKind: string) => void;
  /**
   * Run only the target node and its transitive inputs (ancestor subgraph),
   * then surface the result onto its card — so confirming an edit shows a
   * result without a full-graph run.
   */
  runUpToNode?: (nodeId: string) => void;
}

// Lets memoized node cards edit their own params without threading callbacks
// through `node.data` (which would pollute the serializable graph model).
export const NodeEditingContext = createContext<NodeEditing | null>(null);

export function useNodeEditing(): NodeEditing | null {
  return useContext(NodeEditingContext);
}
