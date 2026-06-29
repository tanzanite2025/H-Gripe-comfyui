import type { NodeStatus } from "../runtime/dag";
import type { NodeSpec } from "../graph/nodeSpecs";

// Minimap node fill: run status wins (so a survey view shows progress/failures
// at a glance), falling back to a per-category color when the node is idle.
const STATUS_COLOR: Partial<Record<NodeStatus, string>> = {
  running: "#ffcc00",
  succeeded: "#38d39f",
  cached: "#38d39f",
  failed: "#ff5d5d",
  skipped: "#555a66",
};

const CATEGORY_COLOR: Record<NodeSpec["category"], string> = {
  input: "#6aa3ff",
  generate: "#b98cff",
  control: "#ffa657",
  utility: "#8a93a3",
  output: "#5fd0d0",
};

export function miniMapColor(status: NodeStatus | undefined, category: NodeSpec["category"]): string {
  return (status && STATUS_COLOR[status]) || CATEGORY_COLOR[category];
}
