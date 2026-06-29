// Deterministic, model-free prompt optimisation used by the `promptOptimize`
// node's `local` mode. Kept pure and dependency-free so it can be unit-tested
// in isolation and mirrored 1:1 by the Rust runner (see `studio.rs`).

export type LocalPreset =
  | "cleanup"
  | "photographic"
  | "anime"
  | "cinematic"
  | "detailed";

// Booster tags appended (deduped) per preset. `cleanup` only normalises.
const PRESET_TAGS: Record<LocalPreset, string[]> = {
  cleanup: [],
  photographic: [
    "photorealistic",
    "high detail",
    "sharp focus",
    "natural lighting",
    "8k",
  ],
  anime: ["anime style", "vibrant colors", "clean lineart", "highly detailed"],
  cinematic: [
    "cinematic lighting",
    "dramatic composition",
    "depth of field",
    "film grain",
  ],
  detailed: ["highly detailed", "intricate", "ultra quality", "masterpiece"],
};

/** Split a prompt into trimmed, non-empty, comma-separated segments. */
function segments(text: string): string[] {
  return text
    .replace(/\s+/g, " ")
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/**
 * Normalise a prompt (collapse whitespace, split on commas, drop empties,
 * case-insensitively dedupe keeping first occurrence) and append the preset's
 * booster tags (also deduped against what is already present). Returns the
 * comma-joined result. An empty input yields an empty string.
 */
export function optimizePromptLocally(
  text: string,
  preset: LocalPreset = "cleanup",
): string {
  const out: string[] = [];
  const seen = new Set<string>();
  const push = (segment: string) => {
    const key = segment.toLowerCase();
    if (seen.has(key)) return;
    seen.add(key);
    out.push(segment);
  };

  for (const segment of segments(text)) push(segment);
  // Only decorate when there is real content to decorate.
  if (out.length > 0) {
    for (const tag of PRESET_TAGS[preset] ?? []) push(tag);
  }
  return out.join(", ");
}
