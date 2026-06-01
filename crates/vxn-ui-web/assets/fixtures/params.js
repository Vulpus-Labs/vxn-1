// E015 / 0080: shared `window.vxn.params`-shaped fixture for dispatch
// tests. patchCount=10 — small enough to fit on one screen, large enough
// that per-patch ids (0..9) clearly separate from globals (≥ 20).
//
// Real `window.vxn.params` has ~150 entries; the dispatcher logic is
// identical at this scale.

export const PATCH_COUNT = 10;

function floatDesc(name, label, def = 0.0) {
  return {
    name,
    label,
    min: 0.0,
    max: 1.0,
    default: def,
    kind: 'float',
    unit: '',
    taper: { kind: 'linear' },
  };
}

function boolDesc(name, label, def = false) {
  return { name, label, min: 0, max: 1, default: def ? 1 : 0, kind: 'bool' };
}

function enumDesc(name, label, variants, def = 0) {
  return {
    name,
    label,
    min: 0,
    max: variants.length - 1,
    default: def,
    kind: 'enum',
    variants,
  };
}

// Build a fresh params object each call so tests can mutate without
// leaking state across files.
export function buildParams() {
  const ASSIGN_VARIANTS  = ['Poly', 'Unison', 'Solo', 'Twin'];
  const XMOD_VARIANTS    = ['Off', 'Sync', 'FM'];
  const FILTER_VARIANTS  = ['Lowpass', 'Highpass', 'Bandpass', 'Notch'];
  return {
    // Per-patch (upper at 0..9, lower at 10..19).
    0:  enumDesc('assign_mode', 'Assign', ASSIGN_VARIANTS, 0),
    1:  boolDesc('lfo1_free_run', 'Free'),
    2:  floatDesc('lfo1_delay_time', 'Delay'),
    3:  floatDesc('lfo1_fade', 'Fade'),
    4:  enumDesc('xmod_type', 'Cross Mod', XMOD_VARIANTS, 0),
    // Lower-layer twins (Upper id + PATCH_COUNT). Same name + shape.
    10: enumDesc('assign_mode', 'Assign', ASSIGN_VARIANTS, 0),
    11: boolDesc('lfo1_free_run', 'Free'),
    12: floatDesc('lfo1_delay_time', 'Delay'),
    13: floatDesc('lfo1_fade', 'Fade'),
    14: enumDesc('xmod_type', 'Cross Mod', XMOD_VARIANTS, 0),
    // Globals (id ≥ 2·PATCH_COUNT, layer-independent).
    20: enumDesc('filter_mode', 'Mode', FILTER_VARIANTS, 0),
    21: floatDesc('filter_slope', 'Slope'),
  };
}

// Convenience: install the fixture on `window.vxn`. Tests that don't need
// to mutate the result call this once in `beforeEach`.
export function installFixture() {
  globalThis.window = globalThis;
  window.vxn = {
    params: buildParams(),
    patchCount: PATCH_COUNT,
    subdivisions: [],
    send: {},
  };
}
