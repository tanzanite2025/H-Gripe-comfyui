// Node type catalogue. Each node kind declares its typed input/output ports
// and default params. The editor builds handles from this, the runtime reads
// it to wire inputs/outputs, and connection validation uses the port types.

import type { PortDataType, PortSpec } from "./model";

export type ParamControl =
  | "text"
  | "textarea"
  | "number"
  | "select"
  | "slider"
  | "checkbox"
  | "path";

export interface ParamSpec {
  key: string;
  label: string;
  control: ParamControl;
  options?: string[];
  defaultValue?: unknown;
  /** For `slider` / `number`. */
  min?: number;
  max?: number;
  step?: number;
  /** Optional hint shown under the control in the inspector. */
  hint?: string;
  /** Render this param directly on the node card (not just the inspector). */
  inline?: boolean;
  /** For `path` controls: native file-picker extension filter. */
  pickerFilterName?: string;
  pickerExtensions?: string[];
}

export interface NodeSpec {
  kind: string;
  title: string;
  /** Short description shown in the inspector / node palette. */
  description: string;
  /** Palette grouping. */
  category: "input" | "generate" | "output";
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
    category: "input",
    inputs: [],
    outputs: [port("text", "text", "text")],
    params: [
      {
        key: "text",
        label: "Prompt",
        control: "textarea",
        defaultValue: "",
        inline: true,
      },
    ],
  },
  batch: {
    kind: "batch",
    title: "Batch",
    description:
      "Sweeps a list of text items (one per line). A normal Run emits the first item; use \"Run ×N\" to fan out one run per item.",
    category: "input",
    inputs: [],
    outputs: [port("item", "item", "text")],
    params: [
      {
        key: "items",
        label: "Items (one per line)",
        control: "textarea",
        defaultValue: "",
        hint: "one prompt / value per line",
        inline: true,
      },
    ],
  },
  imageSource: {
    kind: "imageSource",
    title: "Image Source",
    description: "An image file on disk, used as a reference / input image.",
    category: "input",
    inputs: [],
    outputs: [port("image", "image", "image")],
    params: [
      {
        key: "path",
        label: "Image path",
        control: "path",
        defaultValue: "",
        hint: "absolute path to an image file",
        inline: true,
        pickerFilterName: "Images",
        pickerExtensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff"],
      },
    ],
  },
  psdTemplate: {
    kind: "psdTemplate",
    title: "PSD Template",
    description: "A .psd template path carried through to export.",
    category: "input",
    inputs: [],
    outputs: [port("template", "template", "any")],
    params: [
      {
        key: "path",
        label: "Template path",
        control: "path",
        defaultValue: "",
        hint: "absolute path to a .psd template",
        inline: true,
        pickerFilterName: "PSD",
        pickerExtensions: ["psd"],
      },
    ],
  },
  number: {
    kind: "number",
    title: "Number",
    description: "A numeric value (seed, count, …) fed into other nodes.",
    category: "input",
    inputs: [],
    outputs: [port("value", "value", "number")],
    params: [
      { key: "value", label: "Value", control: "number", defaultValue: 0, inline: true },
    ],
  },
  generate: {
    kind: "generate",
    title: "Generate",
    description:
      "Run an image generation operation through the H-Gripe broker.",
    category: "generate",
    inputs: [
      port("prompt", "prompt", "text"),
      port("reference", "reference", "image"),
      port("seed", "seed", "number"),
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
        inline: true,
      },
      { key: "model", label: "Model", control: "text", defaultValue: "" },
      { key: "size", label: "Size", control: "text", defaultValue: "1024x1024" },
      {
        key: "steps",
        label: "Steps",
        control: "slider",
        defaultValue: 20,
        min: 1,
        max: 50,
        step: 1,
        inline: true,
      },
      {
        key: "seed",
        label: "Seed",
        control: "number",
        defaultValue: 0,
        hint: "overridden by a connected seed input",
      },
      {
        key: "credentials_ref",
        label: "Credentials",
        control: "text",
        defaultValue: "",
        hint: "set automatically when you pick a profile",
      },
    ],
  },
  preview: {
    kind: "preview",
    title: "Preview",
    description:
      "Display a thumbnail of an image. The original path is preserved for export.",
    category: "output",
    inputs: [port("image", "image", "image")],
    outputs: [],
    params: [],
  },
  save: {
    kind: "save",
    title: "Export",
    description:
      "Sink node: collects the resulting image path (and optional PSD template) for export.",
    category: "output",
    inputs: [
      port("image", "image", "image"),
      port("template", "template", "any"),
    ],
    outputs: [],
    params: [
      { key: "filename", label: "File name", control: "text", defaultValue: "output.png", inline: true },
    ],
  },
  psdExport: {
    kind: "psdExport",
    title: "PSD Export",
    description:
      "Write the generated image into a PSD template's placeholder (true smart-object replacement when possible) and export final.psd + preview.png + metadata.json.",
    category: "output",
    inputs: [
      port("image", "image", "image"),
      port("template", "template", "any"),
    ],
    outputs: [],
    params: [
      { key: "filename", label: "File name", control: "text", defaultValue: "final", inline: true },
      {
        key: "output_dir",
        label: "Output dir",
        control: "path",
        defaultValue: "",
        hint: "leave empty to use the configured output directory",
      },
      {
        key: "placeholder",
        label: "Placeholder layer",
        control: "text",
        defaultValue: "",
        hint: "template layer name to replace (empty = whole canvas)",
        inline: true,
      },
      {
        key: "fit_mode",
        label: "Fit",
        control: "select",
        options: ["contain", "cover", "stretch"],
        defaultValue: "contain",
        inline: true,
      },
      {
        key: "smart_object_mode",
        label: "Smart object",
        control: "select",
        options: ["disable", "replace_content"],
        defaultValue: "replace_content",
        hint: "replace_content rewrites the smart object (stays editable in Photoshop)",
        inline: true,
      },
    ],
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

/** Node kinds grouped by palette category, in display order. */
export function paletteGroups(): { category: NodeSpec["category"]; specs: NodeSpec[] }[] {
  const order: NodeSpec["category"][] = ["input", "generate", "output"];
  return order.map((category) => ({
    category,
    specs: Object.values(NODE_SPECS).filter((s) => s.category === category),
  }));
}
