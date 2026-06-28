import { createContext, useContext } from "react";

export interface NodeEditing {
  /** Update a single param of a node (used by inline node-card controls). */
  onParamChange: (nodeId: string, key: string, value: unknown) => void;
}

// Lets memoized node cards edit their own params without threading callbacks
// through `node.data` (which would pollute the serializable graph model).
export const NodeEditingContext = createContext<NodeEditing | null>(null);

export function useNodeEditing(): NodeEditing | null {
  return useContext(NodeEditingContext);
}
