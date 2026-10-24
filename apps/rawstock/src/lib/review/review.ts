/**
 * Review view logic: zoom and pan math, the clipping mask, the split
 * divider, and the job board that feeds the viewer.
 *
 * Fit shows the preview proxy. 100% and 200% show one `develop_full` per
 * settings state, taken once as a texture (or read through `get_region`
 * when the frame is too large), so panning never develops. 200% is a
 * nearest-neighbour upscale of the 100% pixels on the client.
 */

import { Delivery } from "../delivery.ts";
import { FrameBuffer } from "../frame.ts";
import type { PackedFrame } from "../frame.ts";
import type {
  AppError,
  CacheEvictedEvent,
  CacheReadyEvent,
  CancelledEvent,
  DoneEvent,
  EditSettings,
  FailedEvent,
  ImageRefinedEvent,
  JobKind,
  JobStarted,
} from "../contract.ts";

export type Zoom = "fit" | 1 | 2;
export type Side = "after" | "before";

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Point {
  x: number;
  y: number;
}

/** A bitmap and the rectangle of image space it covers. */
export interface Layer {
  frame: PackedFrame;
  rect: Rect;
  /** One frame pixel per image pixel, so it is drawn nearest-neighbour. */
  exact: boolean;
}

export interface ReviewView {
  imageId: string;
  /** Image space: full-resolution pixels once known, else the proxy's. */
  width: number;
  height: number;
  after: Layer | null;
  before: Layer | null;
  /** At 100% or 200%, a newer full develop is running behind this frame. */
  updating: boolean;
  /** The after layer is the preview proxy, not full resolution. */
  proxy: boolean;
  /** The before layer is the proxy while the after layer is exact. */
  beforeProxy: boolean;
  delivery: "texture" | "region";
}

/** Zoom and pan math, in device pixels. Pan is a normalised image centre. */
export class Viewport {
  static fitScale(imageWidth: number, imageHeight: number, viewWidth: number, viewHeight: number, gutter: number): number {
    if (imageWidth <= 0 || imageHeight <= 0) {
      return 1;
    }
    const width = Math.max(1, viewWidth - 2 * gutter);
    const height = Math.max(1, viewHeight - 2 * gutter);
    return Math.min(width / imageWidth, height / imageHeight);
  }

  static scale(zoom: Zoom, imageWidth: number, imageHeight: number, viewWidth: number, viewHeight: number, gutter: number): number {
    return zoom === "fit" ? Viewport.fitScale(imageWidth, imageHeight, viewWidth, viewHeight, gutter) : zoom;
  }

  /** Keep the image inside the view, with `gutter` past each edge. */
  static clampCenter(center: Point, imageWidth: number, imageHeight: number, viewWidth: number, viewHeight: number, scale: number, gutter: number): Point {
    return {
      x: Viewport.clampAxis(center.x, imageWidth * scale, viewWidth, gutter),
      y: Viewport.clampAxis(center.y, imageHeight * scale, viewHeight, gutter),
    };
  }

  /** Screen position of image pixel (0, 0). */
  static origin(center: Point, imageWidth: number, imageHeight: number, viewWidth: number, viewHeight: number, scale: number): Point {
    return {
      x: viewWidth / 2 - center.x * imageWidth * scale,
      y: viewHeight / 2 - center.y * imageHeight * scale,
    };
  }

  static imageAt(screen: Point, center: Point, imageWidth: number, imageHeight: number, viewWidth: number, viewHeight: number, scale: number): Point {
    const origin = Viewport.origin(center, imageWidth, imageHeight, viewWidth, viewHeight, scale);
    return { x: (screen.x - origin.x) / scale, y: (screen.y - origin.y) / scale };
  }

  /** The centre that puts image point `image` under screen point `screen`. */
  static centerKeeping(image: Point, screen: Point, imageWidth: number, imageHeight: number, viewWidth: number, viewHeight: number, scale: number): Point {
    return {
      x: (image.x * scale - (screen.x - viewWidth / 2)) / (imageWidth * scale),
      y: (image.y * scale - (screen.y - viewHeight / 2)) / (imageHeight * scale),
    };
  }

