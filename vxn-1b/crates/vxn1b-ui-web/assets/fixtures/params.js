// 0209: shared `window.vxn.params`-shaped fixture for the dispatch suites.
//
// vxn1b is a single-patch synth (`window.vxn.patchCount === 1`), so a layer
// flip is a no-op: for any id ≥ 2·patchCount the upper/lower lookup returns the
// same id. The compact faceplate dropped the Cutoff "Tuned" strip and the Delay
// Sync toggle, so the only sync-partner pairs left are LFO 1 rate↔sync and
// LFO 2 rate↔sync.
//
// The `dim-rules.test.js` suite (which is out of scope for the 0209 rewrite and
// must stay green) still exercises the layer-offset machinery against this same
// fixture — it asserts per-patch watch ids translate by `+PATCH_COUNT` on the
// lower layer (`voice_mode`@0, `xmod_type`@4, `lfo1_free_run`@1) and looks up
// `xmod_type`/`filter_mode`/`filter_slope`/`lfo1_delay_time`/`lfo1_fade` by
// name. So the fixture keeps a representative multi-patch `PATCH_COUNT` and
// those descriptors intact. `dispatch-orchestration.test.js` overrides
// `window.vxn.patchCount = 1` locally to test the real single-patch behaviour.
//
// Real `window.vxn.params` has ~130 entries; the dispatcher logic is identical
// at this scale. This table carries only the params the two suites read.

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
function buildParams() {
  // The layer-rebind fixtures only need *an* enum at a known id; the real
  // surface is `stack_width` × `voice_mode` since 0266.
  const ASSIGN_VARIANTS  = ['Poly', 'Solo'];
  const XMOD_VARIANTS    = ['Off', 'Sync', 'FM'];
  const FILTER_VARIANTS  = ['Lowpass', 'Highpass', 'Bandpass', 'Notch'];
  return {
    // ── Per-patch block (upper 0..9, lower 10..19). The dim-rules suite
    // relies on the specific ids below (voice_mode@0, lfo1_free_run@1,
    // xmod_type@4) and their `+PATCH_COUNT` lower twins.
    0:  enumDesc('voice_mode', 'Voice', ASSIGN_VARIANTS, 0),
    1:  boolDesc('lfo1_free_run', 'Free'),
    2:  floatDesc('lfo1_delay_time', 'Delay'),
    3:  floatDesc('lfo1_fade', 'Fade'),
    4:  enumDesc('xmod_type', 'Cross Mod', XMOD_VARIANTS, 0),
    // Lower-layer twins (upper id + PATCH_COUNT). Same name + shape.
    10: enumDesc('voice_mode', 'Voice', ASSIGN_VARIANTS, 0),
    11: boolDesc('lfo1_free_run', 'Free'),
    12: floatDesc('lfo1_delay_time', 'Delay'),
    13: floatDesc('lfo1_fade', 'Fade'),
    14: enumDesc('xmod_type', 'Cross Mod', XMOD_VARIANTS, 0),
    // ── Globals (id ≥ 2·PATCH_COUNT, layer-independent). In vxn1b every
    // param is effectively a global (single patch → no layer offset), so the
    // sync/cutoff/filter params the dispatch suite drives all live here.
    20: enumDesc('filter_mode', 'Mode', FILTER_VARIANTS, 0),
    21: floatDesc('filter_slope', 'Slope'),
    22: floatDesc('cutoff', 'Cutoff', 0.5),
    23: floatDesc('resonance', 'Resonance', 0.0),
    // Cutoff's "Tuned" partner (0250) — the toggle that re-maps the fader to
    // note-quantised MIDI C0..C4.
    30: boolDesc('cutoff_tuned', 'Tuned'),
    // LFO 1 rate↔sync pair.
    24: floatDesc('lfo1_rate', 'Rate'),
    25: boolDesc('lfo1_sync', 'Sync'),
    // LFO 2 rate↔sync pair.
    26: floatDesc('lfo2_rate', 'LFO2 Rate'),
    27: boolDesc('lfo2_sync', 'LFO2 Sync'),
    // A couple of plain faders (no partner) for null-override coverage.
    28: floatDesc('drive', 'Drive', 0.0),
    29: floatDesc('mix', 'Mix', 0.5),
    // Delay time↔sync pair (0267) — global, so its id sits past 2 × patchCount
    // in the real table; here the fixture's ids are flat and the pairing code
    // reads the same either way.
    31: floatDesc('delay_time', 'Time', 0.35),
    32: boolDesc('delay_sync', 'Sync'),
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
