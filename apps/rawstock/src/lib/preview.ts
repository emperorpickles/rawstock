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
