import { CropRect, ProxySize } from "./frame";
import { CommandError } from "./ranges";
import type { ControlEdit, Frame, Host, RenderView } from "./types";

export interface LoopView {
  ready: boolean;
  compare: boolean;
  mode: "fit" | "pixel";
  displayWidth: number;
  displayHeight: number;
  viewportWidth: number;
  viewportHeight: number;
  panX: number;
  panY: number;
}

/**
 * Sends preview jobs immediately. The worker applies every edit in order and
 * cancels a render when a newer ticket is already queued, so this class does
 * not wait for the previous bitmap before submitting the next one.
 *
 * A finished frame is shown when it is newer than the one on screen, even if
 * a later ticket is already in flight. The 1440 settle waits for that drag
 * frame to come back, so the upgrade does not cancel the bitmap the pointer
 * is waiting on.
 */
export class PreviewLoop {
  private ticket = 0;
  private shown = 0;
  private inflight: Promise<void> = Promise.resolve();
  private settled: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly host: Host,
    private readonly view: () => LoopView,
    private readonly paint: (frame: Frame) => void,
    private readonly fail: (message: string) => void,
  ) {}

  drag(edit: ControlEdit | null = null): void {
    this.send(edit, false);
    this.scheduleSettle();
  }

  settle(): void {
    this.clear();
    this.send(null, true);
  }

  now(): void {
    this.clear();
    this.send(null, true);
  }

  dispose(): void {
    this.clear();
    this.ticket += 1;
  }

  private scheduleSettle(): void {
    this.clear();
    const generation = this.ticket;
    const pending = this.inflight;
    this.settled = setTimeout(() => {
      void pending.then(() => {
        if (this.ticket === generation) {
          this.send(null, true);
        }
      });
    }, 150);
  }

  private clear(): void {
    if (this.settled !== null) {
      clearTimeout(this.settled);
      this.settled = null;
    }
  }

  private send(edit: ControlEdit | null, settled: boolean): void {
    const current = this.view();
    if (!current.ready) {
      return;
    }
    const ticket = ++this.ticket;
    const view = PreviewLoop.viewFor(current, settled);
    const job = this.host.render({ ticket, compare: current.compare, edit, view }).then(
      (frame) => {
        if (frame.ticket > this.shown) {
          this.shown = frame.ticket;
          this.paint(frame);
        }
      },
      (error: unknown) => {
        if (CommandError.isCancelled(error) || ticket !== this.ticket) {
          return;
        }
        this.fail(CommandError.message(error));
      },
    );
    this.inflight = job.then(
      () => undefined,
      () => undefined,
    );
  }

  private static viewFor(current: LoopView, settled: boolean): RenderView {
    if (current.mode === "fit") {
      return {
        kind: "fit",
        longEdge: settled ? ProxySize.settled : ProxySize.drag,
      };
    }
    const crop = CropRect.visible(
      current.displayWidth,
      current.displayHeight,
      current.viewportWidth,
      current.viewportHeight,
      current.panX,
      current.panY,
    );
    return { kind: "hundred", ...crop, exact: settled };
  }
}
