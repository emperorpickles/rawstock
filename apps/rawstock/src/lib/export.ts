/**
 * JPEG export for the M1 dialog.
 *
 * The filename, overwrite, and RAW/sidecar rules live here so the dialog
 * can show them before a job starts. `apps/rawstock/src-tauri/src/export_dest.rs`
 * is the same check on the command side. Keep the sentences in step.
 */

import type { EditSettings, ExportOptions } from "./contract.ts";
import { cloneSettings } from "./edits.ts";
import type { SliderScale } from "./slider.ts";

interface ExportMemory {
  quality: number;
  lastFolder: string | null;
  [key: string]: unknown;
}

export const JPEG_QUALITY_DEFAULT = 92;

/** What is already at the normalized destination. The dialog passes `missing` and the backend stats the path. */
export type DestKind = "missing" | "file" | "directory";

export type ExportReview =
  | { action: "export"; path: string; message: "" }
  | { action: "confirm"; path: string; message: string }
  | { action: "refuse"; path: string; message: string };

export interface ExportRequest {
  imageId: string;
  settings: EditSettings;
  options: ExportOptions;
}

const STAGE_LABELS: Record<string, string> = {
  decode: "Decoding",
  Exposure: "Exposure",
  Halation: "Halation",
  Density: "Film",
  Grain: "Grain",
  "Dye scan": "Scan",
  Print: "Print",
  Encode: "Encoding",
  encode: "Encoding",
  write: "Writing",
};

export class ExportPlan {
  static readonly scale: SliderScale = {
    min: 1,
    max: 100,
    fine: 1,
    step: 1,
    coarse: 10,
    defaultValue: JPEG_QUALITY_DEFAULT,
    majorEvery: 25,
    minorEvery: 5,
    digits: 0,
    unit: "",
    ariaUnit: "percent",
  };

  /** Integer quality in 1..=100. A non-number becomes the default; other values clamp. */
  static quality(value: number): number {
    if (!Number.isFinite(value)) {
      return JPEG_QUALITY_DEFAULT;
    }
    return Math.min(100, Math.max(1, Math.round(value)));
  }

  /** A saved quality outside 1..=100 is the default, matching the settings file. */
  static savedQuality(value: number): number {
    if (!Number.isFinite(value)) {
      return JPEG_QUALITY_DEFAULT;
    }
    const rounded = Math.round(value);
    if (rounded < 1 || rounded > 100) {
      return JPEG_QUALITY_DEFAULT;
    }
    return rounded;
  }

  static baseName(path: string): string {
    const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    return slash === -1 ? path : path.slice(slash + 1);
  }

  static folder(path: string): string {
    const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    if (slash < 0) {
      return "";
    }
    if (slash === 0) {
      return path.slice(0, 1);
    }
    const head = path.slice(0, slash);
    if (/^[A-Za-z]:$/.test(head)) {
      return path.slice(0, slash + 1);
    }
    return head;
  }

  /** `<stem>.jpg`, keeping the RAW's base name and dropping its extension. */
  static jpegName(imagePath: string): string {
    const base = ExportPlan.baseName(imagePath);
    const dot = base.lastIndexOf(".");
    const stem = dot > 0 ? base.slice(0, dot) : base;
    return `${stem || "photo"}.jpg`;
  }

  /**
   * Last export folder when there is one, otherwise the folder the RAW is in.
   * The file name is always `<stem>.jpg`.
   */
  static defaultPath(imagePath: string, lastFolder: string | null): string {
    const folder = lastFolder?.trim() ? lastFolder.trim() : ExportPlan.folder(imagePath);
    return joinPath(folder, ExportPlan.jpegName(imagePath));
  }

