/**
 * After `cache:ready`, a frame that fits in the budget is taken once and
 * panned here. `get_region` is only for a larger frame or memory pressure.
 * An X-T2 frame is about 96 MB, under this budget.
 */
export class Delivery {
  static readonly byteBudget = 128 * 1024 * 1024;

  static choose(width: number, height: number, memoryPressure: boolean): "texture" | "region" {
    if (memoryPressure || width * height * 4 > Delivery.byteBudget) {
      return "region";
    }
    return "texture";
  }
}

/** Latest job per image and kind. Older events are ignored. */
export class JobTracker {
  private readonly latest = new Map<string, number>();

  note(imageId: string, kind: string, jobId: number): void {
    this.latest.set(`${imageId}:${kind}`, jobId);
  }

  current(imageId: string, kind: string, jobId: number): boolean {
    return this.latest.get(`${imageId}:${kind}`) === jobId;
  }
}
