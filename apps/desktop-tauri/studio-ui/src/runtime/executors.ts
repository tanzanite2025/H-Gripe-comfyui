// Default executor registry: maps node kinds to runtime behaviour.
//
// The `generate` node composes an ApiTask and runs it through the existing
// H-Gripe broker (`run_task_json`). Source nodes (`prompt`, `imageSource`,
// `psdTemplate`, `number`) are pure value providers; `preview` / `save` are
// sinks. This wires the renderer-agnostic DAG runtime to real backend
// capability.

import { runTaskJson } from "../bridge/tauri";
import type { ExecutorRegistry } from "./dag";

// Params that are not forwarded into the broker task's `params` map.
const GENERATE_RESERVED = new Set(["provider", "operation"]);

export const defaultExecutors: ExecutorRegistry = {
  prompt: async (ctx) => ({ text: String(ctx.params.text ?? "") }),

  imageSource: async (ctx) => ({ image: String(ctx.params.path ?? "") || null }),

  psdTemplate: async (ctx) => ({ template: String(ctx.params.path ?? "") || null }),

  number: async (ctx) => ({ value: Number(ctx.params.value ?? 0) }),

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

    const task = {
      id: `studio-${ctx.nodeId}-${Date.now()}`,
      provider: String(ctx.params.provider ?? "mock"),
      operation: String(ctx.params.operation ?? "image.generate"),
      inputs,
      params,
      credentials_ref: null,
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
};
