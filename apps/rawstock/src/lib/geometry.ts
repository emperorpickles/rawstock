import type { CropRect } from "./contract.ts";

/** Crop aspect choices. `original` is the photo's own ratio and a full frame. */
export type AspectId = "original" | "free" | "3:2" | "4:3" | "1:1" | "16:9";

export type CropHandle = "move" | "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

export interface FrameRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

const ASPECTS: readonly AspectId[] = ["original", "free", "3:2", "4:3", "1:1", "16:9"];

export class GeometryView {
  static orientedSize(
    width: number,
    height: number,
    quarterTurns: number,
  ): { width: number; height: number } {
    const turns = ((quarterTurns % 4) + 4) % 4;
    if (turns % 2 === 1) {
      return { width: height, height: width };
    }
    return { width, height };
  }

  /** Largest upright rect of `imageWidth`×`imageHeight` inside the view. */
  static fit(imageWidth: number, imageHeight: number, viewWidth: number, viewHeight: number): FrameRect {
    if (viewWidth <= 0 || viewHeight <= 0 || imageWidth <= 0 || imageHeight <= 0) {
      return { x: 0, y: 0, width: 0, height: 0 };
    }
    const scale = Math.min(viewWidth / imageWidth, viewHeight / imageHeight);
    const width = imageWidth * scale;
    const height = imageHeight * scale;
    return {
      x: (viewWidth - width) / 2,
      y: (viewHeight - height) / 2,
      width,
      height,
    };
  }
}

export class CropMath {
  static readonly minSize = 0.05;

  static aspects(): readonly AspectId[] {
    return ASPECTS;
  }

  static label(id: AspectId): string {
    if (id === "original") {
      return "Original";
    }
    if (id === "free") {
      return "Free";
    }
    return id;
  }

  /** Pixel width/height. Null means the crop is not locked. */
  static pixelRatio(id: AspectId, imageWidth: number, imageHeight: number): number | null {
    switch (id) {
      case "free":
        return null;
      case "original":
        return imageWidth > 0 && imageHeight > 0 ? imageWidth / imageHeight : null;
      case "3:2":
        return 3 / 2;
      case "4:3":
        return 4 / 3;
      case "1:1":
        return 1;
      case "16:9":
        return 16 / 9;
      default:
        return null;
    }
  }

  /** Width/height in normalised units for a pixel ratio on this image. */
  static normalizedRatio(pixelRatio: number, imageWidth: number, imageHeight: number): number {
    if (!(imageWidth > 0) || !(imageHeight > 0)) {
      return pixelRatio;
    }
    return (pixelRatio * imageHeight) / imageWidth;
  }

  static isFull(crop: CropRect | undefined): boolean {
    if (!crop) {
      return true;
    }
    return crop.x <= 1e-4 && crop.y <= 1e-4 && crop.w >= 1 - 1e-4 && crop.h >= 1 - 1e-4;
  }

  /** Largest centered crop with pixel ratio `pixelRatio`. */
  static maximum(pixelRatio: number, imageWidth: number, imageHeight: number): CropRect {
    if (!(pixelRatio > 0) || !(imageWidth > 0) || !(imageHeight > 0)) {
      return { x: 0, y: 0, w: 1, h: 1 };
    }
    const imageRatio = imageWidth / imageHeight;
    if (Math.abs(imageRatio - pixelRatio) < 1e-4) {
      return { x: 0, y: 0, w: 1, h: 1 };
    }
    if (imageRatio > pixelRatio) {
      const w = pixelRatio / imageRatio;
      return { x: (1 - w) / 2, y: 0, w, h: 1 };
    }
    const h = imageRatio / pixelRatio;
    return { x: 0, y: (1 - h) / 2, w: 1, h };
  }

  /** A starting free crop that does not match the image's own ratio. */
  static freeStart(): CropRect {
    return { x: 0.08, y: 0.14, w: 0.84, h: 0.68 };
  }

