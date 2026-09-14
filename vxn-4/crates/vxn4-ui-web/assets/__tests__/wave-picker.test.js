// The operator waveform table.
//
// The `dc` flag is a claim about the waveform, not a decoration: it is the
// warning that this shape biases the sum bus when used as a carrier. A flag
// that disagreed with its own function would be worse than no flag, so the
// test measures the mean rather than trusting the label.

import { describe, it, expect } from "vitest";
import { OP_WAVE_DEFS, wavePreviewSvg } from "../panels/wave-picker.js";

const TAU = Math.PI * 2;

function mean(f) {
  const N = 4096;
  let s = 0;
  for (let i = 0; i < N; i++) s += f((i / N) * TAU);
  return s / N;
}

describe("OP_WAVE_DEFS", () => {
  it("flags exactly the waveforms with a non-zero mean", () => {
    for (const def of OP_WAVE_DEFS) {
      const m = mean(def.f);
      // 1% of full scale: below that the bias is inaudible and the chip would
      // be crying wolf on every waveform with a rounding error in it.
      expect(Math.abs(m) > 0.01, `${def.name} mean=${m.toFixed(4)}`).toBe(def.dc);
    }
  });

  it("stays inside a sane amplitude, so the preview is a shape and not a spike", () => {
    for (const def of OP_WAVE_DEFS) {
      for (let i = 0; i < 512; i++) {
        const v = def.f((i / 512) * TAU);
        expect(Number.isFinite(v), `${def.name} is not finite`).toBe(true);
        expect(Math.abs(v), `${def.name} exceeds full scale`).toBeLessThanOrEqual(1.0001);
      }
    }
  });
});

describe("wavePreviewSvg", () => {
  it("draws one full cycle inside the box", () => {
    const svg = wavePreviewSvg(OP_WAVE_DEFS[0], 92, 34);
    expect(svg).toContain('width="92"');
    const coords = [...svg.matchAll(/[ML]([\d.]+) ([\d.]+)/g)];
    expect(coords.length).toBeGreaterThan(50);
    for (const [, x, y] of coords) {
      expect(Number(x)).toBeGreaterThanOrEqual(0);
      expect(Number(x)).toBeLessThanOrEqual(92);
      expect(Number(y)).toBeGreaterThanOrEqual(0);
      expect(Number(y)).toBeLessThanOrEqual(34);
    }
  });

  it("clamps rather than escaping the box for a peaky waveform", () => {
    // Impulse is a Dirichlet kernel: its centre lobe is full scale and every
    // other sample is small. It must still draw inside its 34px box.
    const impulse = OP_WAVE_DEFS.find((d) => d.name === "Impulse");
    const svg = wavePreviewSvg(impulse, 92, 34);
    for (const [, , y] of svg.matchAll(/[ML]([\d.]+) ([\d.]+)/g)) {
      expect(Number(y)).toBeGreaterThanOrEqual(0);
      expect(Number(y)).toBeLessThanOrEqual(34);
    }
  });
});
