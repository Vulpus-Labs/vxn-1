// The key-scaling curve.
//
// `ksDb` is the whole point of the panel: it is what lets an operator be "a
// little quieter as you go up, a lot quieter as you go down", which a single
// key-track number cannot express. The two sides are independent in depth,
// curve AND direction, and the test is here because "independent" is the sort
// of property that quietly stops being true.

import { describe, it, expect } from "vitest";
import { ksDb, KS_DB_SPAN } from "../panels/ks-graph.js";

const flat = { bp: 60, l: 0, r: 0, lExp: false, rExp: false, lBoost: false, rBoost: false };

describe("ksDb", () => {
  it("is silent at the break point and flat at zero depth", () => {
    // `toBeCloseTo`, not `toBe`: a cut of zero comes out as -0, which is the
    // same number and a different assertion.
    expect(ksDb(flat, 60)).toBeCloseTo(0, 10);
    for (const k of [0, 30, 60, 90, 127]) expect(ksDb(flat, k)).toBeCloseTo(0, 10);
  });

  it("cuts below and boosts above, independently", () => {
    const ks = { ...flat, l: 99, r: 99, rBoost: true };
    expect(ksDb(ks, 0)).toBeLessThan(0);
    expect(ksDb(ks, 127)).toBeGreaterThan(0);
    // Only one side moves when only one side is set.
    const leftOnly = { ...flat, l: 99 };
    expect(ksDb(leftOnly, 0)).toBeLessThan(0);
    expect(ksDb(leftOnly, 127)).toBeCloseTo(0, 10);
  });

  it("bends the exponential side harder near the break point", () => {
    const lin = { ...flat, r: 99 };
    const exp = { ...flat, r: 99, rExp: true };
    const near = 60 + 20;
    expect(Math.abs(ksDb(exp, near))).toBeLessThan(Math.abs(ksDb(lin, near)));
    // Both arrive at the same place once the normaliser saturates.
    expect(ksDb(exp, 120)).toBeCloseTo(ksDb(lin, 120), 6);
  });

  it("never exceeds the graph's full scale", () => {
    const ks = { bp: 0, l: 99, r: 99, lExp: false, rExp: false, lBoost: true, rBoost: true };
    for (let k = 0; k <= 127; k++) expect(Math.abs(ksDb(ks, k))).toBeLessThanOrEqual(KS_DB_SPAN);
  });
});
