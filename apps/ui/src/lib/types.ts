/** Wire types shared with `apps/desktop`. Names match the camelCase serde boundary. */

export type ModelKind = "parametric" | "spectral";

export interface StockSummary {
  path: string;
  name: string;
  model: ModelKind;
  revision: number;
}

export interface StockState {
  path: string;
  name: string;
  model: ModelKind;
  revision: number;
  source: string;
  license: string;
  exposureEv: number;
  printExposure: number;
  dyeScales: [number, number, number] | null;
  grainAmount: number;
  grainAvailable: boolean;
  halationEnabled: boolean;
  halationThreshold: number;
  halationRadius: number;
  halationWeight: [number, number, number];
}

export interface SessionState {
  path: string;
  fileName: string;
  sensorWidth: number;
  sensorHeight: number;
  displayWidth: number;
  displayHeight: number;
  orientation: string;
  stock: StockState | null;
}

export type ControlEdit =
  | { control: "exposure"; value: number }
  | { control: "printExposure"; value: number }
  | { control: "dyeScales"; value: [number, number, number] }
  | { control: "grainAmount"; value: number }
  | { control: "halationEnabled"; value: boolean }
  | { control: "halationThreshold"; value: number }
  | { control: "halationRadius"; value: number }
  | { control: "halationRed"; value: number };

export type RenderView =
  | { kind: "fit"; longEdge: number }
  | {
      kind: "hundred";
      x: number;
      y: number;
      width: number;
      height: number;
      exact: boolean;
    };

export interface RenderRequest {
  ticket: number;
  compare: boolean;
  edit: ControlEdit | null;
  view: RenderView;
}

export interface SaveDraft {
  path: string;
  name: string;
  source: string;
  license: string;
  revision: number;
}

export interface Progress {
  stage: string;
  doneRows: number;
  rows: number;
}

export interface Frame {
  ticket: number;
  width: number;
  height: number;
  fidelity: "preview" | "exact";
  bytes: Uint8Array;
}

export interface Host {
  listStocks(): Promise<StockSummary[]>;
  session(): Promise<SessionState | null>;
  openRaw(path: string): Promise<SessionState>;
  loadStock(path: string): Promise<SessionState>;
  render(request: RenderRequest): Promise<Frame>;
  exportJpeg(path: string): Promise<void>;
  saveStock(draft: SaveDraft): Promise<StockState>;
  onProgress(handler: (progress: Progress) => void): Promise<() => void>;
  onSession(handler: () => void): Promise<() => void>;
  onSessionError(handler: (message: string) => void): Promise<() => void>;
}
