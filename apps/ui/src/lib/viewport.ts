import { ProxySize } from "./frame";

/** What one gesture asks the develop worker to do. */
export interface DevelopWork {
  /** Fit-proxy long edge. Null skips the proxy. */
  fit: number | null;
  /** Exact full frame, after the proxy when `fit` is set. */
  full: boolean;
}

export type ViewportGesture = "pan" | "zoom" | "resize";

/**
 * Which gestures develop. Pan, zoom, and resize never do: the window already
 * holds the bitmap and only moves a rectangle of it.
 */
export class DevelopPlan {
  static for(gesture: ViewportGesture | "edit" | "settle" | "open"): DevelopWork {
    switch (gesture) {
      case "pan":
      case "zoom":
      case "resize":
        return { fit: null, full: false };
      case "edit":
        return { fit: ProxySize.drag, full: false };
      case "settle":
      case "open":
        return { fit: ProxySize.settled, full: true };
    }
  }

  /**
   * A/B develops only when that side has no bitmap yet. The before and after
   * frames are the pair the window is allowed to keep.
   */
  static compare(sideReady: boolean): DevelopWork {
    return sideReady ? { fit: null, full: false } : DevelopPlan.for("open");
  }

  static runs(work: DevelopWork): boolean {
    return work.fit !== null || work.full;
  }
}

export type PictureSide = "before" | "after";

interface Slot<T> {
  scene: number;
  edit: number;
  image: T;
}

/**
 * At most two images: the A/B before frame and the current edit.
 * A shared bitmap (no edit yet) counts once.
 */
export class PairCache<T> {
  before: Slot<T> | null = null;
  after: Slot<T> | null = null;

  get(side: PictureSide, scene: number, edit: number): T | null {
    const slot = this.slot(side);
    if (!slot || slot.scene !== scene) return null;
    if (side === "after" && slot.edit !== edit) return null;
    return slot.image;
  }

  /** Last image for this scene, including one the edit has moved past. */
  fallback(side: PictureSide, scene: number): T | null {
    const slot = this.slot(side);
    if (!slot || slot.scene !== scene) return null;
    return slot.image;
  }

  put(side: PictureSide, scene: number, edit: number, image: T, release: (image: T) => void): void {
    const previous = this.slot(side);
    this.set(side, { scene, edit, image });
    this.releaseIfOrphan(previous, release);
  }

  /** The unedited after frame is also the before frame. One bitmap, two slots. */
  shareBeforeFromAfter(release: (image: T) => void): void {
    if (!this.after) return;
    const previous = this.before;
    this.before = {
      scene: this.after.scene,
      edit: this.after.edit,
      image: this.after.image,
    };
    this.releaseIfOrphan(previous, release);
  }

  invalidateAfter(release: (image: T) => void): void {
    if (!this.after) return;
    const previous = this.after;
    this.after = null;
    this.releaseIfOrphan(previous, release);
  }

  invalidateBoth(release: (image: T) => void): void {
    const held = new Set<T>();
    if (this.before) held.add(this.before.image);
    if (this.after) held.add(this.after.image);
    this.before = null;
    this.after = null;
    for (const image of held) release(image);
  }

  private releaseIfOrphan(previous: Slot<T> | null, release: (image: T) => void): void {
    if (!previous) return;
    if (this.before?.image === previous.image || this.after?.image === previous.image) return;
    release(previous.image);
  }

  private slot(side: PictureSide): Slot<T> | null {
    return side === "before" ? this.before : this.after;
  }

  private set(side: PictureSide, slot: Slot<T>): void {
    if (side === "before") this.before = slot;
    else this.after = slot;
  }
}

export interface PickedPicture<T> {
  image: T;
  /** True when `image` is the exact full frame, false when it is the fit proxy. */
  full: boolean;
}

/**
 * Fit proxies stay small. Full frames are the pair {@link PairCache} caps:
 * dropping the edited side frees that bitmap before the next full render.
 */
export class PictureBook<T> {
  readonly fit = new PairCache<T>();
  readonly full = new PairCache<T>();

  putFit(side: PictureSide, scene: number, edit: number, image: T, release: (image: T) => void): void {
    this.fit.put(side, scene, edit, image, release);
    if (side === "after" && edit === 0) this.fit.shareBeforeFromAfter(release);
  }

  putFull(side: PictureSide, scene: number, edit: number, image: T, release: (image: T) => void): void {
    this.full.put(side, scene, edit, image, release);
    if (side === "after" && edit === 0) this.full.shareBeforeFromAfter(release);
  }

  /** Drop the edited full frame. The fit proxy stays up so the view does not go blank. */
  invalidateEdit(release: (image: T) => void): void {
    this.full.invalidateAfter(release);
  }

  invalidateScene(release: (image: T) => void): void {
    this.fit.invalidateBoth(release);
    this.full.invalidateBoth(release);
  }

  canShow(side: PictureSide, scene: number, edit: number): boolean {
    return this.full.get(side, scene, edit) !== null || this.fit.get(side, scene, edit) !== null;
  }

  pick(mode: "fit" | "pixel", side: PictureSide, scene: number, edit: number): PickedPicture<T> | null {
    const full = this.full.get(side, scene, edit);
    const fit = this.fit.get(side, scene, edit);
    if (mode === "pixel" && full) return { image: full, full: true };
    if (fit) return { image: fit, full: false };
    if (full) return { image: full, full: true };
    const staleFit = this.fit.fallback(side, scene);
    if (staleFit) return { image: staleFit, full: false };
    return null;
  }
}
