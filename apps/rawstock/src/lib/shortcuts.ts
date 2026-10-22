export type ShortcutPlatform = "mac" | "windows" | "other";

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
    if (!(target instanceof HTMLElement)) {
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
   * `\` , `J`, and the arrow keys have no modifier. `Ctrl+Y` is redo on Windows.
   * Returns null while a text field is focused so those keys type instead.
   */
  static actionFor(event: KeyboardEvent): string | null {
    if (event.altKey || event.repeat || Shortcuts.isTextEntry(event.target)) {
      return null;
    }
    const key = event.key.toLowerCase();
    const platform = Shortcuts.platform();
    const mod = platform === "mac" ? event.metaKey : event.ctrlKey;
    const otherMod = platform === "mac" ? event.ctrlKey : event.metaKey;
    if (otherMod) {
      return null;
    }
    if (mod && event.shiftKey && key === "o") {
      return "open-folder";
    }
    if (mod && event.shiftKey && key === "z") {
      return "redo";
    }
    if (mod && !event.shiftKey && key === "o") {
      return "open";
    }
    if (mod && !event.shiftKey && key === "e") {
      return "export";
    }
    if (mod && !event.shiftKey && key === "z") {
      return "undo";
    }
    if (mod && !event.shiftKey && key === "0") {
      return "fit";
    }
    if (mod && !event.shiftKey && key === "1") {
      return "zoom-100";
    }
    if (mod && !event.shiftKey && key === "2") {
      return "zoom-200";
    }
    if (platform === "windows" && event.ctrlKey && !event.shiftKey && key === "y") {
      return "redo";
    }
    if (mod) {
      return null;
    }
    if (event.key === "\\") {
      return "before-after";
    }
    if (key === "j") {
      return "clipping";
    }
    if (event.key === "ArrowLeft") {
      return "filmstrip-prev";
    }
    if (event.key === "ArrowRight") {
      return "filmstrip-next";
    }
    return null;
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
}

export function exposureText(value: number): string {
  const sign = value > 0 ? "+" : "";
  return `${sign}${value.toFixed(2)} EV`;
}

export function quantityText(value: number, unit: string): string {
  return `${value.toFixed(2)} ${unit}`;
}
