// Default executor registry: maps node kinds to runtime behaviour.
//
// The `generate` node composes an ApiTask and runs it through the existing
// H-Gripe broker (`run_task_json`). `prompt` is a pure value source; `preview`
// is a sink that just forwards its input image so downstream/inspector can use
// it. This wires the renderer-agnostic DAG runtime to real backend capability.

import { runTaskJson } from "../bridge/tauri";
import type { ExecutorRegistry } from "./dag";

export const defaultExecutors: ExecutorRegistry = {
  prompt: async (ctx) => ({ text: String(ctx.params.text ?? "") }),

  generate: async (ctx) => {
    const prompt = (ctx.inputs.prompt as string | undefined) ?? "";
    const reference = ctx.inputs.reference as string | undefined;

    const inputs: Record<string, unknown> = {};
    if (prompt) inputs.prompt = prompt;
    if (reference) inputs.image_path = reference;

    const params: Record<string, unknown> = {};
    if (ctx.params.model) params.model = ctx.params.model;
    if (ctx.params.size) params.size = ctx.params.size;

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
};
