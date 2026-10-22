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

  /** Visible source rectangle, in oriented pixels, for a client-side pan. */
  static visibleRect(
    imageWidth: number,
    imageHeight: number,
    viewWidth: number,
    viewHeight: number,
    zoom: number,
    panX: number,
    panY: number,
  ): { x: number; y: number; width: number; height: number } {
    const width = Math.max(1, Math.min(imageWidth, viewWidth / zoom));
    const height = Math.max(1, Math.min(imageHeight, viewHeight / zoom));
    const x = clamp(panX, 0, Math.max(0, imageWidth - width));
    const y = clamp(panY, 0, Math.max(0, imageHeight - height));
    return {
      x: Math.floor(x),
      y: Math.floor(y),
      width: Math.max(1, Math.ceil(width)),
      height: Math.max(1, Math.ceil(height)),
    };
  }
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
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
