/** 8-byte little-endian header, then RGBA8. Matches `frame.rs`. */

export interface PackedFrame {
  width: number;
  height: number;
  rgba: Uint8ClampedArray<ArrayBuffer>;
}

export class FrameBuffer {
  static readonly header = 8;

  static decode(value: ArrayBuffer | Uint8Array): PackedFrame {
    const bytes = value instanceof Uint8Array ? value : new Uint8Array(value);
    if (bytes.byteLength < FrameBuffer.header) {
      throw new Error("The frame was shorter than its header.");
    }
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const width = view.getUint32(0, true);
    const height = view.getUint32(4, true);
    const rgba = bytes.subarray(FrameBuffer.header);
    if (rgba.byteLength !== width * height * 4) {
      throw new Error("The frame length did not match its width and height.");
    }
    const copy = new Uint8ClampedArray(new ArrayBuffer(rgba.byteLength));
    copy.set(rgba);
    return { width, height, rgba: copy };
  }
}
