// Value mapping.
//
// `invValue` exists so the EG and key-scaling graphs can drag in seconds and
// put the answer back into a fader's normalised position. If the two disagree
// the graph and its faders fight each other on every pointermove, which reads
// as the handle "sticking" rather than as a maths bug.

import { describe, it, expect } from "vitest";
import { mapValue, invValue, formatValue, fmtHz, fmtSec } from "../panels/value.js";

const SPECS = {
  linear: { min: 0, max: 100 },
  bipolar: { min: -100, max: 100 },
  exp: { min: 0.001, max: 8, exp: 1 },
  expFromZero: { min: 0, max: 20000, exp: 1 },
};

describe("mapValue / invValue", () => {
  it("round-trips every taper", () => {
    for (const [name, spec] of Object.entries(SPECS)) {
      for (const t of [0, 0.13, 0.5, 0.87, 1]) {
        expect(invValue(spec, mapValue(spec, t)), `${name} at ${t}`).toBeCloseTo(t, 6);
      }
    }
  });

  it("does not divide by zero on a log taper whose range starts at zero", () => {
    // Plenty of specs want `min: 0` and mean "as near silence as the fader
    // gets"; the floor is clamped rather than the spec being rejected.
    const v = mapValue(SPECS.expFromZero, 0);
    expect(Number.isFinite(v)).toBe(true);
    expect(v).toBeGreaterThan(0);
  });

  it("quantises a stepped spec to its detents", () => {
    const den = { min: 1, max: 8, step: 1 };
    const seen = new Set();
    for (let i = 0; i <= 100; i++) seen.add(mapValue(den, i / 100));
    expect([...seen].sort((a, b) => a - b)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
  });
});

describe("formatValue", () => {
  it("prints a leading + only where the spec is signed", () => {
    expect(formatValue({ min: -100, max: 100, dp: 0, signed: true }, 1)).toBe("+100");
    expect(formatValue({ min: -100, max: 100, dp: 0 }, 1)).toBe("100");
    // Zero is not positive, and "+0" reads as a value someone dialled in.
    expect(formatValue({ min: -100, max: 100, dp: 0, signed: true }, 0.5)).toBe("0");
  });

  it("switches units where the number would stop being readable", () => {
    expect(fmtHz(440)).toBe("440 Hz");
    expect(fmtHz(12000)).toBe("12.00 kHz");
    expect(fmtSec(0.025)).toBe("25 ms");
    expect(fmtSec(2.5)).toBe("2.50 s");
  });
});
