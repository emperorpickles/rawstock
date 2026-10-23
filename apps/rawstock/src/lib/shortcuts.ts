export type ShortcutPlatform = "mac" | "windows" | "other";

/** Shape of a keydown that `Shortcuts.match` reads. A `KeyboardEvent` fits. */
export interface KeyPress {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  repeat?: boolean;
}

export interface ShortcutSpec {
  action: string;
  /** What the cheat sheet says it does. */
  label: string;
  /** `KeyboardEvent.key`, lower case for letters. */
  key: string;
  /** Command on macOS, Control elsewhere. */
  mod: boolean;
  /** Only for modifier shortcuts. Unmodified keys ignore Shift (`?` needs it). */
  shift?: boolean;
  /** Limits the shortcut to one platform. */
  platform?: ShortcutPlatform;
  /** Menu item that carries the same accelerator, when it differs from `action`. */
  menuId?: string;
}

export interface CheatSheetRow {
  action: string;
  label: string;
  keys: string;
}

/**
 * Every keyboard shortcut. `actionFor` matches against this table and the
 * cheat sheet is generated from it, so the two cannot drift. A unit test
 * checks the modifier rows against the menu accelerators in `menu.rs`.
 */
export const SHORTCUTS: readonly ShortcutSpec[] = [
  { action: "open", label: "Open a RAW file", key: "o", mod: true },
  { action: "open-folder", label: "Open a folder", key: "o", mod: true, shift: true },
  { action: "export", label: "Export", key: "e", mod: true },
  { action: "undo", label: "Undo", key: "z", mod: true },
  { action: "redo", label: "Redo", key: "z", mod: true, shift: true },
  { action: "redo", label: "Redo", key: "y", mod: true, platform: "windows", menuId: "redo-y" },
  { action: "fit", label: "Fit to window", key: "0", mod: true },
  { action: "zoom-100", label: "Zoom to 100%", key: "1", mod: true },
  { action: "zoom-200", label: "Zoom to 200%", key: "2", mod: true },
  { action: "preferences", label: "Preferences", key: ",", mod: true },
  { action: "before-after", label: "Before and after", key: "\\", mod: false },
  { action: "clipping", label: "Clipping overlay", key: "j", mod: false },
  { action: "filmstrip-prev", label: "Previous photo", key: "ArrowLeft", mod: false },
  { action: "filmstrip-next", label: "Next photo", key: "ArrowRight", mod: false },
  { action: "filmstrip-first", label: "First photo", key: "Home", mod: false },
  { action: "filmstrip-last", label: "Last photo", key: "End", mod: false },
  { action: "cheat-sheet", label: "Keyboard shortcuts", key: "?", mod: false },
];

const KEY_NAMES: Record<string, string> = {
  ArrowLeft: "Left Arrow",
  ArrowRight: "Right Arrow",
  Home: "Home",
  End: "End",
  ",": ",",
};

export class Shortcuts {
  static platform(): ShortcutPlatform {
    const value = navigator.platform || "";
    if (value.startsWith("Mac")) {
      return "mac";
    }
    if (value.startsWith("Win")) {
      return "windows";
    }
    return "other";
  }

  static isTextEntry(target: EventTarget | null): boolean {
    if (typeof HTMLElement === "undefined" || !(target instanceof HTMLElement)) {
      return false;
    }
    if (target.isContentEditable) {
      return true;
    }
    const tag = target.tagName;
    return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
  }

  /**
   * Modifier shortcuts use Command on macOS and Control elsewhere.
   * `\` , `J`, `?`, and the arrow keys have no modifier. `Ctrl+Y` is redo on
   * Windows. Returns null while a text field is focused so those keys type
   * instead.
   */
  static actionFor(event: KeyboardEvent): string | null {
    if (Shortcuts.isTextEntry(event.target)) {
      return null;
    }
    return Shortcuts.match(event, Shortcuts.platform());
  }

  /** Table lookup without the focus check. */
  static match(event: KeyPress, platform: ShortcutPlatform): string | null {
    if (event.altKey || event.repeat) {
      return null;
    }
    const mod = platform === "mac" ? event.metaKey : event.ctrlKey;
    const otherMod = platform === "mac" ? event.ctrlKey : event.metaKey;
    if (otherMod) {
      return null;
    }
    const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
    const hit = SHORTCUTS.find(
      (spec) =>
        spec.mod === mod &&
        spec.key === key &&
        (!spec.mod || Boolean(spec.shift) === event.shiftKey) &&
        (!spec.platform || spec.platform === platform),
    );
    return hit?.action ?? null;
  }

  /** True when the webview should emit the action itself. */
  static dispatchLocally(action: string, event: KeyboardEvent): boolean {
    if (!action) {
      return false;
    }
    const key = event.key.toLowerCase();
    if (action === "redo" && key === "y") {
      return true;
    }
    return !event.metaKey && !event.ctrlKey;
  }

  /** How a shortcut is written for `platform`: `Cmd+Shift+O`, `Ctrl+E`, `J`. */
  static keys(spec: ShortcutSpec, platform: ShortcutPlatform): string {
    const parts: string[] = [];
    if (spec.mod) {
      parts.push(platform === "mac" ? "Cmd" : "Ctrl");
    }
    if (spec.mod && spec.shift) {
      parts.push("Shift");
    }
    parts.push(KEY_NAMES[spec.key] ?? spec.key.toUpperCase());
    return parts.join("+");
  }

  /** One row per action, alternate keys joined with "or". */
  static cheatSheet(platform: ShortcutPlatform): CheatSheetRow[] {
    const rows: CheatSheetRow[] = [];
    for (const spec of SHORTCUTS) {
      if (spec.platform && spec.platform !== platform) {
        continue;
      }
      const keys = Shortcuts.keys(spec, platform);
      const existing = rows.find((row) => row.action === spec.action);
      if (existing) {
        existing.keys = `${existing.keys} or ${keys}`;
      } else {
        rows.push({ action: spec.action, label: spec.label, keys });
      }
    }
    return rows;
  }
}

export function exposureText(value: number): string {
  const sign = value > 0 ? "+" : "";
  return `${sign}${value.toFixed(2)} EV`;
}

export function quantityText(value: number, unit: string): string {
  return `${value.toFixed(2)} ${unit}`;
}
