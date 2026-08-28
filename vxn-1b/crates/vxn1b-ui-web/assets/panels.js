// panels.js — re-export barrel (ticket 0141).
//
// The faceplate primitives are split into cohesive modules under `panels/` +
// `util/` (matching VXN2's modular `panels/` layout):
//
//   util/drag.js          — drag / paint / value-popup primitives + clampVariant
//                           / tgRow (consumes the shared wireDrag, 0140)
//   panels/fader.js       — fader, LFO-rate subdivision label, wave knob,
//                           bipolar dial, waveform glyphs
//   panels/discrete.js    — Switch / ButtonGroup / HeaderSwitch
//   panels/meter.js       — level-meter ballistics + the stereo meter widget
//                           and the frame registry (0240)
//   panels/scope.js       — the layer oscilloscope: trigger search + canvas
//                           trace
//   panels/preset-bar.js  — the preset bar (dirty-tracking via the bridge's
//                           onMutation hook, not a sender monkey-patch)
//
// In production the splice loader concatenates those files directly (see
// `assemble_faceplate` in lib.rs) and drops every `export … from` line here, so
// this barrel contributes nothing to the inline bundle. Under Node ESM it
// re-exports the whole surface so the vitest suites — and any other consumer —
// keep importing from `../panels.js` unchanged. The shared widgets
// (`valuePop` / `wireDrag` / the cutoff-tuned + note-name helpers) are
// re-exported straight from the shared crate, exactly as the old module did.

export { valuePop } from '../../../../crates/vxn-core-ui-web/assets/value-pop.js';
export { wireDrag } from '../../../../crates/vxn-core-ui-web/assets/wire-drag.js';
export {
  midiToHz, hzToMidi, noteName,
  cutoffTunedNormToHz, cutoffTunedHzToNorm, cutoffTunedNoteName,
  CUTOFF_TUNED_MIDI_MIN, CUTOFF_TUNED_MIDI_MAX,
} from '../../../../crates/vxn-core-ui-web/assets/cutoff-tuned.js';

export {
  PIXELS_PER_DETENT, KNOB_INDICATOR_TRANSITION_MS,
  wireFaderDrag, wireNormDrag, attachValuePop, paintFader, clampVariant, tgRow,
} from './util/drag.js';

export {
  WAVE_GLYPHS, glyphPath, SVG_NS,
  makeFader, subdivisionLabel, makeWave, makeDial, makeBipolar,
} from './panels/fader.js';

export {
  makeSwitch, makeButtonGroup, makeRocker, makeHeaderSwitch,
} from './panels/discrete.js';

export { presetBar } from './panels/preset-bar.js';

export { matrixOverlay } from './panels/matrix.js';

export {
  SCOPE_SEARCH_FRACTION, SCOPE_RANGE,
  findFirstRisingCross, scopeStart, makeScope,
} from './panels/scope.js';

export {
  METER_FLOOR_DB, METER_CLIP_DB, METER_DECAY_DB_PER_S, METER_HOLD_MS,
  METER_PEAK_DECAY_DB_PER_S, GR_RELEASE_DB_PER_S,
  toDb, dbToNorm, advanceMeter, initialMeterState, grToNorm, advanceGr,
  makeMeter, meterRegistry,
} from './panels/meter.js';
