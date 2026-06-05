import { describe, it, expect, beforeEach } from 'vitest';
import { variantIdx } from '../dispatch.js';

// Fixture: two per-patch params with the same variants at their Upper /
// Lower ids (mirrors how `patch_clap_id` lays out a real param), plus one
// global param at id ≥ 2·patchCount. patchCount picked to match the real
// PATCH_COUNT so the layer-offset math reads naturally.
const PATCH_COUNT = 100;

beforeEach(() => {
  globalThis.window = globalThis;
  window.vxn = {
    patchCount: PATCH_COUNT,
    params: {
      // Per-patch enum (Upper = 0, Lower = 0 + PATCH_COUNT).
      0:                   { name: 'mode', variants: ['A', 'B', 'C'] },
      [PATCH_COUNT]:       { name: 'mode', variants: ['A', 'B', 'C'] },
      // Global enum (id ≥ 2·patchCount, layer-independent).
      [2 * PATCH_COUNT]:   { name: 'shape', variants: ['Sine', 'Tri'] },
    },
  };
});

describe('variantIdx', () => {
  it('returns the index of a known variant on the upper layer', () => {
    expect(variantIdx('mode', 'B', 'upper')).toBe(1);
  });

  it('returns -1 for an unknown variant', () => {
    expect(variantIdx('mode', 'X', 'upper')).toBe(-1);
  });

  it('returns -1 for an unknown param', () => {
    expect(variantIdx('missing', 'A', 'upper')).toBe(-1);
  });

  it('routes per-patch params through the layer offset', () => {
    // Lower-side translation hits id = PATCH_COUNT — the fixture's Lower
    // entry has the same variants, so the index matches.
    expect(variantIdx('mode', 'C', 'lower')).toBe(2);
  });

  it('treats globals (id ≥ 2·patchCount) as layer-independent', () => {
    expect(variantIdx('shape', 'Tri', 'upper')).toBe(1);
    expect(variantIdx('shape', 'Tri', 'lower')).toBe(1);
  });
});