  static infer(crop: CropRect | undefined, imageWidth: number, imageHeight: number): AspectId {
    if (CropMath.isFull(crop)) {
      return "original";
    }
    if (!crop || !(imageWidth > 0) || !(imageHeight > 0) || !(crop.h > 0)) {
      return "free";
    }
    const pixel = (crop.w * imageWidth) / (crop.h * imageHeight);
    const image = imageWidth / imageHeight;
    if (Math.abs(pixel - image) < 0.02) {
      return "original";
    }
    const named: Array<[AspectId, number]> = [
      ["1:1", 1],
      ["4:3", 4 / 3],
      ["3:2", 3 / 2],
      ["16:9", 16 / 9],
    ];
    for (const [id, ratio] of named) {
      if (Math.abs(pixel - ratio) < 0.02) {
        return id;
      }
    }
    return "free";
  }

  /**
   * Clockwise quarter turn of a crop in y-down normalised space.
   * The left half becomes the bottom half.
   */
  static clockwise(crop: CropRect): CropRect {
    return {
      x: crop.y,
      y: 1 - crop.x - crop.w,
      w: crop.h,
      h: crop.w,
    };
  }

  static counterClockwise(crop: CropRect): CropRect {
    return {
      x: 1 - crop.y - crop.h,
      y: crop.x,
      w: crop.h,
      h: crop.w,
    };
  }

  static drag(
    crop: CropRect,
    handle: CropHandle,
    dx: number,
    dy: number,
    pixelRatio: number | null,
    imageWidth: number,
    imageHeight: number,
  ): CropRect {
    if (handle === "move") {
      return CropMath.place(crop.x + dx, crop.y + dy, crop.w, crop.h);
    }
    const ratio =
      pixelRatio == null ? null : CropMath.normalizedRatio(pixelRatio, imageWidth, imageHeight);
    let x = crop.x;
    let y = crop.y;
    let w = crop.w;
    let h = crop.h;
    const east = handle === "e" || handle === "ne" || handle === "se";
    const west = handle === "w" || handle === "nw" || handle === "sw";
    const south = handle === "s" || handle === "se" || handle === "sw";
    const north = handle === "n" || handle === "ne" || handle === "nw";
    if (east) {
      w += dx;
    }
    if (west) {
      x += dx;
      w -= dx;
    }
    if (south) {
      h += dy;
    }
    if (north) {
      y += dy;
      h -= dy;
    }
    if (ratio != null && ratio > 0) {
      const anchorRight = west && !east;
      const anchorBottom = north && !south;
      if (east || west) {
        h = w / ratio;
      } else {
        w = h * ratio;
      }
      if (anchorRight) {
        x = crop.x + crop.w - w;
      }
      if (anchorBottom) {
        y = crop.y + crop.h - h;
      }
    }
    return CropMath.place(x, y, w, h);
  }

  /**
   * Degrees to rotate so the drawn line becomes horizontal, clamped to ±45.
   * A line shorter than `minPx` is ignored.
   */
  static horizon(x0: number, y0: number, x1: number, y1: number, minPx = 8): number | null {
    const dx = x1 - x0;
    const dy = y1 - y0;
    if (dx * dx + dy * dy < minPx * minPx) {
      return null;
    }
    let degrees = (Math.atan2(dy, dx) * 180) / Math.PI;
    if (degrees > 90) {
      degrees -= 180;
    }
    if (degrees < -90) {
      degrees += 180;
    }
    // Adding zero turns a horizontal line's -0 into 0.
    return Math.min(45, Math.max(-45, -degrees)) + 0;
  }

  private static place(x: number, y: number, w: number, h: number): CropRect {
    let width = Math.min(1, Math.max(CropMath.minSize, w));
    let height = Math.min(1, Math.max(CropMath.minSize, h));
    if (width > 1) {
      width = 1;
    }
    if (height > 1) {
      height = 1;
    }
    const left = Math.min(Math.max(0, x), 1 - width);
    const top = Math.min(Math.max(0, y), 1 - height);
    return { x: left, y: top, w: width, h: height };
  }
}
