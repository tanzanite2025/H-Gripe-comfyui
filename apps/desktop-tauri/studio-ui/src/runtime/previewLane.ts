// The preview execution lane: a single-slot, latest-wins, cancellable runner.
//
// Editor previews (a filter on a downscaled proxy, a video scrub frame) must
// feel immediate and must NOT share the global run lock (`inFlight` in
// useStudioRunController) — otherwise a preview would block, or be blocked by, a
// full graph Run. This primitive owns that lane: only the most recent request
// matters, so starting a new job cancels the one in flight. See
// `docs/cards/editor-resource-model.md` (§ "Four lanes" → "Preview").
//
// It is deliberately framework-agnostic (no React) so the latest-wins /
// cancellation semantics are unit-testable; a thin hook can wrap it when the
// first preview feature lands.

/** Handed to a preview job so it can bail out once superseded/disposed. */
export interface PreviewSignal {
  /** Flips to true when a newer request supersedes this one (or on dispose). */
  readonly cancelled: boolean;
}

/** A preview computation; should check `signal.cancelled` at await points. */
export type PreviewJob<T> = (signal: PreviewSignal) => Promise<T>;

/** Outcome of a preview run: `applied` if still latest, else `superseded`. */
export type PreviewOutcome<T> =
  | { status: "applied"; value: T }
  | { status: "superseded" };

interface MutableSignal {
  cancelled: boolean;
}

/**
 * Single-slot, latest-wins runner. `run` immediately cancels any in-flight job;
 * its promise resolves `applied` only if no newer `run` started before it
 * settled, otherwise `superseded` (so callers can ignore stale results).
 */
export class PreviewLane {
  private seq = 0;
  private current: MutableSignal | null = null;

  /** True while a preview job is in flight. */
  get busy(): boolean {
    return this.current !== null;
  }

  async run<T>(job: PreviewJob<T>): Promise<PreviewOutcome<T>> {
    const token = ++this.seq;
    if (this.current) this.current.cancelled = true;
    const signal: MutableSignal = { cancelled: false };
    this.current = signal;
    try {
      const value = await job(signal);
      if (token !== this.seq || signal.cancelled) return { status: "superseded" };
      return { status: "applied", value };
    } finally {
      if (token === this.seq) this.current = null;
    }
  }

  /** Cancel the in-flight job (if any) without starting a new one. */
  cancel(): void {
    if (this.current) this.current.cancelled = true;
    this.current = null;
    this.seq++;
  }
}
