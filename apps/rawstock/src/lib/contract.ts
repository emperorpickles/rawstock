/** Wire types for the rawstock 1.0 command/event contract. camelCase JSON. */

export type SupportLevel = "calibrated" | "decodes" | "unsupported";

export type SidecarStatus = "none" | "loaded" | "invalid" | "stockVersionMismatch";

export type StockCategory = "colorNegative" | "slide" | "bw" | "fujiSim";

export type JobKind = "preview" | "full" | "thumbnails" | "export";

export type CancelReason = "superseded" | "explicit" | "imageClosed";

export type CacheReason = "imageSwitched" | "settingsChanged" | "memoryBudget";

export type ErrorCode =
  | "unsupportedCamera"
  | "corruptFile"
  | "unsupportedFormat"
  | "io"
  | "sidecarInvalid"
  | "stockMissing"
  | "cacheMiss"
  | "internal";

export interface AppError {
  code: ErrorCode;
  message: string;
  path?: string;
}

export interface CameraInfo {
  make: string;
  model: string;
}

export interface ExifInfo {
  iso?: number;
  shutter?: string;
  aperture?: string;
  focalLength?: string;
  lens?: string;
  capturedAt?: string;
  orientation?: string;
}

export interface StockRef {
  id: string;
  revision: number;
}

/** `custom` is recorded for a later backend. This build develops as-shot only. */
export type WhiteBalance =
  | { mode: "asShot" }
  | { mode: "custom"; temperatureK: number; tint: number };

export interface DyeEdit {
  scales: [number, number, number];
}

export interface GrainEdit {
  amount: number;
  size: number;
}

export interface HalationEdit {
  enabled: boolean;
  threshold?: number;
  radius?: number;
  weight?: [number, number, number];
}

export interface CropRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface GeometryEdit {
  quarterTurns: 0 | 1 | 2 | 3;
  straightenDeg: number;
  crop?: CropRect;
}

/** Lens corrections applied while the RAW file is developed. */
export interface LensEdit {
  /** Red/blue registration plus the blue/purple defringe. On by default. */
  caCorrection: boolean;
}

export interface EditSettings {
  schema: 1;
  stock: StockRef;
  exposureEv: number;
  whiteBalance: WhiteBalance;
  printExposure: number;
  dye?: DyeEdit;
  contrast?: number;
  grain: GrainEdit;
  halation: HalationEdit;
  scan: { look: string };
  geometry: GeometryEdit;
  lens: LensEdit;
}

export interface SidecarInfo {
  status: SidecarStatus;
  settings?: EditSettings;
  savedRevision?: number;
  installedRevision?: number;
}

export interface ImageInfo {
  imageId: string;
  path: string;
  fileName: string;
  camera: CameraInfo;
  support: SupportLevel;
  exif: ExifInfo;
  width: number;
  height: number;
  sidecar: SidecarInfo;
}

export interface RejectedPath {
  path: string;
  error: AppError;
}

export interface OpenOutcome {
  images: ImageInfo[];
  rejected: RejectedPath[];
}

export interface StockSummary {
  id: string;
  name: string;
  category: StockCategory;
  model: string;
  revision: number;
}

export interface JobStarted {
  jobId: number;
}

export interface ExportOptions {
  format: "jpeg";
  quality: number;
  path: string;
}

export interface SaveResult {
  path: string;
  savedAt: string;
}

export interface ProgressEvent {
  jobId: number;
  imageId: string;
  kind: JobKind;
  stage: string;
  done: number;
  total: number;
}

export interface DoneEvent {
  jobId: number;
  imageId: string;
  kind: JobKind;
  settingsHash: string;
  width: number;
  height: number;
  exact: boolean;
  outputPath?: string;
}

export interface CancelledEvent {
  jobId: number;
  imageId: string;
  kind: JobKind;
  reason: CancelReason;
}

export interface FailedEvent {
  jobId: number;
  imageId: string;
  kind: JobKind;
  error: AppError;
}

export interface ThumbnailReadyEvent {
  jobId: number;
  imageId: string;
  stockId: string;
  settingsHash: string;
}

export interface CacheReadyEvent {
  imageId: string;
  settingsHash: string;
  width: number;
  height: number;
}

export interface CacheEvictedEvent {
  imageId: string;
  settingsHash?: string;
  reason: CacheReason;
}

export interface OpenRequested {
  paths: string[];
}

export interface MenuAction {
  action: string;
}

export function defaultSettings(): EditSettings {
  return {
    schema: 1,
    stock: { id: "", revision: 0 },
    exposureEv: 0,
    whiteBalance: { mode: "asShot" },
    printExposure: 0,
    grain: { amount: 1, size: 1 },
    halation: { enabled: false },
    scan: { look: "default" },
    geometry: { quarterTurns: 0, straightenDeg: 0 },
    lens: { caCorrection: true },
  };
}

export function appError(error: unknown): AppError {
  if (isAppError(error)) {
    return error;
  }
  if (typeof error === "string") {
    try {
      const parsed: unknown = JSON.parse(error);
      if (isAppError(parsed)) {
        return parsed;
      }
    } catch {
      return { code: "internal", message: error };
    }
  }
  return {
    code: "internal",
    message: "Something went wrong. The last preview is still on screen.",
  };
}

function isAppError(value: unknown): value is AppError {
  if (!value || typeof value !== "object") {
    return false;
  }
  const record = value as Record<string, unknown>;
  return typeof record.message === "string" && typeof record.code === "string";
}
