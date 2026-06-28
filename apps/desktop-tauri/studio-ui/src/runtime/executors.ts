// Default executor registry: maps node kinds to runtime behaviour.
//
// The `generate` node composes an ApiTask and runs it through the existing
// H-Gripe broker (`run_task_json`). Source nodes (`prompt`, `imageSource`,
// `psdTemplate`, `number`) are pure value providers; `preview` / `save` are
// sinks. This wires the renderer-agnostic DAG runtime to real backend
// capability.

import { composePsd, getOutputDir, runTaskJson } from "../bridge/tauri";
import type { ExecutorRegistry } from "./dag";

// Params that are not forwarded into the broker task's `params` map; they are
// top-level task fields instead.
const GENERATE_RESERVED = new Set(["provider", "operation", "credentials_ref"]);

/** Non-empty, trimmed lines of a batch node's `items` param. */
export function batchItems(items: unknown): string[] {
  return String(items ?? "")
    .split("\n")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

export const defaultExecutors: ExecutorRegistry = {
  prompt: async (ctx) => ({ text: String(ctx.params.text ?? "") }),

  // Emits a single item from the list. A normal run emits index 0; batch
  // fan-out sweeps `index` via runGraph's paramOverrides.
  batch: async (ctx) => {
    const items = batchItems(ctx.params.items);
    const index = Number(ctx.params.index ?? 0);
    return { item: items[index] ?? items[0] ?? "" };
  },

  imageSource: async (ctx) => ({ image: String(ctx.params.path ?? "") || null }),

  psdTemplate: async (ctx) => ({ template: String(ctx.params.path ?? "") || null }),

  number: async (ctx) => ({ value: Number(ctx.params.value ?? 0) }),

  // Pass-through relay: forwards whatever arrives on `in` to `out` unchanged.
  reroute: async (ctx) => ({ out: ctx.inputs.in ?? null }),

  generate: async (ctx) => {
    const prompt = (ctx.inputs.prompt as string | undefined) ?? "";
    const reference = ctx.inputs.reference as string | undefined;
    const seedInput = ctx.inputs.seed as number | undefined;

    const inputs: Record<string, unknown> = {};
    if (prompt) inputs.prompt = prompt;
    if (reference) inputs.image_path = reference;

    // Forward every non-reserved, non-empty param into the broker task params.
    // A connected `seed` input overrides the param of the same name.
    const params: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(ctx.params)) {
      if (GENERATE_RESERVED.has(key)) continue;
      if (value === "" || value === null || value === undefined) continue;
      params[key] = value;
    }
    if (seedInput !== undefined) params.seed = seedInput;

    const credentialsRef = String(ctx.params.credentials_ref ?? "") || null;
    const task = {
      id: `studio-${ctx.nodeId}-${Date.now()}`,
      provider: String(ctx.params.provider ?? "mock"),
      operation: String(ctx.params.operation ?? "image.generate"),
      inputs,
      params,
      credentials_ref: credentialsRef,
      output_type: "image",
      cache_policy: { enabled: false, ttl_seconds: null, key: null },
      retry_policy: { max_attempts: 1, backoff_ms: 200, timeout_ms: 60000 },
    };

    const result = await runTaskJson(task);
    if (result.status === "failed") {
      throw new Error(result.error?.message ?? "generation failed");
    }
    const image = result.output_files?.[0]?.path;
    return { image: image ?? null, result };
  },

  preview: async (ctx) => ({ image: ctx.inputs.image ?? null }),

  save: async (ctx) => ({
    image: ctx.inputs.image ?? null,
    template: ctx.inputs.template ?? null,
    filename: String(ctx.params.filename ?? "output.png"),
  }),

  // Writes the upstream image into the PSD template's placeholder (true
  // smart-object replacement when possible) and exports the .psd triplet via
  // the backend `compose_psd` command.
  psdExport: async (ctx) => {
    const image = (ctx.inputs.image as string | undefined) ?? null;
    const template = (ctx.inputs.template as string | undefined) ?? null;
    if (!image) throw new Error("PSD Export needs a connected image input");
    if (!template) throw new Error("PSD Export needs a connected PSD template input");

    // Fall back to the configured output directory when none is set on the node.
    const outputDir = String(ctx.params.output_dir ?? "").trim() || (await getOutputDir());
    if (!outputDir) throw new Error("PSD Export needs an output directory");

    const placeholderName = String(ctx.params.placeholder ?? "").trim();
    const result = await composePsd({
      template,
      image,
      outputDir,
      filename: String(ctx.params.filename ?? "final") || "final",
      placeholder: placeholderName ? JSON.stringify({ name: placeholderName }) : undefined,
      fitMode: (String(ctx.params.fit_mode ?? "contain") as "contain" | "cover" | "stretch"),
      smartObjectMode: (String(ctx.params.smart_object_mode ?? "disable") as "disable" | "replace_content"),
    });
    if (result.status !== "succeeded") {
      throw new Error(`PSD export failed: ${result.status}`);
    }
    return {
      psdPath: result.psd_path,
      previewPath: result.preview_path || null,
      metadataPath: result.metadata_path,
      placeholderKind: result.placeholder_kind,
      smartObjectMode: result.smart_object_mode,
    };
  },
};