  /**
   * `exists` describes `path` after a `.jpg` extension is added, not the text
   * the user typed. A RAW path and its sidecar are refused before that.
   */
  static review(imagePath: string, destination: string, kind: DestKind): ExportReview {
    const raw = imagePath.trim();
    const trimmed = destination.trim();
    if (!trimmed || !ExportPlan.baseName(trimmed)) {
      return { action: "refuse", path: trimmed, message: "Choose a destination for the export." };
    }
    const sidecar = `${raw}.rawstock.toml`;
    if (samePath(trimmed, raw)) {
      return {
        action: "refuse",
        path: trimmed,
        message: `Choose a different file. Export cannot replace ${ExportPlan.baseName(raw)}, the RAW photo.`,
      };
    }
    if (samePath(trimmed, sidecar)) {
      return {
        action: "refuse",
        path: trimmed,
        message: `Choose a different file. Export cannot replace ${ExportPlan.baseName(sidecar)}, the edit saved beside the photo.`,
      };
    }
    const path = ensureJpeg(trimmed);
    if (samePath(path, raw)) {
      return {
        action: "refuse",
        path,
        message: `Choose a different file. Export cannot replace ${ExportPlan.baseName(raw)}, the RAW photo.`,
      };
    }
    if (samePath(path, sidecar)) {
      return {
        action: "refuse",
        path,
        message: `Choose a different file. Export cannot replace ${ExportPlan.baseName(sidecar)}, the edit saved beside the photo.`,
      };
    }
    if (kind === "directory") {
      return { action: "refuse", path, message: "Choose a file. That destination is a folder." };
    }
    if (kind === "file") {
      return {
        action: "confirm",
        path,
        message: replaceMessage(ExportPlan.baseName(path)),
      };
    }
    return { action: "export", path, message: "" };
  }

  /** The edit goes through whole, crop and rotation included. */
  static request(
    imageId: string,
    settings: EditSettings,
    quality: number,
    path: string,
    overwrite: boolean,
  ): ExportRequest {
    return {
      imageId,
      settings: cloneSettings(settings),
      options: {
        format: "jpeg",
        quality: ExportPlan.quality(quality),
        path,
        overwrite,
      },
    };
  }

  static remembered<T extends { export: ExportMemory }>(settings: T, quality: number, path: string): T {
    const folder = ExportPlan.folder(path).trim();
    return {
      ...settings,
      export: {
        ...settings.export,
        quality: ExportPlan.quality(quality),
        lastFolder: folder || null,
      },
    };
  }

  static isReplacePrompt(message: string): boolean {
    return message.includes("already exists. Replace it?");
  }

  static stageLabel(stage: string): string {
    return STAGE_LABELS[stage] ?? stage;
  }

  static progressText(stage: string, done: number, total: number): string {
    if (stage === "queued" || total <= 0) {
      return "Queued";
    }
    return `${ExportPlan.stageLabel(stage)} ${done + 1}/${total}`;
  }

  /** The job finished. This build's develop service does not write the file. */
  static finishedNotice(path: string): string {
    const name = ExportPlan.baseName(path) || "Export";
    return `${name} export finished. The JPEG is not written in this build.`;
  }

  static cancelledNotice(): string {
    return "Export cancelled. No file was written.";
  }
}

function replaceMessage(fileName: string): string {
  return `${fileName} already exists. Replace it? The RAW photo is not changed.`;
}

function ensureJpeg(path: string): string {
  const base = ExportPlan.baseName(path);
  const dot = base.lastIndexOf(".");
  const ext = dot > 0 ? base.slice(dot + 1) : "";
  if (ext.toLowerCase() === "jpg" || ext.toLowerCase() === "jpeg") {
    return path;
  }
  return `${path}.jpg`;
}

function joinPath(folder: string, name: string): string {
  if (!folder || folder === "/" || folder === "\\") {
    return `${folder === "\\" ? "\\" : "/"}${name}`;
  }
  const sep = folder.includes("\\") && !folder.includes("/") ? "\\" : "/";
  const trimmed = folder.endsWith("/") || folder.endsWith("\\") ? folder.slice(0, -1) : folder;
  if (/^[A-Za-z]:$/.test(trimmed)) {
    return `${trimmed}${sep}${name}`;
  }
  return `${trimmed}${sep}${name}`;
}

/** Case-insensitive, and `..` is resolved. Matches the Rust check. */
function samePath(left: string, right: string): boolean {
  return identity(left) === identity(right);
}

function identity(path: string): string {
  const text = path.trim().replaceAll("\\", "/");
  const absolute = text.startsWith("/");
  const parts: string[] = [];
  for (const part of text.split("/")) {
    if (part === "" || part === ".") {
      continue;
    }
    if (part === "..") {
      parts.pop();
      continue;
    }
    parts.push(part.toLowerCase());
  }
  return `${absolute ? "/" : ""}${parts.join("/")}`;
}
