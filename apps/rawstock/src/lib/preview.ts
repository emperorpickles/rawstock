/** A terminal preview event. The job id is the only field the inbox needs. */
export interface PreviewNotice {
  jobId: number;
}

/**
 * `develop_preview` can finish before its command returns the job id.
 * Events that arrive in that window are kept and applied once `note` learns
 * the id. A stale event is a different job from that window: it must not
 * replace the adjust frame, and the caller still delivers it to whoever
 * owns that job.
 */
export class PreviewInbox<T extends PreviewNotice> {
  private waiting = false;
  private current: number | null = null;
  private early = new Map<number, T>();

  /** Call before the command, while the id is still unknown. */
  expect(): void {
    this.waiting = true;
    this.current = null;
  }

  /** The command failed before an id existed. Returns what was buffered. */
  abandon(): T[] {
    this.waiting = false;
    this.current = null;
    const stale = [...this.early.values()];
    this.early.clear();
    return stale;
  }

  /** `early` is held. `current` belongs to the known job. `stale` does not. */
  receive(notice: T): "early" | "current" | "stale" {
    if (this.current === notice.jobId) {
      return "current";
    }
    if (this.waiting && this.current === null) {
      this.early.set(notice.jobId, notice);
      return "early";
    }
    return "stale";
  }

  /**
   * The command returned `jobId`. `matched` arrived before that, if anything
   * did. `stale` are other events buffered in the same window.
   */
  note(jobId: number): { matched: T | null; stale: T[] } {
    this.waiting = false;
    this.current = jobId;
    const matched = this.early.get(jobId) ?? null;
    const stale = [...this.early.values()].filter((notice) => notice.jobId !== jobId);
    this.early.clear();
    return { matched, stale };
  }
}

/**
 * Preview sizes from the command contract. A drag asks for a 720 px long
 * edge. Releasing the pointer asks for the screen. The backend supersedes
 * the older job; this class only picks the size and does not debounce.
 */
export class PreviewSchedule {
  static readonly dragEdge = 720;

  static longEdge(phase: "drag" | "settle", screenLongEdge: number): number {
    if (phase === "drag") {
      return PreviewSchedule.dragEdge;
    }
    if (!Number.isFinite(screenLongEdge) || screenLongEdge <= 0) {
      return PreviewSchedule.dragEdge;
    }
    return Math.max(1, Math.round(screenLongEdge));
  }
}
