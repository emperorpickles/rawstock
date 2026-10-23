/**
 * Session edits for one open filmstrip.
 *
 * Undo history lives in memory and disappears on quit. Sidecar writes are
 * debounced; the caller flushes on image switch and on quit. A slider drag
 * previews without a history step and commits once.
 */

import { defaultSettings } from "./contract.ts";
import type { EditSettings, ImageInfo, SidecarInfo, StockSummary } from "./contract.ts";

export function cloneSettings(settings: EditSettings): EditSettings {
  return JSON.parse(JSON.stringify(settings)) as EditSettings;
}

function same(left: EditSettings, right: EditSettings): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function encode(settings: EditSettings): string {
  return JSON.stringify(cloneSettings(settings));
}

interface Session {
  undo: EditSettings[];
  redo: EditSettings[];
  current: EditSettings;
  /** Live slider value. Present only between preview and commit. */
  draft: EditSettings | null;
}

/** Per-image undo stack. A drag is one step because preview does not push. */
export class EditHistory {
  private sessions = new Map<string, Session>();

  has(imageId: string): boolean {
    return this.sessions.has(imageId);
  }

  /** Remember `initial` the first time. Later calls keep the session. */
  open(imageId: string, initial: EditSettings): EditSettings {
    let session = this.sessions.get(imageId);
    if (!session) {
      session = { undo: [], redo: [], current: cloneSettings(initial), draft: null };
      this.sessions.set(imageId, session);
    }
    return cloneSettings(session.draft ?? session.current);
  }

  current(imageId: string): EditSettings | null {
    const session = this.sessions.get(imageId);
    if (!session) {
      return null;
    }
    return cloneSettings(session.draft ?? session.current);
  }

  /** True when this image has an uncommitted drag or an undo step. */
  dirty(imageId: string): boolean {
    const session = this.sessions.get(imageId);
    if (!session) {
      return false;
    }
    if (session.draft && !same(session.draft, session.current)) {
      return true;
    }
    return session.undo.length > 0;
  }

  preview(imageId: string, next: EditSettings): void {
    const session = this.sessions.get(imageId);
    if (!session) {
      return;
    }
    session.draft = cloneSettings(next);
  }

  /** Push one step when `next` differs from the last commit. */
  commit(imageId: string, next: EditSettings): boolean {
    const session = this.sessions.get(imageId);
    if (!session) {
      return false;
    }
    session.draft = null;
    if (same(session.current, next)) {
      return false;
    }
    session.undo.push(cloneSettings(session.current));
    session.current = cloneSettings(next);
    session.redo = [];
    return true;
  }

  /**
   * Undo. An in-progress drag is dropped first, back to the last commit,
   * without consuming a step. The next undo walks the stack.
   */
  undo(imageId: string): EditSettings | null {
    const session = this.sessions.get(imageId);
    if (!session) {
      return null;
    }
    if (session.draft && !same(session.draft, session.current)) {
      session.draft = null;
      return cloneSettings(session.current);
    }
    session.draft = null;
    const previous = session.undo.pop();
    if (!previous) {
      return null;
    }
    session.redo.push(cloneSettings(session.current));
    session.current = previous;
    return cloneSettings(session.current);
  }

  redo(imageId: string): EditSettings | null {
    const session = this.sessions.get(imageId);
    if (!session) {
      return null;
    }
    session.draft = null;
    const next = session.redo.pop();
    if (!next) {
      return null;
    }
    session.undo.push(cloneSettings(session.current));
    session.current = next;
    return cloneSettings(session.current);
  }
}

type Timer = (delayMs: number, run: () => void) => () => void;

interface Pending {
  settings: EditSettings;
  encoded: string;
}

/**
 * Debounced sidecar writes. `flush` runs them immediately (image switch
 * and quit). A blocked image, such as one with an unreadable sidecar, is
 * never written.
 */
export class Autosave {
  private pending = new Map<string, Pending>();
  private saved = new Map<string, string>();
  private blocked = new Set<string>();
  private cancelTimer: (() => void) | null = null;
  private readonly delayMs: number;
  private readonly write: (imageId: string, settings: EditSettings) => Promise<void>;
  private readonly timer: Timer;
  private readonly onError: (error: unknown) => void;

  constructor(
    delayMs: number,
    write: (imageId: string, settings: EditSettings) => Promise<void>,
    timer?: Timer,
    onError?: (error: unknown) => void,
  ) {
    this.delayMs = delayMs;
    this.write = write;
    this.timer =
      timer ??
      ((delay, run) => {
        const id = setTimeout(run, delay);
        return () => clearTimeout(id);
      });
    this.onError = onError ?? (() => {});
  }

  /** The settings already on disk, so an unchanged edit is not rewritten. */
  remember(imageId: string, settings: EditSettings): void {
    this.saved.set(imageId, encode(settings));
  }