  /** Whole image pixels on screen, clipped to the image. */
  static visibleRect(center: Point, imageWidth: number, imageHeight: number, viewWidth: number, viewHeight: number, scale: number): Rect {
    const origin = Viewport.origin(center, imageWidth, imageHeight, viewWidth, viewHeight, scale);
    const x0 = Math.max(0, Math.floor(-origin.x / scale));
    const y0 = Math.max(0, Math.floor(-origin.y / scale));
    const x1 = Math.min(imageWidth, Math.ceil((viewWidth - origin.x) / scale));
    const y1 = Math.min(imageHeight, Math.ceil((viewHeight - origin.y) / scale));
    return { x: x0, y: y0, width: Math.max(0, x1 - x0), height: Math.max(0, y1 - y0) };
  }

  /** `rect` grown by half its size on each side, inside the image. */
  static padded(rect: Rect, imageWidth: number, imageHeight: number): Rect {
    const x0 = Math.max(0, Math.floor(rect.x - rect.width / 2));
    const y0 = Math.max(0, Math.floor(rect.y - rect.height / 2));
    const x1 = Math.min(imageWidth, Math.ceil(rect.x + rect.width * 1.5));
    const y1 = Math.min(imageHeight, Math.ceil(rect.y + rect.height * 1.5));
    return { x: x0, y: y0, width: Math.max(0, x1 - x0), height: Math.max(0, y1 - y0) };
  }

  static intersect(a: Rect, b: Rect): Rect | null {
    const x0 = Math.max(a.x, b.x);
    const y0 = Math.max(a.y, b.y);
    const x1 = Math.min(a.x + a.width, b.x + b.width);
    const y1 = Math.min(a.y + a.height, b.y + b.height);
    if (x1 <= x0 || y1 <= y0) {
      return null;
    }
    return { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
  }

  private static clampAxis(center: number, drawn: number, view: number, gutter: number): number {
    if (drawn <= 0 || drawn + 2 * gutter <= view) {
      return 0.5;
    }
    const edge = (view / 2 - gutter) / drawn;
    return Math.min(1 - edge, Math.max(edge, center));
  }
}

/**
 * Pixels clipped in any channel of the displayed sRGB bitmap. A channel at
 * 255 is a highlight clip and wins over a channel at 0 in the same pixel.
 */
export class Clipping {
  static readonly highlight: readonly [number, number, number] = [255, 48, 48];
  static readonly shadow: readonly [number, number, number] = [40, 120, 255];

  static mask(rgba: Uint8ClampedArray, width: number, height: number): Uint8ClampedArray<ArrayBuffer> {
    const out = new Uint8ClampedArray(new ArrayBuffer(width * height * 4));
    const [hr, hg, hb] = Clipping.highlight;
    const [sr, sg, sb] = Clipping.shadow;
    for (let index = 0; index < width * height * 4; index += 4) {
      const r = rgba[index];
      const g = rgba[index + 1];
      const b = rgba[index + 2];
      if (r === 255 || g === 255 || b === 255) {
        out[index] = hr;
        out[index + 1] = hg;
        out[index + 2] = hb;
        out[index + 3] = 255;
      } else if (r === 0 || g === 0 || b === 0) {
        out[index] = sr;
        out[index + 1] = sg;
        out[index + 2] = sb;
        out[index + 3] = 255;
      }
    }
    return out;
  }

  static counts(rgba: Uint8ClampedArray): { highlights: number; shadows: number } {
    let highlights = 0;
    let shadows = 0;
    for (let index = 0; index < rgba.length; index += 4) {
      const r = rgba[index];
      const g = rgba[index + 1];
      const b = rgba[index + 2];
      if (r === 255 || g === 255 || b === 255) {
        highlights += 1;
      } else if (r === 0 || g === 0 || b === 0) {
        shadows += 1;
      }
    }
    return { highlights, shadows };
  }
}

/** The split divider, as a fraction of the viewer's width. */
export class Divider {
  static readonly step = 0.02;
  static readonly bigStep = 0.1;
  static readonly min = 0;
  static readonly max = 1;

  static clamp(position: number): number {
    return Math.min(Divider.max, Math.max(Divider.min, position));
  }

