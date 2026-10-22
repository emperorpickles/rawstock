import type { Frame } from "./types";

/** Long edge of the fit proxy. Matches `DRAG_LONG_EDGE` and `SETTLED_LONG_EDGE`. */
export class ProxySize {
  static readonly drag = 720;
  static readonly settled = 1440;
}

/** Binary frame returned by `render_frame`. See `frame.rs` for the layout. */
export class FrameBuffer {
  static readonly magic = 0x52535031;
  static readonly header = 32;

  static decode(value: ArrayBuffer | Uint8Array): Frame {
    const buffer = FrameBuffer.copy(value);
    const view = new DataView(buffer);
    if (buffer.byteLength < FrameBuffer.header) {
      throw new Error("The preview was shorter than its header.");
    }
    const magic = view.getUint32(0, true);
    const version = view.getUint32(4, true);
    if (magic !== FrameBuffer.magic || version !== 1) {
      throw new Error("The preview header was not a Rawstock frame.");
    }
    const ticket = Number(view.getBigUint64(8, true));
    const width = view.getUint32(16, true);
    const height = view.getUint32(20, true);
    const fidelity = view.getUint8(24) === 1 ? "exact" : "preview";
    const bytes = new Uint8Array(buffer, FrameBuffer.header);
    const expected = width * height * 3;
    if (bytes.byteLength !== expected) {
      throw new Error(`Expected ${expected} RGB bytes, received ${bytes.byteLength}.`);
    }
    return { ticket, width, height, fidelity, bytes };
  }

  static copy(value: ArrayBuffer | Uint8Array): ArrayBuffer {
    if (value instanceof ArrayBuffer) {
      return value;
    }
    const copy = new Uint8Array(value.byteLength);
    copy.set(value);
    return copy.buffer;
  }
}

/** RGBA expansion for `ImageData`, which has no RGB constructor. */
export class RgbaPixels {
  static fromRgb(rgb: Uint8Array, width: number, height: number): Uint8ClampedArray<ArrayBuffer> {
    const buffer = new ArrayBuffer(width * height * 4);
    const out = new Uint8ClampedArray(buffer);
    RgbaPixels.fill(rgb, out, 0, width * height);
    return out;
  }

  static fill(rgb: Uint8Array, out: Uint8ClampedArray, startPixel: number, endPixel: number): void {
    for (let pixel = startPixel; pixel < endPixel; pixel += 1) {
      const source = pixel * 3;
      const dest = pixel * 4;
      out[dest] = rgb[source] ?? 0;
      out[dest + 1] = rgb[source + 1] ?? 0;
      out[dest + 2] = rgb[source + 2] ?? 0;
      out[dest + 3] = 255;
    }
  }
}

/** A decoded preview kept as one bitmap. The RGB bytes are not retained. */
export interface HeldPicture {
  bitmap: ImageBitmap;
  fidelity: Frame["fidelity"];
}

/**
 * Turns a worker frame into a bitmap. The copy yields between slices so a
 * pan already on screen can paint while a full frame arrives.
 */
export class FrameBitmap {
  static async create(frame: Frame, live: () => boolean = () => true): Promise<HeldPicture | null> {
    const { width, height } = frame;
    if (width === 0 || height === 0 || !live()) return null;
    const buffer = new ArrayBuffer(width * height * 4);
    const out = new Uint8ClampedArray(buffer);
    const pixels = width * height;
    const slice = 512 * 1024;
    for (let start = 0; start < pixels; start += slice) {
      if (!live()) return null;
      RgbaPixels.fill(frame.bytes, out, start, Math.min(pixels, start + slice));
      if (start + slice < pixels) {
        await new Promise((resolve) => setTimeout(resolve, 0));
      }
    }
    if (!live()) return null;
    const bitmap = await createImageBitmap(new ImageData(out, width, height));
    if (!live()) {
      bitmap.close();
      return null;
    }
    return { bitmap, fidelity: frame.fidelity };
  }
}

/** Bitmap currently on the stage. `full` is the exact frame; otherwise it is a fit proxy. */
export interface DrawnPicture {
  bitmap: ImageBitmap;
  full: boolean;
}

