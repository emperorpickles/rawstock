import type { CropRect, EditSettings, StockSummary } from "./contract.ts";
import { CropMath } from "./geometry.ts";
import type { AspectId } from "./geometry.ts";
import type { SliderScale } from "./slider.ts";

/** Scan looks stored on the edit. The develop service applies `default` today. */
export const SCAN_LOOKS: readonly { id: string; label: string }[] = [
  { id: "default", label: "Stock default" },
  { id: "flat", label: "Flat print" },
  { id: "punchy", label: "Punchy print" },
];

const TURNS = [0, 1, 2, 3] as const;

export class Adjustments {
  static readonly exposureScale: SliderScale = {
    min: -3,
    max: 3,
    fine: 0.01,
    step: 0.1,
    coarse: 1,
    defaultValue: 0,
    detent: 0,
    detentWindow: 0.03,
    majorEvery: 1,
    minorEvery: 0.5,
    digits: 2,
    signed: true,
    unit: "EV",
  };

  static readonly print: SliderScale = {
    min: 0,
    max: 2.5,
    fine: 0.01,
    step: 0.05,
    coarse: 0.25,
    defaultValue: 0,
    majorEvery: 0.5,
    minorEvery: 0.1,
    digits: 2,
    unit: "",
    ariaUnit: "print exposure",
  };

  static readonly dye: SliderScale = {
    min: 0.25,
    max: 2.5,
    fine: 0.01,
    step: 0.05,
    coarse: 0.25,
    defaultValue: 1,
    majorEvery: 0.5,
    minorEvery: 0.25,
    digits: 2,
    unit: "×",
    ariaUnit: "times the stock dye",
  };

  static readonly contrastScale: SliderScale = {
    min: 0.5,
    max: 2,
    fine: 0.01,
    step: 0.05,
    coarse: 0.25,
    defaultValue: 1,
    detent: 1,
    detentWindow: 0.03,
    majorEvery: 0.5,
    minorEvery: 0.1,
    digits: 2,
    unit: "×",
    ariaUnit: "times the stock contrast",
  };

  static readonly grainAmountScale: SliderScale = {
    min: 0,
    max: 2,
    fine: 0.01,
    step: 0.05,
    coarse: 0.25,
    defaultValue: 1,
    majorEvery: 0.5,
    minorEvery: 0.1,
    digits: 2,
    unit: "×",
    ariaUnit: "times the stock grain",
  };

  static readonly grainSizeScale: SliderScale = {
    min: 0.25,
    max: 2,
    fine: 0.01,
    step: 0.05,
    coarse: 0.25,
    defaultValue: 1,
    majorEvery: 0.5,
    minorEvery: 0.1,
    digits: 2,
    unit: "×",
    ariaUnit: "times the stock grain size",
  };

  static readonly threshold: SliderScale = {
    min: 0,
    max: 2,
    fine: 0.001,
    step: 0.01,
    coarse: 0.1,
    defaultValue: 0.25,
    majorEvery: 0.5,
    minorEvery: 0.1,
    digits: 3,
    unit: "",
    ariaUnit: "halation threshold",
  };

  static readonly radius: SliderScale = {
    min: 0,
    max: 0.05,
    fine: 0.0001,
    step: 0.001,
    coarse: 0.01,
    defaultValue: 0.008,
    majorEvery: 0.01,
    minorEvery: 0.005,
    digits: 4,
    unit: "",
    ariaUnit: "halation radius",
  };

  static readonly straightenScale: SliderScale = {
    min: -45,
    max: 45,
    fine: 0.01,
    step: 0.1,
    coarse: 1,
    defaultValue: 0,
    detent: 0,
    detentWindow: 0.08,
    majorEvery: 15,
    minorEvery: 5,
    digits: 2,
    signed: true,
    unit: "°",
    ariaUnit: "degrees",
  };

  static exposure(settings: EditSettings, exposureEv: number): EditSettings {
    return { ...settings, exposureEv };
  }

  static printExposure(settings: EditSettings, printExposure: number): EditSettings {
    return { ...settings, printExposure };
  }

  static dyeChannel(settings: EditSettings, index: 0 | 1 | 2, value: number): EditSettings {
    const current = settings.dye?.scales ?? [1, 1, 1];
    const scales: [number, number, number] = [current[0], current[1], current[2]];
    scales[index] = value;
    const dye = scales.every((scale) => scale === 1) ? undefined : { scales };
    return { ...settings, dye };
  }