  /** The new position for a key, or `null` when the key does not move it. */
  static key(position: number, key: string, shift: boolean): number | null {
    const step = shift ? Divider.bigStep : Divider.step;
    switch (key) {
      case "ArrowLeft":
      case "ArrowDown":
        return Divider.clamp(position - step);
      case "ArrowRight":
      case "ArrowUp":
        return Divider.clamp(position + step);
      case "Home":
        return Divider.min;
      case "End":
        return Divider.max;
      default:
        return null;
    }
  }
}

/** The commands the review view sends. `Host` fits. */
export interface ReviewHost {
  developPreview(imageId: string, settings: EditSettings, longEdge: number, compare: boolean): Promise<JobStarted>;
  developFull(imageId: string, settings: EditSettings): Promise<JobStarted>;
  takeFrame(jobId: number): Promise<Uint8Array>;
  getRegion(args: { imageId: string; settingsHash: string } & Rect): Promise<Uint8Array>;
}

export interface ReviewCallbacks {
  onview: (view: ReviewView) => void;
  onerror: (error: AppError) => void;
  /** A job was started, for the progress line. */
  onjob?: (imageId: string, kind: JobKind, jobId: number) => void;
}

type Slot = "preview" | "full";

interface Job {
  id: number | null;
  adopting: boolean;
  side: Side;
  slot: Slot;
  key: string;
  edge: number;
  hash: string;
}

interface SideState {
  settings: EditSettings;
  key: string;
  preview: Layer | null;
  previewKey: string;
  previewEdge: number;
  /** The whole full frame, or in region delivery the latest crop. */
  full: Layer | null;
  fullKey: string;
  fullHash: string;
  failedPreview: string;
  failedFull: string;
}

type EarlyEvent =
  | { type: "done"; event: DoneEvent }
  | { type: "cancelled"; event: CancelledEvent }
  | { type: "failed"; event: FailedEvent };

/**
 * One review session for the selected image. The backend keeps one live job
 * per image and kind, so the after side goes first and the before side
 * waits for the slot. "Before" is the settings the image was first opened
 * with in this run.
 */
export class ReviewController {
  static readonly defaultEdge = 1536;

  private imageId = "";
  private sizeHint: { width: number; height: number } | null = null;
  private fullSize: { width: number; height: number } | null = null;
  private after: SideState | null = null;
  private before: SideState | null = null;
  private zoom: Zoom = "fit";
  private compare = false;
  private longEdge = ReviewController.defaultEdge;
  /** The adjust panel starts after-side previews. This controller does not. */
  private yieldAfter = false;
  private memoryPressure = false;
  private delivery: "texture" | "region" = "texture";
  private pending: Record<Slot, Job | null> = { preview: null, full: null };
  private known = new Map<number, Job>();
  private early = new Map<number, EarlyEvent>();
  /** Preview ids the adjust panel replaced. Their frames are discarded. */
  private abandoned = new Set<number>();
  private landed = new Map<string, Job>();
  private ready = new Map<string, { width: number; height: number }>();
  private opened = new Map<string, EditSettings>();
  private regionRect: Rect | null = null;
  private regionBusy = false;
  private regionAgain = false;

  private readonly host: ReviewHost;
  private readonly callbacks: ReviewCallbacks;

  constructor(host: ReviewHost, callbacks: ReviewCallbacks) {
    this.host = host;
    this.callbacks = callbacks;
  }

  /** Show `imageId`. The first settings seen for it become "before". */
  open(imageId: string, settings: EditSettings, size?: { width: number; height: number }): void {
    if (imageId === this.imageId) {
      this.update(settings);
      return;
    }
    if (!this.opened.has(imageId)) {
      this.opened.set(imageId, ReviewController.clone(settings));
    }
    this.imageId = imageId;
    this.sizeHint = size && size.width > 0 && size.height > 0 ? size : null;
    this.fullSize = null;
    this.after = ReviewController.side(settings);
    this.before = ReviewController.side(this.opened.get(imageId) ?? settings);
    this.pending = { preview: null, full: null };
    this.known.clear();
    this.early.clear();
    this.abandoned.clear();
    this.landed.clear();
    this.ready.clear();
    this.regionRect = null;
    this.delivery = this.memoryPressure ? "region" : "texture";
    this.emit();
    this.pump();
  }

