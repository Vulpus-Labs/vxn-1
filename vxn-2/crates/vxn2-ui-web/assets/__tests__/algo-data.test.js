import { describe, it, expect } from "vitest";
// IIFE attaches `window.__vxn.panels.algoData` (split out of op-row.js in
// 0141). Import for side effect, mirroring the production bundle load.
import "../panels/algo-data.js";

const { algoData } = window.__vxn.panels;

describe("algo-data — static algorithm tables", () => {
  it("exposes the carrier/fb tables, OP_PARAMS and isCarrier", () => {
    expect(algoData).toBeTruthy();
    expect(Array.isArray(algoData.ALGO_CARRIERS)).toBe(true);
    expect(Array.isArray(algoData.ALGO_FB_OPS)).toBe(true);
    expect(typeof algoData.isCarrier).toBe("function");
    expect(typeof algoData.OP_PARAMS).toBe("object");
  });

  it("ALGO_CARRIERS is 32 rows of 6 booleans", () => {
    expect(algoData.ALGO_CARRIERS).toHaveLength(32);
    for (const row of algoData.ALGO_CARRIERS) {
      expect(row).toHaveLength(6);
      for (const v of row) expect(typeof v).toBe("boolean");
    }
  });

  it("ALGO_FB_OPS is 32 entries, each a 1-indexed op in 1..6", () => {
    expect(algoData.ALGO_FB_OPS).toHaveLength(32);
    for (const op of algoData.ALGO_FB_OPS) {
      expect(Number.isInteger(op)).toBe(true);
      expect(op).toBeGreaterThanOrEqual(1);
      expect(op).toBeLessThanOrEqual(6);
    }
  });

  it("isCarrier reads the table for known algos (1-indexed)", () => {
    // Algo 1 carriers {1,3}.
    expect(algoData.isCarrier(1, 1)).toBe(true);
    expect(algoData.isCarrier(1, 2)).toBe(false);
    expect(algoData.isCarrier(1, 3)).toBe(true);
    expect(algoData.isCarrier(1, 6)).toBe(false);
    // Algo 16 carriers {1} only.
    expect(algoData.isCarrier(16, 1)).toBe(true);
    expect(algoData.isCarrier(16, 2)).toBe(false);
    // Algo 32 is fully additive — every op carries.
    for (let op = 1; op <= 6; op++) expect(algoData.isCarrier(32, op)).toBe(true);
  });

  it("isCarrier is false outside the [1,32]×[1,6] domain", () => {
    expect(algoData.isCarrier(0, 1)).toBe(false);
    expect(algoData.isCarrier(33, 1)).toBe(false);
    expect(algoData.isCarrier(1, 0)).toBe(false);
    expect(algoData.isCarrier(1, 7)).toBe(false);
  });

  it("isCarrier agrees with the raw table for the whole grid", () => {
    for (let a = 1; a <= 32; a++) {
      for (let op = 1; op <= 6; op++) {
        expect(algoData.isCarrier(a, op)).toBe(algoData.ALGO_CARRIERS[a - 1][op - 1]);
      }
    }
  });
});
