import type { StockState, StockSummary } from "./types";

/** The stocks panel. One file is one row, even when two paths name it. */
export class StockList {
  /** Canonical id from the shell. A missing id falls back to the path with `.` and `..` removed. */
  static id(stock: { id?: string; path: string }): string {
    if (stock.id && stock.id.length > 0) {
      return stock.id;
    }
    return StockList.normalize(stock.path);
  }

  static normalize(path: string): string {
    const absolute = path.startsWith("/");
    const parts: string[] = [];
    for (const part of path.split(/[/\\]+/)) {
      if (part === "" || part === ".") {
        continue;
      }
      if (part === "..") {
        if (parts.length > 0 && parts[parts.length - 1] !== "..") {
          parts.pop();
        } else if (!absolute) {
          parts.push(part);
        }
        continue;
      }
      parts.push(part);
    }
    const joined = parts.join("/");
    return absolute ? `/${joined}` : joined;
  }

  /** Keep the first registration of each stock id. */
  static dedupe(stocks: StockSummary[]): StockSummary[] {
    const seen = new Set<string>();
    const unique: StockSummary[] = [];
    for (const stock of stocks) {
      const id = StockList.id(stock);
      if (seen.has(id)) {
        continue;
      }
      seen.add(id);
      unique.push(stock.id === id ? stock : { ...stock, id });
    }
    return unique;
  }

  /** Add an opened stock unless that file is already in the panel. */
  static include(stocks: StockSummary[], stock: StockState | null): StockSummary[] {
    if (!stock) {
      return stocks;
    }
    const id = StockList.id(stock);
    if (stocks.some((item) => StockList.id(item) === id)) {
      return stocks;
    }
    return [
      ...stocks,
      {
        id,
        path: stock.path,
        name: stock.name,
        model: stock.model,
        revision: stock.revision,
      },
    ];
  }
}
