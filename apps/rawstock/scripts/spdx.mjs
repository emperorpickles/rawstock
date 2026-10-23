// SPDX license expressions against the GPL-3.0-compatible allowlist.
// OR passes when any side passes, AND only when every side does.
// `A WITH exception` is judged on A. Old crates write `MIT/Apache-2.0`,
// which means OR.

export const ALLOWED = new Set([
  "MIT",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Zlib",
  "0BSD",
  "Unicode-3.0",
  "Unicode-DFS-2016",
  "MPL-2.0",
  "LGPL-2.1",
  "LGPL-2.1-only",
  "LGPL-2.1-or-later",
  "LGPL-3.0",
  "LGPL-3.0-only",
  "LGPL-3.0-or-later",
  "CC0-1.0",
  // SIL Open Font License. The GNU license list marks OFL-1.1 as
  // GPL-compatible. Inter and JetBrains Mono ship under it.
  "OFL-1.1",
]);

export class Spdx {
  /** True when `expression` can be used under the allowlist. */
  static allowed(expression, allowed = ALLOWED) {
    if (typeof expression !== "string" || expression.trim() === "") {
      return false;
    }
    const tokens = Spdx.tokens(expression);
    const parser = new SpdxParser(tokens, allowed);
    const result = parser.or();
    if (parser.index !== tokens.length) {
      throw new Error(`Unexpected "${tokens[parser.index]}" in license "${expression}"`);
    }
    return result;
  }

  static tokens(expression) {
    return expression
      .replace(/\s*\/\s*/g, " OR ")
      .replace(/([()])/g, " $1 ")
      .split(/\s+/)
      .filter(Boolean);
  }
}

class SpdxParser {
  constructor(tokens, allowed) {
    this.tokens = tokens;
    this.allowed = allowed;
    this.index = 0;
  }

  peek() {
    return this.tokens[this.index];
  }

  next() {
    const token = this.tokens[this.index];
    if (token === undefined) {
      throw new Error("License expression ends early");
    }
    this.index += 1;
    return token;
  }

  or() {
    let result = this.and();
    while (this.peek()?.toUpperCase() === "OR") {
      this.next();
      const right = this.and();
      result = result || right;
    }
    return result;
  }

  and() {
    let result = this.term();
    while (this.peek()?.toUpperCase() === "AND") {
      this.next();
      const right = this.term();
      result = result && right;
    }
    return result;
  }

  term() {
    const token = this.next();
    if (token === "(") {
      const result = this.or();
      if (this.next() !== ")") {
        throw new Error("Unbalanced parentheses in license expression");
      }
      return result;
    }
    if (this.peek()?.toUpperCase() === "WITH") {
      this.next();
      this.next();
    }
    return this.allowed.has(token.replace(/\+$/, ""));
  }
}