  close(): void {
    this.imageId = "";
    this.after = null;
    this.before = null;
    this.pending = { preview: null, full: null };
    this.known.clear();
    this.early.clear();
    this.abandoned.clear();
    this.landed.clear();
    this.ready.clear();
    this.emit();
  }

  /**
   * Settings settled. Older full develops for the after side are superseded.
   * A drag does not write this key; pumping when it already matches is a
   * no-op while that full frame is current.
   */
  update(settings: EditSettings): void {
    if (!this.after) {
      return;
    }
    const key = ReviewController.key(settings);
    if (key !== this.after.key) {
      this.after.settings = ReviewController.clone(settings);
      this.after.key = key;
      this.emit();
    }
    this.pump();
  }

  setZoom(zoom: Zoom): void {
    this.zoom = zoom;
    this.emit();
    this.pump();
  }

  /** True while split or A/B needs the before image. */
  setCompare(active: boolean): void {
    this.compare = active;
    this.emit();
    this.pump();
  }

  /**
   * After-side previews belong to the adjust panel, which picks 720 px while
   * a slider moves and the screen size when it settles. Before-side previews
   * and full develops stay here.
   */
  adjustOwnsAfterPreview(): void {
    this.yieldAfter = true;
  }

  /**
   * Reserve the preview slot for an adjust develop whose id is not known yet,
   * so a before-side preview does not take the slot out from under it.
   * The drag key stays on that job. `after.key` is the settled edit, and a
   * full develop is stale only against that, so a finished drag preview does
   * not develop the intermediate value.
   */
  holdPreview(settings: EditSettings, edge: number): void {
    if (!this.after) {
      return;
    }
    const key = ReviewController.key(settings);
    const previous = this.pending.preview;
    if (previous?.side === "after" && previous.id != null) {
      this.known.delete(previous.id);
      this.abandoned.add(previous.id);
    }
    this.pending.preview = {
      id: null,
      adopting: false,
      side: "after",
      slot: "preview",
      key,
      edge,
      hash: "",
    };
    this.emit();
  }

  /** The adjust command failed before it had an id. Free the slot. */
  releasePreview(): void {
    const job = this.pending.preview;
    if (!job || job.side !== "after" || job.id !== null) {
      return;
    }
    this.pending.preview = null;
    this.pump();
  }

  /** The adjust preview's id arrived. A terminal event may already be buffered. */
  claimPreview(jobId: number): void {
    const job = this.pending.preview;
    if (!job || job.side !== "after" || job.id !== null) {
      return;
    }
    job.id = jobId;
    this.known.set(jobId, job);
    const early = this.early.get(jobId);
    if (!early) {
      return;
    }
    this.early.delete(jobId);
    if (early.type === "done") {
      this.done(early.event);
    } else if (early.type === "cancelled") {
      this.cancelled(early.event);
    } else {
      this.failed(early.event);
    }
  }

  /** The viewer's long edge in device pixels. Rounded so resizes do not churn. */
  setLongEdge(pixels: number): void {
    const edge = Math.max(256, Math.ceil(pixels / 256) * 256);
    if (edge === this.longEdge) {
      return;
    }
    this.longEdge = edge;
    this.pump();
  }

  /** Visible image rectangle at 100% or 200%. Used by region delivery only. */
  view(rect: Rect): void {
    this.regionRect = rect;
    if (this.delivery === "region") {
      void this.fetchRegion();
    }
  }

  done(event: DoneEvent): void {
    if (this.abandoned.delete(event.jobId)) {
      void this.host.takeFrame(event.jobId).catch(() => {});
      return;
    }
    const job = this.known.get(event.jobId);
    if (!job) {
      this.remember(event.jobId, { type: "done", event });
      return;
    }
    if (event.imageId !== this.imageId) {
      return;
    }
    this.known.delete(event.jobId);
    if (job.slot === "preview") {
      // The slot stays taken until the frame is on screen.
      void this.adoptPreview(job);
      return;
    }
    if (this.pending.full === job) {
      this.pending.full = null;
    }
    job.hash = event.settingsHash;
    this.landed.set(job.hash, job);
    const size = this.ready.get(job.hash);
    if (size) {
      void this.adoptFull(job, size);
    }
    this.pump();
  }