  block(imageId: string, blocked: boolean): void {
    if (blocked) {
      this.blocked.add(imageId);
      this.pending.delete(imageId);
      this.rearm();
      return;
    }
    this.blocked.delete(imageId);
  }

  schedule(imageId: string, settings: EditSettings): void {
    const encoded = encode(settings);
    if (this.blocked.has(imageId) || this.saved.get(imageId) === encoded) {
      this.pending.delete(imageId);
      this.rearm();
      return;
    }
    this.pending.set(imageId, { settings: cloneSettings(settings), encoded });
    this.rearm();
  }

  async flush(): Promise<void> {
    this.cancel();
    for (const imageId of [...this.pending.keys()]) {
      const pending = this.pending.get(imageId);
      if (!pending || this.blocked.has(imageId)) {
        this.pending.delete(imageId);
        continue;
      }
      try {
        await this.write(imageId, pending.settings);
        this.saved.set(imageId, pending.encoded);
        this.pending.delete(imageId);
      } catch (error) {
        if (this.blocked.has(imageId)) {
          this.pending.delete(imageId);
        }
        throw error;
      }
    }
  }

  private rearm(): void {
    this.cancel();
    if (this.pending.size === 0) {
      return;
    }
    this.cancelTimer = this.timer(this.delayMs, () => {
      this.cancelTimer = null;
      void this.flush().catch((error: unknown) => this.onError(error));
    });
  }

  private cancel(): void {
    this.cancelTimer?.();
    this.cancelTimer = null;
  }
}

/** Settings an image opens on. A bad sidecar falls back to stock defaults. */
export function baselineSettings(
  image: ImageInfo,
  starting: { id: string; revision: number } | null,
): EditSettings {
  if (
    image.sidecar.settings &&
    (image.sidecar.status === "loaded" || image.sidecar.status === "stockVersionMismatch")
  ) {
    return cloneSettings(image.sidecar.settings);
  }
  const settings = defaultSettings();
  if (starting) {
    settings.stock = { id: starting.id, revision: starting.revision };
  }
  return settings;
}

export function sidecarFromSave(settings: EditSettings, stocks: StockSummary[]): SidecarInfo {
  const installed = stocks.find((stock) => stock.id === settings.stock.id);
  const mismatch = installed != null && installed.revision !== settings.stock.revision;
  return {
    status: mismatch ? "stockVersionMismatch" : "loaded",
    settings: cloneSettings(settings),
    savedRevision: settings.stock.revision,
    installedRevision: installed?.revision,
  };
}

/**
 * Capture order: EXIF date when both photos have one, otherwise the file
 * name. Dated photos come before undated ones.
 */
export function sortImages(images: ImageInfo[]): ImageInfo[] {
  return [...images].sort((left, right) => {
    const leftDate = left.exif.capturedAt;
    const rightDate = right.exif.capturedAt;
    if (leftDate && rightDate && leftDate !== rightDate) {
      return leftDate < rightDate ? -1 : 1;
    }
    if (leftDate && !rightDate) {
      return -1;
    }
    if (!leftDate && rightDate) {
      return 1;
    }
    const byName = left.fileName.toLowerCase().localeCompare(right.fileName.toLowerCase());
    if (byName !== 0) {
      return byName;
    }
    return left.path < right.path ? -1 : left.path > right.path ? 1 : 0;
  });
}

/** Short labels drawn on a filmstrip entry. */
export function filmstripBadges(image: ImageInfo): string[] {
  const badges: string[] = [];
  if (image.sidecar.status === "loaded" || image.sidecar.status === "stockVersionMismatch") {
    badges.push("Edited");
  }
  if (image.sidecar.status === "invalid") {
    badges.push("Edit unreadable");
  }
  if (image.error?.code === "corruptFile") {
    badges.push("Error");
  } else if (image.support === "unsupported" || image.error?.code === "unsupportedCamera") {
    badges.push("Unsupported");
    if (image.error) {
      badges.push("Error");
    }
  } else if (image.support === "decodes") {
    badges.push("Uncalibrated");
  }
  return badges;
}

export function filmstripLabel(image: ImageInfo): string {
  const parts = [image.fileName, ...filmstripBadges(image)];
  if (image.error?.message) {
    parts.push(image.error.message);
  }
  return parts.join(", ");
}

/** Non-blocking notice when the saved stock revision is not the installed one. */
export function versionWarning(settings: EditSettings, stocks: StockSummary[]): string | null {
  if (!settings.stock.id || stocks.length === 0) {
    return null;
  }
  const installed = stocks.find((stock) => stock.id === settings.stock.id);
  if (!installed || installed.revision === settings.stock.revision) {
    return null;
  }
  return `Edited with ${installed.name} rev ${settings.stock.revision}; installed rev ${installed.revision}; the render may differ.`;
}

export function invalidSidecarNotice(fileName: string): string {
  return `${fileName} has an edit file that could not be read. The photo opened with stock defaults, and that edit file was left unchanged.`;
}
