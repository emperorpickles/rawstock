import type { CropRect, GeometryEdit } from "./contract.ts";

/** Crop aspect choices. `original` is the photo's own ratio and a full frame. */
export type AspectId = "original" | "free" | "3:2" | "4:3" | "1:1" | "16:9";

export type CropHandle = "move" | "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

export interface FrameRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Point {
  x: number;
  y: number;
}

/** A 2D affine map, in canvas `setTransform` order: x' = a·x + c·y + e. */
export interface Affine {
  a: number;
  b: number;
  c: number;
  d: number;
  e: number;
  f: number;
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

/**
 * The edit's framing, as `rawstock_core::geometry::Framing` applies it to an
 * export. Quarter turns rotate the camera frame clockwise. Straighten turns
 * that frame about its centre, clockwise for a positive angle, and scales
 * it so the rotated image still covers the whole turned frame. The crop is
 * normalised in that frame. One frame unit is one export pixel.
 */
export class Framing {
  /** The smallest scale at which the rotated image covers the frame. */
  static coverScale(degrees: number, width: number, height: number): number {
    if (Math.abs(degrees) < 1e-6 || !(width > 0) || !(height > 0)) {
      return 1;
    }
    const radians = (Math.abs(degrees) * Math.PI) / 180;
    const sin = Math.sin(radians);
    const cos = Math.cos(radians);
    return Math.max(cos + (sin * height) / width, cos + (sin * width) / height);
  }

  /** Camera-frame pixels to turned, straightened frame units. */
  static imageToFrame(imageWidth: number, imageHeight: number, quarterTurns: number, degrees: number): Affine {
    const turns = ((quarterTurns % 4) + 4) % 4;
    const frame = GeometryView.orientedSize(imageWidth, imageHeight, turns);
    const scale = Framing.coverScale(degrees, frame.width, frame.height);
    // Exact quarter turns, then the straighten angle.
    const [turnCos, turnSin] = [
      [1, 0],
      [0, 1],
      [-1, 0],
      [0, -1],
    ][turns];
    const radians = (degrees * Math.PI) / 180;
    const cos = Math.cos(radians);
    const sin = Math.sin(radians);
    const a = scale * (cos * turnCos - sin * turnSin);
    const b = scale * (sin * turnCos + cos * turnSin);
    const c = -b;
    const d = a;
    const cx = imageWidth / 2;
    const cy = imageHeight / 2;
    return {
      a,
      b,
      c,
      d,
      e: frame.width / 2 - (a * cx + c * cy),
      f: frame.height / 2 - (b * cx + d * cy),
    };
  }

  static apply(map: Affine, point: Point): Point {
    return { x: map.a * point.x + map.c * point.y + map.e, y: map.b * point.x + map.d * point.y + map.f };
  }

  static invert(map: Affine): Affine {
    const det = map.a * map.d - map.b * map.c;
    const a = map.d / det;
    const b = -map.b / det;
    const c = -map.c / det;
    const d = map.a / det;
    return { a, b, c, d, e: -(a * map.e + c * map.f), f: -(b * map.e + d * map.f) };
  }

  /** `first`, then `second`. */
  static then(first: Affine, second: Affine): Affine {
    return {
      a: second.a * first.a + second.c * first.b,
      b: second.b * first.a + second.d * first.b,
      c: second.a * first.c + second.c * first.d,
      d: second.b * first.c + second.d * first.d,
      e: second.a * first.e + second.c * first.f + second.e,
      f: second.b * first.e + second.d * first.f + second.f,
    };
  }

  /**
   * The crop in whole pixels of a `width`×`height` turned frame, rounded
   * outward and at least 1×1, as `Framing::crop_pixels` in rawstock-core.
   */
  static cropPixels(crop: CropRect | undefined, width: number, height: number): FrameRect {
    if (!crop) {
      return { x: 0, y: 0, width, height };
    }
    const left = Math.min(Math.max(0, Math.floor(crop.x * width)), Math.max(0, width - 1));
    const top = Math.min(Math.max(0, Math.floor(crop.y * height)), Math.max(0, height - 1));
    const right = Math.min(width, Math.ceil((crop.x + crop.w) * width));
    const bottom = Math.min(height, Math.ceil((crop.y + crop.h) * height));
    return { x: left, y: top, width: Math.max(right, left + 1) - left, height: Math.max(bottom, top + 1) - top };
  }

  /** `map`, then the crop's top-left moved to the origin: the export's own pixels. */
  static cropped(map: Affine, crop: FrameRect): Affine {
    return Framing.then(map, { a: 1, b: 0, c: 0, d: 1, e: -crop.x, f: -crop.y });
  }

  /**
   * A view centre, normalised in frame rect `from`, moved to rect `to` so
   * the same point of the photo stays in the middle of the view.
   */
  static recenter(center: Point, from: FrameRect, to: FrameRect): Point {
    if (!(to.width > 0) || !(to.height > 0)) {
      return center;
    }
    return {
      x: (from.x + center.x * from.width - to.x) / to.width,
      y: (from.y + center.y * from.height - to.y) / to.height,
    };
  }

  /** Whole camera-frame pixels under a frame rectangle, clipped to the image. */
  static imageBounds(map: Affine, rect: FrameRect, imageWidth: number, imageHeight: number): FrameRect {
    const back = Framing.invert(map);
    const corners = [
      { x: rect.x, y: rect.y },
      { x: rect.x + rect.width, y: rect.y },
      { x: rect.x, y: rect.y + rect.height },
      { x: rect.x + rect.width, y: rect.y + rect.height },
    ].map((corner) => Framing.apply(back, corner));
    const x0 = Math.max(0, Math.floor(Math.min(...corners.map((p) => p.x)) + 1e-9));
    const y0 = Math.max(0, Math.floor(Math.min(...corners.map((p) => p.y)) + 1e-9));
    const x1 = Math.min(imageWidth, Math.ceil(Math.max(...corners.map((p) => p.x)) - 1e-9));
    const y1 = Math.min(imageHeight, Math.ceil(Math.max(...corners.map((p) => p.y)) - 1e-9));
    return { x: x0, y: y0, width: Math.max(0, x1 - x0), height: Math.max(0, y1 - y0) };
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

  /** The straighten angle after levelling a line drawn on a photo already turned by `base`. */
  static levelled(base: number, horizon: number): number {
    return Math.min(45, Math.max(-45, base + horizon)) + 0;
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

/** Crop mode's own keys and checks. */
export class CropMode {
  /**
   * Enter keeps the crop and Esc cancels it. Neither fires with a modifier,
   * from a text field, or as Enter on a control that Enter already presses.
   */
  static isExitKey(
    event: { key: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean; repeat?: boolean },
    targetTag: string,
  ): boolean {
    if (event.metaKey || event.ctrlKey || event.altKey || event.repeat) {
      return false;
    }
    const tag = targetTag.toUpperCase();
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
      return false;
    }
    if (event.key === "Escape") {
      return true;
    }
    return event.key === "Enter" && tag !== "BUTTON" && tag !== "A";
  }

  static sameGeometry(a: GeometryEdit, b: GeometryEdit): boolean {
    const crop = (rect: CropRect | undefined) => (rect ? `${rect.x},${rect.y},${rect.w},${rect.h}` : "");
    return a.quarterTurns === b.quarterTurns && a.straightenDeg === b.straightenDeg && crop(a.crop) === crop(b.crop);
  }
}