  cancelled(event: CancelledEvent): void {
    if (this.abandoned.delete(event.jobId)) {
      return;
    }
    const job = this.known.get(event.jobId);
    if (!job) {
      this.remember(event.jobId, { type: "cancelled", event });
      return;
    }
    this.known.delete(event.jobId);
    if (this.pending[job.slot] === job) {
      this.pending[job.slot] = null;
      this.pump();
    }
  }

  failed(event: FailedEvent): void {
    if (this.abandoned.delete(event.jobId)) {
      return;
    }
    const job = this.known.get(event.jobId);
    if (!job) {
      this.remember(event.jobId, { type: "failed", event });
      return;
    }
    this.known.delete(event.jobId);
    if (this.pending[job.slot] !== job) {
      return;
    }
    this.pending[job.slot] = null;
    this.markFailed(job);
    this.callbacks.onerror(event.error);
    this.pump();
  }

  cacheReady(event: CacheReadyEvent): void {
    if (event.imageId !== this.imageId) {
      return;
    }
    const size = { width: event.width, height: event.height };
    this.ready.set(event.settingsHash, size);
    const job = this.landed.get(event.settingsHash);
    if (job) {
      void this.adoptFull(job, size);
    }
  }

  /**
   * `settingsChanged` follows our own newer develop, whose `cache:ready`
   * comes next, so it is ignored. A region read of the old hash that misses
   * is handled in `fetchRegion`.
   */
  cacheEvicted(event: CacheEvictedEvent): void {
    if (event.imageId !== this.imageId || event.reason === "settingsChanged") {
      return;
    }
    if (event.settingsHash) {
      this.ready.delete(event.settingsHash);
    }
    if (event.reason === "memoryBudget") {
      this.memoryPressure = true;
      this.delivery = "region";
    }
    for (const side of this.sides()) {
      side.full = null;
      side.fullKey = "";
      side.fullHash = "";
    }
    this.emit();
    this.pump();
  }

  /**
   * The backend swapped the quick develop for the full one. Fit previews
   * are developed again; full frames already waited for it.
   */
  refined(event: ImageRefinedEvent): void {
    if (event.imageId !== this.imageId) {
      return;
    }
    for (const side of this.sides()) {
      side.previewKey = "";
    }
    this.pump();
  }

  /** What the viewer should draw now. */
  snapshot(): ReviewView {
    const after = this.after;
    const before = this.before;
    const empty: ReviewView = {
      imageId: this.imageId,
      width: 0,
      height: 0,
      after: null,
      before: null,
      updating: false,
      proxy: false,
      beforeProxy: false,
      delivery: this.delivery,
    };
    if (!after || !before) {
      return empty;
    }
    const { width, height } = this.imageSize();
    const zoomed = this.zoom !== "fit";
    const afterLayer = this.layerFor(after, zoomed, width, height);
    const shared = before.key === after.key;
    const beforeLayer = shared ? afterLayer : this.layerFor(before, zoomed && this.delivery === "texture", width, height);
    return {
      imageId: this.imageId,
      width,
      height,
      after: afterLayer,
      before: beforeLayer,
      updating: zoomed && after.full !== null && after.fullKey !== after.key,
      proxy: afterLayer !== null && !afterLayer.exact,
      beforeProxy: zoomed && beforeLayer !== null && !beforeLayer.exact,
      delivery: this.delivery,
    };
  }

  private layerFor(side: SideState, zoomed: boolean, width: number, height: number): Layer | null {
    const whole = { x: 0, y: 0, width, height };
    if (zoomed && side.full) {
      return side.full;
    }
    if (side.preview) {
      return { ...side.preview, rect: whole };
    }
    return side.full;
  }

