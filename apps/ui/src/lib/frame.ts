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
    for (let index = 0, pixel = 0; index < rgb.length; index += 3, pixel += 4) {
      out[pixel] = rgb[index] ?? 0;
      out[pixel + 1] = rgb[index + 1] ?? 0;
      out[pixel + 2] = rgb[index + 2] ?? 0;
      out[pixel + 3] = 255;
    }
    return out;
  }
}

export class PreviewCanvas {
  static async draw(
    canvas: HTMLCanvasElement,
    frame: Frame,
    mode: "fit" | "pixel",
    current: () => boolean = () => true,
  ): Promise<void> {
    const rgba = RgbaPixels.fromRgb(frame.bytes, frame.width, frame.height);
    const bitmap = await createImageBitmap(new ImageData(rgba, frame.width, frame.height));
    if (!current()) {
      bitmap.close();
      return;
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) {
      bitmap.close();
      return;
    }
    if (mode === "pixel") {
      canvas.width = frame.width;
      canvas.height = frame.height;
      const dpr = window.devicePixelRatio || 1;
      canvas.style.width = `${frame.width / dpr}px`;
      canvas.style.height = `${frame.height / dpr}px`;
      ctx.drawImage(bitmap, 0, 0);
      bitmap.close();
      return;
    }
    canvas.style.width = "";
    canvas.style.height = "";
    const stage = canvas.parentElement;
    const cssWidth = stage?.clientWidth ?? frame.width;
    const cssHeight = stage?.clientHeight ?? frame.height;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.max(1, Math.round(cssWidth * dpr));
    canvas.height = Math.max(1, Math.round(cssHeight * dpr));
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssWidth, cssHeight);
    const scale = Math.min(cssWidth / frame.width, cssHeight / frame.height);
    const width = frame.width * scale;
    const height = frame.height * scale;
    const x = (cssWidth - width) / 2;
    const y = (cssHeight - height) / 2;
    ctx.imageSmoothingEnabled = true;
    ctx.drawImage(bitmap, x, y, width, height);
    bitmap.close();
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
