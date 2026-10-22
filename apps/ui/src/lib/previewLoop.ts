import { CommandError } from "./ranges";
import type { ControlEdit, Frame, Host } from "./types";
import { DevelopPlan, type ViewportGesture } from "./viewport";

export interface LoopView {
  ready: boolean;
  compare: boolean;
  /** Bumps when the file, stock, rotation, or a committed save changes. */
  scene: number;
  /** Bumps on each slider edit. The A/B before frame ignores it. */
  edit: number;
}

/** A finished bitmap plus the settings it belongs to. */
export interface PaintedFrame {
  frame: Frame;
  kind: "fit" | "full";
  scene: number;
  edit: number;
  compare: boolean;
}

/**
 * Fit proxies stay on the fast path: 720 while a slider moves, 1440 once it
 * rests, then one exact full frame. Pan, zoom, and resize call
 * {@link PreviewLoop.viewport} and do not reach the worker. A newer ticket
 * cancels the render already on the thread; a reply whose scene or edit has
 * moved on is dropped here as well.
 */
export class PreviewLoop {
  private ticket = 0;
  private shown = 0;
  private inflight: Promise<void> = Promise.resolve();
  private settled: ReturnType<typeof setTimeout> | null = null;
  /** Picture id whose full frame was already requested. */
  private cover: string | null = null;

  constructor(
    private readonly host: Host,
    private readonly view: () => LoopView,
    private readonly paint: (frame: PaintedFrame) => void,
    private readonly fail: (message: string) => void,
    private readonly developed: () => void = () => {},
  ) {}

  /** Slider is moving. Fit proxy only; the full frame waits until it rests. */
  drag(edit: ControlEdit | null = null): void {
    this.cover = null;
    this.follow(DevelopPlan.for("edit"), edit);
    this.scheduleSettle();
  }

  /** Open, stock, rotation, save, or an A/B side that is not cached yet. */
  now(): void {
    this.clear();
    const current = this.view();
    if (!current.ready) return;
    const id = PreviewLoop.pictureId(current);
    if (this.cover === id) return;
    this.cover = id;
    this.follow(DevelopPlan.for("open"), null);
  }

  /**
   * Pan, zoom, or a viewport resize. This does not develop. The plan is what
   * makes that true; a gesture that starts returning work would be sent.
   */
  viewport(gesture: ViewportGesture): void {
    const work = DevelopPlan.for(gesture);
    if (!DevelopPlan.runs(work)) return;
    this.cover = null;
    this.follow(work, null);
  }

  dispose(): void {
    this.clear();
    this.ticket += 1;
    this.cover = null;
  }

  private settle(): void {
    this.clear();
    const current = this.view();
    if (!current.ready) return;
    this.cover = PreviewLoop.pictureId(current);
    this.follow(DevelopPlan.for("settle"), null);
  }

  private scheduleSettle(): void {
    this.clear();
    const generation = this.ticket;
    const pending = this.inflight;
    this.settled = setTimeout(() => {
      void pending.then(() => {
        if (this.ticket === generation) this.settle();
      });
    }, 150);
  }

  private clear(): void {
    if (this.settled !== null) {
      clearTimeout(this.settled);
      this.settled = null;
    }
  }

  private follow(work: { fit: number | null; full: boolean }, edit: ControlEdit | null): void {
    if (!this.view().ready) return;
    if (work.fit === null) {
      if (work.full) {
        const current = this.view();
        this.sendFull(current.scene, current.edit, current.compare);
      }
      return;
    }
    this.sendFit(edit, work.fit, work.full);
  }

  private sendFit(edit: ControlEdit | null, longEdge: number, thenFull: boolean): void {
    const current = this.view();
    if (!current.ready) return;
    const ticket = ++this.ticket;
    const scene = current.scene;
    const editGen = current.edit;
    const compare = current.compare;
    this.developed();
    const job = this.host
      .render({
        ticket,
        compare,
        edit,
        view: { kind: "fit", longEdge },
      })
      .then(
        (frame) => {
          if (this.live(scene, editGen, compare) && frame.ticket > this.shown) {
            this.shown = frame.ticket;
            this.paint({ frame, kind: "fit", scene, edit: editGen, compare });
          }
          if (thenFull && ticket === this.ticket && this.live(scene, editGen, compare)) {
            this.sendFull(scene, editGen, compare);
          }
        },
        (error: unknown) => this.onError(error, ticket),
      );
    this.inflight = job.then(
      () => undefined,
      () => undefined,
    );
  }

  private sendFull(scene: number, edit: number, compare: boolean): void {
    const ticket = ++this.ticket;
    this.developed();
    const job = this.host
      .render({
        ticket,
        compare,
        edit: null,
        view: { kind: "full" },
      })
      .then(
        (frame) => {
          if (!this.live(scene, edit, compare)) return;
          if (frame.ticket > this.shown) this.shown = frame.ticket;
          this.paint({ frame, kind: "full", scene, edit, compare });
        },
        (error: unknown) => this.onError(error, ticket),
      );
    this.inflight = job.then(
      () => undefined,
      () => undefined,
    );
  }

  /** Before frames stay valid across slider edits. After frames do not. */
  private live(scene: number, edit: number, compare: boolean): boolean {
    const current = this.view();
    if (current.scene !== scene) return false;
    if (!compare && current.edit !== edit) return false;
    return true;
  }

  private onError(error: unknown, ticket: number): void {
    if (CommandError.isCancelled(error) || ticket !== this.ticket) return;
    this.cover = null;
    this.fail(CommandError.message(error));
  }

  private static pictureId(current: LoopView): string {
    const edit = current.compare ? 0 : current.edit;
    return `${current.scene}:${current.compare ? 1 : 0}:${edit}`;
  }
}