  private imageSize(): { width: number; height: number } {
    if (this.fullSize) {
      return this.fullSize;
    }
    const proxy = this.after?.preview?.frame;
    if (this.sizeHint) {
      const { width, height } = this.sizeHint;
      if (proxy && proxy.width >= proxy.height !== width >= height) {
        return { width: height, height: width };
      }
      return this.sizeHint;
    }
    if (proxy) {
      return { width: proxy.width, height: proxy.height };
    }
    return { width: 0, height: 0 };
  }

  private pump(): void {
    const after = this.after;
    const before = this.before;
    if (!this.imageId || !after || !before) {
      return;
    }
    const compareBefore = this.compare && before.key !== after.key;
    const previewFor = (side: SideState) =>
      (side.previewKey !== side.key || side.previewEdge !== this.longEdge) &&
      side.failedPreview !== `${side.key}@${this.longEdge}`;
    // The adjust panel starts after-side previews. Asking for one here would
    // only block the before side, which still has to wait for a free slot.
    const previewTarget: Side | null =
      !this.yieldAfter && previewFor(after)
        ? "after"
        : compareBefore && previewFor(before)
          ? "before"
          : null;
    this.schedule("preview", previewTarget);

    const zoomed = this.zoom !== "fit";
    const fullFor = (side: SideState) =>
      side.fullKey !== side.key && side.failedFull !== side.key && !this.landedFor(side);
    const fullTarget: Side | null =
      zoomed && fullFor(after)
        ? "after"
        : zoomed && compareBefore && this.delivery === "texture" && fullFor(before)
          ? "before"
          : null;
    this.schedule("full", fullTarget);
  }

  private landedFor(side: SideState): boolean {
    for (const job of this.landed.values()) {
      if (job.key === side.key) {
        return true;
      }
    }
    return false;
  }

  private schedule(slot: Slot, target: Side | null): void {
    if (!target) {
      return;
    }
    if (this.yieldAfter && slot === "preview" && target === "after") {
      return;
    }
    const side = this.sideState(target);
    const current = this.pending[slot];
    if (current) {
      const same = current.side === target && current.key === side.key && (slot === "full" || current.edge === this.longEdge);
      if (same) {
        return;
      }
      // The after side may supersede anything. The before side waits.
      if (current.side === "after" && target === "before") {
        return;
      }
    }
    void this.start(slot, target);
  }

  private async start(slot: Slot, target: Side): Promise<void> {
    const side = this.sideState(target);
    const imageId = this.imageId;
    const job: Job = { id: null, adopting: false, side: target, slot, key: side.key, edge: this.longEdge, hash: "" };
    this.pending[slot] = job;
    let started: JobStarted;
    try {
      started =
        slot === "preview"
          ? await this.host.developPreview(imageId, side.settings, job.edge, target === "before")
          : await this.host.developFull(imageId, side.settings);
    } catch (error) {
      if (this.pending[slot] === job) {
        this.pending[slot] = null;
        this.markFailed(job);
      }
      this.callbacks.onerror(error as AppError);
      return;
    }
    if (imageId !== this.imageId) {
      return;
    }
    job.id = started.jobId;
    this.known.set(started.jobId, job);
    this.callbacks.onjob?.(imageId, slot, started.jobId);
    const early = this.early.get(started.jobId);
    if (early) {
      this.early.delete(started.jobId);
      if (early.type === "done") {
        this.done(early.event);
      } else if (early.type === "cancelled") {
        this.cancelled(early.event);
      } else {
        this.failed(early.event);
      }
    }
  }

  /** Terminal events can beat the command's reply. Keep a few. */
  private remember(jobId: number, event: EarlyEvent): void {
    if (this.early.size >= 64) {
      this.early.clear();
    }
    this.early.set(jobId, event);
  }

  private markFailed(job: Job): void {
    const side = this.sideState(job.side);
    if (job.slot === "preview") {
      side.failedPreview = `${job.key}@${job.edge}`;
    } else {
      side.failedFull = job.key;
    }
  }

  private async adoptPreview(job: Job): Promise<void> {
    if (job.id === null) {
      return;
    }
    const imageId = this.imageId;
    try {
      const frame = FrameBuffer.decode(await this.host.takeFrame(job.id));
      if (imageId !== this.imageId) {
        return;
      }
      const side = this.sideState(job.side);
      side.preview = { frame, rect: { x: 0, y: 0, width: frame.width, height: frame.height }, exact: false };
      side.previewKey = job.key;
      side.previewEdge = job.edge;
      this.emit();
    } catch (error) {
      this.markFailed(job);
      this.callbacks.onerror(error as AppError);
    } finally {
      if (this.pending.preview === job) {
        this.pending.preview = null;
      }
    }
    this.pump();
  }

