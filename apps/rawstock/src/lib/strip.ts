import type { EditSettings, ImageInfo } from "./contract.ts";

export interface StripRequest {
  imageId: string;
  settings: EditSettings;
}

/** Payload of `strip:ready`. Matches `strip.rs`. */
export interface StripReady {
  imageId: string;
  settingsHash: string;
  exact: boolean;
}

export class StripOrder {
  /**
   * The photos to draw, nearest the selected one first, alternating right
   * then left. Photos that failed to open are left out. Before anything is
   * selected the strip fills from the left.
   */
  static requests(
    images: ImageInfo[],
    selected: string,
    settingsFor: (image: ImageInfo) => EditSettings,
  ): StripRequest[] {
    const usable = images.filter((image) => image.error == null);
    const at = Math.max(
      0,
      usable.findIndex((image) => image.imageId === selected),
    );
    const order: ImageInfo[] = [];
    for (let step = 0; order.length < usable.length; step += 1) {
      const right = usable[at + step];
      if (right) {
        order.push(right);
      }
      const left = step > 0 ? usable[at - step] : undefined;
      if (left) {
        order.push(left);
      }
    }
    return order.map((image) => ({ imageId: image.imageId, settings: settingsFor(image) }));
  }

  /** A stable key, so an unchanged queue is not sent again. */
  static key(requests: StripRequest[]): string {
    return JSON.stringify(requests);
  }
}
