/**
 * Stock browser state: which stocks show, how they group, when their
 * thumbnails need drawing again, and which thumbnail events still count.
 *
 * Thumbnails show tone and colour. Grain and halation are left out by the
 * renderer, so a grain or halation change does not redraw them.
 */

import type { EditSettings, StockCategory, StockSummary, ThumbnailReadyEvent } from "./contract.ts";

export interface StockGroup {
  category: StockCategory;
  label: string;
  stocks: StockSummary[];
}

export class StockBrowser {
  /** Reference (the Neutral stock) leads, so every film can be read against it. */
  static readonly order: StockCategory[] = ["reference", "colorNegative", "slide", "bw", "fujiSim"];

  static readonly labels: Record<StockCategory, string> = {
    reference: "Reference",
    colorNegative: "Color negative",
    slide: "Slide",
    bw: "Black and white",
    fujiSim: "Fuji simulation",
  };

  /** Grid columns in the side panel. */
  static readonly columns = 2;

  /** Categories in a fixed order, empty ones dropped, names sorted. */
  static groups(stocks: StockSummary[]): StockGroup[] {
    return StockBrowser.order
      .map((category) => ({
        category,
        label: StockBrowser.labels[category],
        stocks: stocks
          .filter((stock) => stock.category === category)
          .sort((left, right) => left.name.localeCompare(right.name)),
      }))
      .filter((group) => group.stocks.length > 0);
  }

  /** Stocks in the order the grid shows them, for keyboard movement. */
  static flatten(groups: StockGroup[]): StockSummary[] {
    return groups.flatMap((group) => group.stocks);
  }

  static label(stock: StockSummary): string {
    return `${stock.name}, ${StockBrowser.labels[stock.category]}`;
  }

  static detail(stock: StockSummary): string {
    return `${stock.name} · ${stock.model} · revision ${stock.revision}`;
  }

  /**
   * The settings a thumbnail depends on. The stock, grain, halation, print,
   * dye and scan choices are left out: either the thumbnail shows every
   * stock as it ships, or the renderer does not draw them.
   */
  static thumbnailKey(settings: EditSettings): string {
    return JSON.stringify({
      exposureEv: settings.exposureEv,
      whiteBalance: settings.whiteBalance,
      geometry: settings.geometry,
      lens: settings.lens,
    });
  }

  /**
   * Arrow keys move through the grid group by group. Within a group,
   * Left/Right step and Up/Down move a row. Home/End go to the ends.
   * Returns the new flat index, or `null` for a key the grid ignores.
   */
  static move(groups: StockGroup[], index: number, key: string): number | null {
    const total = StockBrowser.flatten(groups).length;
    if (total === 0) {
      return null;
    }
    const columns = StockBrowser.columns;
    switch (key) {
      case "ArrowLeft":
        return Math.max(0, index - 1);
      case "ArrowRight":
        return Math.min(total - 1, index + 1);
      case "Home":
        return 0;
      case "End":
        return total - 1;
      case "ArrowUp":
      case "ArrowDown":
        break;
      default:
        return null;
    }
    const cells: Array<{ group: number; row: number; column: number }> = [];
    groups.forEach((group, groupIndex) => {
      group.stocks.forEach((_, position) => {
        cells.push({
          group: groupIndex,
          row: Math.floor(position / columns),
          column: position % columns,
        });
      });
    });
    const here = cells[Math.min(Math.max(index, 0), total - 1)];
    // Rows across all groups, in display order.
    const rows: Array<{ group: number; row: number }> = [];
    for (const cell of cells) {
      const last = rows[rows.length - 1];
      if (!last || last.group !== cell.group || last.row !== cell.row) {
        rows.push({ group: cell.group, row: cell.row });
      }
    }
    const rowIndex = rows.findIndex((row) => row.group === here.group && row.row === here.row);
    const target = rows[rowIndex + (key === "ArrowDown" ? 1 : -1)];
    if (!target) {
      return index;
    }
    let best = index;
    cells.forEach((cell, flat) => {
      if (cell.group === target.group && cell.row === target.row && cell.column <= here.column) {
        best = flat;
      }
    });
    return best;
  }
}

/**
 * The one thumbnail job whose frames may be shown. A request for another
 * image or newer settings replaces it, so a late frame from an earlier
 * job is dropped.
 */
export class ThumbnailRequests {
  private imageId = "";
  private jobId: number | null = null;
  private key = "";

  /** True when `imageId` and `key` need a new job. */
  needs(imageId: string, key: string): boolean {
    return imageId !== "" && (imageId !== this.imageId || key !== this.key);
  }

  /** The image or settings the next job is for. Earlier frames are stale. */
  begin(imageId: string, key: string): void {
    this.imageId = imageId;
    this.key = key;
    this.jobId = null;
  }

  started(imageId: string, key: string, jobId: number): boolean {
    if (imageId !== this.imageId || key !== this.key) {
      return false;
    }
    this.jobId = jobId;
    return true;
  }

  accepts(event: Pick<ThumbnailReadyEvent, "imageId" | "jobId">): boolean {
    return this.jobId !== null && event.imageId === this.imageId && event.jobId === this.jobId;
  }

  isCurrent(jobId: number): boolean {
    return this.jobId === jobId;
  }

  /** Forget everything, as when no photo is open. */
  clear(): void {
    this.begin("", "");
  }
}