  private async adoptFull(job: Job, size: { width: number; height: number }): Promise<void> {
    if (job.id === null || job.adopting) {
      return;
    }
    job.adopting = true;
    const imageId = this.imageId;
    this.fullSize = size;
    this.delivery = Delivery.choose(size.width, size.height, this.memoryPressure);
    const side = this.sideState(job.side);
    if (this.delivery === "region") {
      if (job.side !== "after") {
        return;
      }
      side.fullKey = job.key;
      side.fullHash = job.hash;
      this.landed.delete(job.hash);
      this.emit();
      await this.fetchRegion();
      return;
    }
    try {
      const frame = FrameBuffer.decode(await this.host.takeFrame(job.id));
      if (imageId !== this.imageId) {
        return;
      }
      side.full = { frame, rect: { x: 0, y: 0, width: frame.width, height: frame.height }, exact: true };
      side.fullKey = job.key;
      side.fullHash = job.hash;
      this.emit();
    } catch (error) {
      this.markFailed(job);
      this.callbacks.onerror(error as AppError);
    } finally {
      this.landed.delete(job.hash);
    }
    this.pump();
  }

  /** One `get_region` at a time; the latest rectangle wins. */
  private async fetchRegion(): Promise<void> {
    const after = this.after;
    if (!after || !after.fullHash || !this.regionRect || !this.fullSize) {
      return;
    }
    if (this.regionBusy) {
      this.regionAgain = true;
      return;
    }
    const imageId = this.imageId;
    const hash = after.fullHash;
    const rect = Viewport.padded(this.regionRect, this.fullSize.width, this.fullSize.height);
    if (rect.width === 0 || rect.height === 0) {
      return;
    }
    this.regionBusy = true;
    try {
      const frame = FrameBuffer.decode(await this.host.getRegion({ imageId, settingsHash: hash, ...rect }));
      if (imageId === this.imageId && this.after === after && after.fullHash === hash) {
        after.full = { frame, rect: { x: rect.x, y: rect.y, width: frame.width, height: frame.height }, exact: true };
        this.emit();
      }
    } catch (error) {
      const appError = error as AppError;
      if (appError.code === "cacheMiss") {
        if (imageId === this.imageId && after.fullHash === hash) {
          // Develop once more. The old crop stays up, marked updating.
          after.fullKey = "";
          after.fullHash = "";
          this.emit();
          this.pump();
        }
      } else {
        this.callbacks.onerror(appError);
      }
    } finally {
      this.regionBusy = false;
    }
    if (this.regionAgain) {
      this.regionAgain = false;
      await this.fetchRegion();
    }
  }

  private sides(): SideState[] {
    return [this.after, this.before].filter((side): side is SideState => side !== null);
  }

  private sideState(side: Side): SideState {
    const state = side === "after" ? this.after : this.before;
    if (!state) {
      throw new Error("The review view has no open image.");
    }
    return state;
  }

  private emit(): void {
    this.callbacks.onview(this.snapshot());
  }

  private static side(settings: EditSettings): SideState {
    return {
      settings: ReviewController.clone(settings),
      key: ReviewController.key(settings),
      preview: null,
      previewKey: "",
      previewEdge: 0,
      full: null,
      fullKey: "",
      fullHash: "",
      failedPreview: "",
      failedFull: "",
    };
  }

  /**
   * What a develop depends on. Geometry is drawn by the viewer over frames
   * in camera orientation, so crop, straighten and turns are left out.
   */
  static developKey(settings: EditSettings): string {
    return JSON.stringify({ ...settings, geometry: null });
  }

  private static key(settings: EditSettings): string {
    return ReviewController.developKey(settings);
  }

  private static clone(settings: EditSettings): EditSettings {
    return JSON.parse(JSON.stringify(settings)) as EditSettings;
  }
}
