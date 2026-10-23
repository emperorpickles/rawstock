/**
 * Tick-scale slider math. The component renders this; tests cover the
 * value, step, detent, and reset behaviour without a DOM.
 */

export interface SliderScale {
  min: number;
  max: number;
  /** Alt+arrow, and the drag quantize. */
  fine: number;
  /** Arrow key step. */
  step: number;
  /** Shift+arrow step. */
  coarse: number;
  defaultValue: number;
  /** Drag snaps here. Bipolar controls use 0. */
  detent?: number;
  /** How close a drag must be, in value units, before it snaps. */
  detentWindow?: number;
  majorEvery: number;
  minorEvery: number;
  digits: number;
  /** Prefix `+` on positive numbers. */
  signed?: boolean;
  /** Readout suffix, such as `EV` or `×`. Empty shows the number alone. */
  unit: string;
  /** Spoken unit when the readout suffix is a symbol. */
  ariaUnit?: string;
}

export interface SliderMods {
  shift?: boolean;
  alt?: boolean;
}

/** One drag or one held-arrow gesture. `end` is the single undo value. */
export class SliderGesture {
  private readonly start: number;
  private value: number;

  constructor(start: number) {
    this.start = start;
    this.value = start;
  }

  latest(): number {
    return this.value;
  }

  preview(next: number): number {
    this.value = next;
    return next;
  }

  /** Null when the pointer or key never left the start value. */
  end(): number | null {
    return Object.is(this.value, this.start) ? null : this.value;
  }
}

export class SliderMath {
  static quantize(value: number, step: number): number {
    if (!Number.isFinite(value) || !(step > 0)) {
      return value;
    }
    const snapped = Math.round(value / step) * step;
    return Number(snapped.toFixed(SliderMath.decimals(step)));
  }

  /** Suggested range, widened so an out-of-range file value still sits on the track. */
  static bounds(scale: SliderScale, value: number): { min: number; max: number } {
    const shown = Number.isFinite(value) ? value : scale.defaultValue;
    return {
      min: Math.min(scale.min, shown),
      max: Math.max(scale.max, shown),
    };
  }

  static ticks(min: number, max: number, every: number): number[] {
    if (!(every > 0) || !(max > min)) {
      return [];
    }
    const start = Math.ceil((min - every * 1e-6) / every) * every;
    const ticks: number[] = [];
    for (let value = start; value <= max + every * 1e-6 && ticks.length < 48; value += every) {
      const tick = SliderMath.quantize(value, every);
      if (tick + every * 1e-6 >= min && tick - every * 1e-6 <= max) {
        ticks.push(tick);
      }
    }
    return ticks;
  }

  /**
   * Arrow keys. Alt is fine, shift is coarse, and a step that would pass
   * the detent lands on it.
   */
  static nudge(value: number, scale: SliderScale, direction: -1 | 1, mods: SliderMods = {}): number {
    const delta = mods.alt ? scale.fine : mods.shift ? scale.coarse : scale.step;
    let next = SliderMath.quantize(value + direction * delta, scale.fine);
    if (scale.detent != null && ((value < scale.detent && next > scale.detent) || (value > scale.detent && next < scale.detent))) {
      next = scale.detent;
    }
    const bounds = SliderMath.bounds(scale, value);
    return Math.min(bounds.max, Math.max(bounds.min, next));
  }

  static fromFraction(fraction: number, scale: SliderScale, value: number): number {
    const bounds = SliderMath.bounds(scale, value);
    const span = bounds.max - bounds.min;
    const t = Math.min(1, Math.max(0, fraction));
    let next = span === 0 ? bounds.min : bounds.min + t * span;
    next = SliderMath.quantize(next, scale.fine);
    if (
      scale.detent != null &&
      scale.detentWindow != null &&
      Math.abs(next - scale.detent) <= scale.detentWindow &&
      scale.detent >= bounds.min &&
      scale.detent <= bounds.max
    ) {
      next = scale.detent;
    }
    return Math.min(bounds.max, Math.max(bounds.min, next));
  }

  static fromPointer(
    clientX: number,
    rect: { left: number; width: number },
    scale: SliderScale,
    value: number,
  ): number {
    const fraction = rect.width <= 0 ? 0 : (clientX - rect.left) / rect.width;
    return SliderMath.fromFraction(fraction, scale, value);
  }

  /** Fill from the detent (or the left edge) to the thumb, as 0..1 fractions. */
  static fill(value: number, scale: SliderScale): { start: number; end: number } {
    const bounds = SliderMath.bounds(scale, value);
    const span = bounds.max - bounds.min;
    const at = span === 0 ? 0 : (value - bounds.min) / span;
    if (scale.detent == null) {
      return { start: 0, end: at };
    }
    const origin = span === 0 ? 0 : (scale.detent - bounds.min) / span;
    return { start: Math.min(origin, at), end: Math.max(origin, at) };
  }

  static reset(scale: SliderScale): number {
    return scale.defaultValue;
  }

  static formatNumber(value: number, scale: SliderScale): string {
    const body = (Number.isFinite(value) ? value : scale.defaultValue).toFixed(scale.digits);
    return scale.signed && value > 0 ? `+${body}` : body;
  }

  static format(value: number, scale: SliderScale): string {
    const number = SliderMath.formatNumber(value, scale);
    return scale.unit ? `${number} ${scale.unit}` : number;
  }

  static ariaText(value: number, scale: SliderScale): string {
    const number = SliderMath.formatNumber(value, scale);
    const unit = scale.ariaUnit ?? scale.unit;
    return unit ? `${number} ${unit}` : number;
  }

  /** Pull a finite number out of a typed readout. Units and a leading `+` are ignored. */
  static parse(text: string): number | null {
    const match = text.trim().match(/^[+-]?\d*\.?\d+(?:e[+-]?\d+)?/i);
    if (!match || match[0] === "" || match[0] === "+" || match[0] === "-" || match[0] === ".") {
      return null;
    }
    const value = Number(match[0]);
    return Number.isFinite(value) ? value : null;
  }

  private static decimals(step: number): number {
    const text = step.toExponential();
    const exp = Number(text.slice(text.indexOf("e") + 1));
    return Math.max(0, -exp);
  }
}
