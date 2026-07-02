// Modal-open state for the shared editor modals (Preview, Mask-Edit, Crop-Edit
// and the media source's unified manual editor), plus the connected-image
// lookup those modals use as their canvas underlay. Owns which node each modal
// targets; the modal components themselves stay in App's JSX.

import { useCallback, useMemo, useState } from "react";
import type { Edge, Node } from "@xyflow/react";

import type { HgripeNodeData } from "./HgripeNode";

export function useModals({ nodes, edges }: { nodes: Node[]; edges: Edge[] }) {
  // Which node (if any) has the shared Preview / Mask-Edit modal open.
  const [previewNodeId, setPreviewNodeId] = useState<string | null>(null);
  const [maskEditNodeId, setMaskEditNodeId] = useState<string | null>(null);
  const [cropEditNodeId, setCropEditNodeId] = useState<string | null>(null);
  // Image source whose unified manual editor (mask + crop) is open, if any.
  const [mediaEditSourceId, setMediaEditSourceId] = useState<string | null>(null);

  const openPreview = useCallback((nodeId: string) => setPreviewNodeId(nodeId), []);
  const openMaskEdit = useCallback((nodeId: string) => setMaskEditNodeId(nodeId), []);
  const openCropEdit = useCallback((nodeId: string) => setCropEditNodeId(nodeId), []);
  const openMediaEdit = useCallback((sourceId: string) => setMediaEditSourceId(sourceId), []);

  // Resolve the image path feeding a node's `image` input port: follow the
  // incoming edge to its source node and read that node's last-run image / path
  // param. Used as the best-effort underlay for the Mask-Edit canvas and the
  // layers of the Preview modal (often empty in browser preview).
  const connectedImagePath = useCallback(
    (nodeId: string): string | null => {
      const edge = edges.find((e) => e.target === nodeId && e.targetHandle === "image");
      if (!edge) return null;
      const src = nodes.find((n) => n.id === edge.source);
      if (!src) return null;
      const d = src.data as HgripeNodeData;
      return d.imagePath ?? (typeof d.params?.path === "string" ? (d.params.path as string) : null);
    },
    [edges, nodes],
  );

  const previewNode = useMemo(
    () => nodes.find((n) => n.id === previewNodeId) ?? null,
    [nodes, previewNodeId],
  );
  const maskEditNode = useMemo(
    () => nodes.find((n) => n.id === maskEditNodeId) ?? null,
    [nodes, maskEditNodeId],
  );
  const cropEditNode = useMemo(
    () => nodes.find((n) => n.id === cropEditNodeId) ?? null,
    [nodes, cropEditNodeId],
  );
  const mediaEditSource = useMemo(
    () => nodes.find((n) => n.id === mediaEditSourceId) ?? null,
    [nodes, mediaEditSourceId],
  );

  return {
    previewNode,
    maskEditNode,
    cropEditNode,
    mediaEditSource,
    setPreviewNodeId,
    setMaskEditNodeId,
    setCropEditNodeId,
    setMediaEditSourceId,
    openPreview,
    openMaskEdit,
    openCropEdit,
    openMediaEdit,
    connectedImagePath,
  };
}
