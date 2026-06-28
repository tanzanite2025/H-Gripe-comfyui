// Node type catalogue. Each node kind declares its typed input/output ports
// and default params. The editor builds handles from this, the runtime reads
// it to wire inputs/outputs, and connection validation uses the port types.

import type { PortDataType, PortSpec } from "./model";

export interface ParamSpec {
  key: string;
  label: string;
  control: "text" | "textarea" | "number" | "select";
  options?: string[];
  defaultValue?: unknown;
}

export interface NodeSpec {
  kind: string;
  title: string;
  /** Short description shown in the inspector / node palette. */
  description: string;
  inputs: PortSpec[];
  outputs: PortSpec[];
  params: ParamSpec[];
}

function port(id: string, label: string, type: PortDataType): PortSpec {
  return { id, label, type };
}

export const NODE_SPECS: Record<string, NodeSpec> = {
  prompt: {
    kind: "prompt",
    title: "Prompt",
    description: "A text prompt fed into generation nodes.",
    inputs: [],
    outputs: [port("text", "text", "text")],
    params: [
      {
        key: "text",
        label: "Prompt",
        control: "textarea",
        defaultValue: "",
      },
    ],
  },
  generate: {
    kind: "generate",
    title: "Generate",
    description:
      "Run an image generation operation through the H-Gripe broker.",
    inputs: [
      port("prompt", "prompt", "text"),
      port("reference", "reference", "image"),
    ],
    outputs: [port("image", "image", "image")],
    params: [
      { key: "provider", label: "Provider", control: "text", defaultValue: "mock" },
      {
        key: "operation",
        label: "Operation",
        control: "select",
        options: ["image.generate", "image.edit", "echo"],
        defaultValue: "image.generate",
      },
      { key: "model", label: "Model", control: "text", defaultValue: "" },
      { key: "size", label: "Size", control: "text", defaultValue: "1024x1024" },
    ],
  },
  preview: {
    kind: "preview",
    title: "Preview",
    description:
      "Display a thumbnail of an image. The original path is preserved for export.",
    inputs: [port("image", "image", "image")],
    outputs: [],
    params: [],
  },
};

export function nodeSpec(kind: string): NodeSpec {
  const spec = NODE_SPECS[kind];
  if (!spec) throw new Error(`unknown node kind: ${kind}`);
  return spec;
}

export function defaultParams(kind: string): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const p of nodeSpec(kind).params) out[p.key] = p.defaultValue ?? "";
  return out;
}