  static contrast(settings: EditSettings, value: number): EditSettings {
    return { ...settings, contrast: value === Adjustments.contrastScale.defaultValue ? undefined : value };
  }

  static grainAmount(settings: EditSettings, amount: number): EditSettings {
    return { ...settings, grain: { ...settings.grain, amount } };
  }

  static grainSize(settings: EditSettings, size: number): EditSettings {
    if (!(size > 0)) {
      return settings;
    }
    return { ...settings, grain: { ...settings.grain, size } };
  }

  static halationEnabled(settings: EditSettings, enabled: boolean): EditSettings {
    return { ...settings, halation: { ...settings.halation, enabled } };
  }

  static halationThreshold(settings: EditSettings, value: number, stockDefault: number): EditSettings {
    const threshold = Math.abs(value - stockDefault) < 1e-6 ? undefined : value;
    return { ...settings, halation: { ...settings.halation, threshold } };
  }

  static halationRadius(settings: EditSettings, value: number, stockDefault: number): EditSettings {
    const radius = Math.abs(value - stockDefault) < 1e-6 ? undefined : value;
    return { ...settings, halation: { ...settings.halation, radius } };
  }

  static scan(settings: EditSettings, look: string): EditSettings {
    return { ...settings, scan: { ...settings.scan, look } };
  }

  static rotate(settings: EditSettings, direction: 1 | -1): EditSettings {
    const quarterTurns = TURNS[(settings.geometry.quarterTurns + direction + 4) % 4];
    const crop = settings.geometry.crop
      ? direction > 0
        ? CropMath.clockwise(settings.geometry.crop)
        : CropMath.counterClockwise(settings.geometry.crop)
      : undefined;
    return { ...settings, geometry: { ...settings.geometry, quarterTurns, crop } };
  }

  static straighten(settings: EditSettings, straightenDeg: number): EditSettings {
    return { ...settings, geometry: { ...settings.geometry, straightenDeg } };
  }

  static crop(settings: EditSettings, crop: CropRect | undefined): EditSettings {
    const next = crop && !CropMath.isFull(crop) ? crop : undefined;
    return { ...settings, geometry: { ...settings.geometry, crop: next } };
  }

  static aspect(
    settings: EditSettings,
    id: AspectId,
    imageWidth: number,
    imageHeight: number,
  ): EditSettings {
    if (id === "original") {
      return Adjustments.crop(settings, undefined);
    }
    if (id === "free") {
      return Adjustments.crop(settings, CropMath.freeStart());
    }
    const ratio = CropMath.pixelRatio(id, imageWidth, imageHeight);
    if (ratio == null) {
      return settings;
    }
    return Adjustments.crop(settings, CropMath.maximum(ratio, imageWidth, imageHeight));
  }

  static showDye(stock: StockSummary | null, settings: EditSettings): boolean {
    return Boolean(stock?.controls?.dye) || settings.dye != null;
  }

  static showContrast(stock: StockSummary | null, settings: EditSettings): boolean {
    return Boolean(stock?.controls?.contrast) || settings.contrast != null;
  }

  static showGrain(stock: StockSummary | null, settings: EditSettings): boolean {
    if (stock?.controls) {
      return stock.controls.grain || settings.grain.amount !== 1 || settings.grain.size !== 1;
    }
    return true;
  }

  static scanOptions(look: string): { id: string; label: string }[] {
    const known = SCAN_LOOKS.some((item) => item.id === look);
    if (known) {
      return [...SCAN_LOOKS];
    }
    return [...SCAN_LOOKS, { id: look, label: `Saved look (${look})` }];
  }

  static thresholdDefault(stock: StockSummary | null): number {
    return stock?.controls?.halationThreshold ?? Adjustments.threshold.defaultValue;
  }

  static radiusDefault(stock: StockSummary | null): number {
    return stock?.controls?.halationRadius ?? Adjustments.radius.defaultValue;
  }

  static orientedFrame(
    frameWidth: number,
    frameHeight: number,
    quarterTurns: number,
  ): { width: number; height: number } {
    const width = frameWidth > 0 ? frameWidth : 3;
    const height = frameHeight > 0 ? frameHeight : 2;
    const turns = ((quarterTurns % 4) + 4) % 4;
    if (turns % 2 === 1) {
      return { width: height, height: width };
    }
    return { width, height };
  }
}
