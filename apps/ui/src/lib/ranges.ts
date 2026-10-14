/** Slider extents. A file value outside the suggested range widens the track. */
export class SliderRange {
  readonly min: number;
  readonly max: number;
  readonly step: number;

  private constructor(min: number, max: number, step: number) {
    this.min = min;
    this.max = max;
    this.step = step;
  }

  static including(start: number, end: number, step: number, value: number): SliderRange {
    return new SliderRange(Math.min(start, value), Math.max(end, value), step);
  }
}

/** Logarithmic position for the halation threshold. */
export class LogSlider {
  readonly floor: number;
  readonly ceil: number;

  private constructor(floor: number, ceil: number) {
    this.floor = floor;
    this.ceil = Math.max(ceil, floor * 1.0001);
  }

  static threshold(value: number): LogSlider {
    const finite = Number.isFinite(value) && value > 0 ? value : 0.25;
    return new LogSlider(Math.min(0.01, finite), Math.max(4, finite));
  }

  toInput(value: number): number {
    const clamped = Math.min(this.ceil, Math.max(this.floor, value));
    const t = Math.log(clamped / this.floor) / Math.log(this.ceil / this.floor);
    return t * 1000;
  }

  fromInput(position: number): number {
    const t = Math.min(1, Math.max(0, position / 1000));
    return this.floor * (this.ceil / this.floor) ** t;
  }
}

export class StockFileName {
  static suggest(name: string, revision: number): string {
    const slug = name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "");
    return `${slug || "stock"}-r${revision}.stock.toml`;
  }
}

export class CommandError {
  static message(error: unknown): string {
    if (typeof error === "string") {
      return error;
    }
    if (error instanceof Error) {
      return error.message;
    }
    if (error && typeof error === "object" && "message" in error) {
      const message = error.message;
      if (typeof message === "string") {
        return message;
      }
    }
    return "Something went wrong.";
  }

  static isCancelled(error: unknown): boolean {
    return CommandError.message(error) === "cancelled";
  }
}

export class Readout {
  static fixed(value: number, digits: number): string {
    return value.toFixed(digits);
  }
}
