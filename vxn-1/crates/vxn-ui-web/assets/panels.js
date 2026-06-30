// panels.js — re-export barrel (ticket 0141).
//
// The faceplate primitives that used to live here in one 1100-line god-file are
// now split into cohesive modules under `panels/` + `util/` (matching VXN2's
// modular `panels/` layout):
//
//   util/drag.js          — drag / paint / value-popup primitives + clampVariant
//                           / tgRow (consumes the shared wireDrag, 0140)
//   panels/fader.js       — fader, LFO-rate subdivision label, wave knob,
//                           Detune+Legato composite, waveform glyphs
//   panels/discrete.js    — Switch / ButtonGroup / Dropdown / HeaderSwitch + FX
//                           tab strip
//   panels/keys.js        — the Keys panel + its constants
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
  wireFaderDrag, attachValuePop, paintFader, clampVariant, tgRow,
} from './util/drag.js';

export {
  WAVE_GLYPHS, glyphPath, SVG_NS, TWIN_TOP_CT,
  makeFader, subdivisionLabel, makeWave, makeDetuneLegato,
} from './panels/fader.js';

export {
  makeSwitch, makeButtonGroup, makeDropdown, makeHeaderSwitch, wireFxTabs,
} from './panels/discrete.js';

export {
  KEY_MODE_NAMES, KEY_LAYERS,
  KEYS_DEFAULT_SPLIT, KEYS_SPLIT_MIN, KEYS_SPLIT_MAX,
  keysPanel,
} from './panels/keys.js';

export { presetBar } from './panels/preset-bar.js';
