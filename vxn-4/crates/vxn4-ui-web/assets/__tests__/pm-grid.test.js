// The PM grid's sign encoding.
//
// Worth a test rather than an eyeball: the encoding is deliberately redundant
// — hue AND an accent-bar edge — precisely because hue alone fails for a
// blue-violet colour-blind player, and the redundancy is exactly the kind of
// thing a later tidy-up removes for looking duplicated.

import { describe, it, expect } from "vitest";
import { pmCellStyle, PM_TINT_POS, PM_TINT_NEG, BUS_TINT } from "../panels/pm-grid.js";

describe("pmCellStyle", () => {
  it("paints nothing at all for an unwired route, whatever its depth", () => {
    for (const k of [0, 0.25, 0.5, 0.75, 1]) {
      expect(pmCellStyle(k, false, false)).toEqual({ boxShadow: "", background: "" });
    }
  });

  it("outlines a wired route even where its depth is zero", () => {
    // A route authored at nothing is still part of the patch's shape and has
    // to be findable; the outline is what says so.
    const s = pmCellStyle(0.5, true, false);
    expect(s.boxShadow).toContain("inset 0 0 0 1px");
    expect(s.background).toBe("");
  });

  it("encodes the sign twice — hue and the accent edge", () => {
    const pos = pmCellStyle(0.9, true, false);
    const neg = pmCellStyle(0.1, true, false);
    expect(pos.boxShadow).toContain(PM_TINT_POS);
    expect(neg.boxShadow).toContain(PM_TINT_NEG);
    // Bottom edge for positive, top edge for negative — the same up/down sense
    // as the bipolar fader the depth is dragged on.
    expect(pos.boxShadow).toContain("inset 0 -3px");
    expect(neg.boxShadow).toContain("inset 0 3px");
  });

  it("gives a zero depth no accent bar, because it has no sign to report", () => {
    expect(pmCellStyle(0.5, true, false).boxShadow).not.toContain("3px 0 -1px");
  });

  it("treats the bus send as unipolar: its own tint, and never an accent bar", () => {
    const quiet = pmCellStyle(0.1, true, true);
    const loud = pmCellStyle(0.9, true, true);
    expect(quiet.boxShadow).toContain(BUS_TINT);
    expect(loud.boxShadow).not.toContain("3px 0 -1px");
    // A low send is a low send, not a negative one: opacity rises with k
    // across the whole range rather than folding at the centre.
    const alpha = (s) => Number(s.background.match(/,\s*([0-9.]+)\)$/)[1]);
    expect(alpha(loud)).toBeGreaterThan(alpha(quiet));
  });

  it("brightens with depth in both directions from centre", () => {
    const alpha = (k) => Number(pmCellStyle(k, true, false).background.match(/,\s*([0-9.]+)\)$/)[1]);
    expect(alpha(1)).toBeCloseTo(alpha(0), 3);
    expect(alpha(1)).toBeGreaterThan(alpha(0.75));
    expect(alpha(0.75)).toBeGreaterThan(alpha(0.6));
  });
});