/**
 * Paints either the fit proxy or a rectangle of the cached full frame.
 * The source bitmap is not rebuilt for a pan or a zoom.
 */
export class ViewportCanvas {
  static draw(
    canvas: HTMLCanvasElement,
    picture: DrawnPicture,
    mode: "fit" | "pixel",
    panX: number,
    panY: number,
    viewWidth: number,
    viewHeight: number,
    displayWidth: number,
    displayHeight: number,
  ): void {
    const ctx = canvas.getContext("2d");
    if (!ctx || viewWidth < 1 || viewHeight < 1 || displayWidth < 1 || displayHeight < 1) return;
    canvas.width = viewWidth;
    canvas.height = viewHeight;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = "#0c0b0a";
    ctx.fillRect(0, 0, viewWidth, viewHeight);
    if (mode === "fit") {
      const scale = Math.min(viewWidth / picture.bitmap.width, viewHeight / picture.bitmap.height);
      const width = picture.bitmap.width * scale;
      const height = picture.bitmap.height * scale;
      ctx.imageSmoothingEnabled = true;
      ctx.drawImage(
        picture.bitmap,
        (viewWidth - width) / 2,
        (viewHeight - height) / 2,
        width,
        height,
      );
      return;
    }
    const crop = CropRect.visible(displayWidth, displayHeight, viewWidth, viewHeight, panX, panY);
    const destX = Math.floor((viewWidth - crop.width) / 2);
    const destY = Math.floor((viewHeight - crop.height) / 2);
    const scaleX = picture.bitmap.width / displayWidth;
    const scaleY = picture.bitmap.height / displayHeight;
    ctx.imageSmoothingEnabled = !picture.full;
    ctx.drawImage(
      picture.bitmap,
      crop.x * scaleX,
      crop.y * scaleY,
      crop.width * scaleX,
      crop.height * scaleY,
      destX,
      destY,
      crop.width,
      crop.height,
    );
  }
}

/** A 90° turn of the view, mapping the pan so the same point stays centered. */
export class ViewTurn {
  static panAfter(
    direction: "clockwise" | "counterClockwise",
    panX: number,
    panY: number,
    viewWidth: number,
    viewHeight: number,
    imageWidth: number,
    imageHeight: number,
  ): { x: number; y: number } {
    const centerX = panX + viewWidth / 2;
    const centerY = panY + viewHeight / 2;
    const moved =
      direction === "clockwise"
        ? { x: imageHeight - centerY, y: centerX }
        : { x: centerY, y: imageWidth - centerX };
    const nextWidth = imageHeight;
    const nextHeight = imageWidth;
    const crop = CropRect.visible(
      nextWidth,
      nextHeight,
      viewWidth,
      viewHeight,
      moved.x - viewWidth / 2,
      moved.y - viewHeight / 2,
    );
    return { x: crop.x, y: crop.y };
  }
}

/** Display-space crop clamped to the oriented frame. */
export class CropRect {
  static visible(
    displayWidth: number,
    displayHeight: number,
    viewWidth: number,
    viewHeight: number,
    originX: number,
    originY: number,
  ): { x: number; y: number; width: number; height: number } {
    const width = Math.max(1, Math.min(viewWidth, displayWidth));
    const height = Math.max(1, Math.min(viewHeight, displayHeight));
    const maxX = Math.max(0, displayWidth - width);
    const maxY = Math.max(0, displayHeight - height);
    return {
      x: Math.min(maxX, Math.max(0, Math.round(originX))),
      y: Math.min(maxY, Math.max(0, Math.round(originY))),
      width,
      height,
    };
  }

  static centered(
    displayWidth: number,
    displayHeight: number,
    viewWidth: number,
    viewHeight: number,
  ): { x: number; y: number } {
    const crop = CropRect.visible(displayWidth, displayHeight, viewWidth, viewHeight, 0, 0);
    return {
      x: Math.max(0, Math.round((displayWidth - crop.width) / 2)),
      y: Math.max(0, Math.round((displayHeight - crop.height) / 2)),
    };
  }
}
