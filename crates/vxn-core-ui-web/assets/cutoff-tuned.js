// Filter "Cutoff Tuned" mode math — shared primitive (0140).
//
// When a filter-cutoff fader is in tuned mode its range is read/displayed as
// a musical note over MIDI C0..C4 (12..60), semitone-snapped, while the
// stored param stays Hz so the DSP and DAW automation are unaffected (the
// engine never reads the toggle). Both faceplates had drifting three-way
// copies of these helpers — VXN1 `panels.js`, VXN2 `main.js` ("Mirrors
// VXN-1's panels.js") and VXN2 `bootstrap.js` (the bare `noteName`). One
// module now owns them.
//
// ES module so the vitest suites can `import` the pure helpers and assert
// the `cutoffTunedNormToHz` / `hzToMidi` round-trip directly; the `export`
// markers are stripped at splice time (see `strip_esm_exports`).

// Fader range: MIDI C0..C4, C2 at the midpoint.
export const CUTOFF_TUNED_MIDI_MIN = 12; // C0
export const CUTOFF_TUNED_MIDI_MAX = 60; // C4

export function midiToHz(m) {
  return 440 * Math.pow(2, (m - 69) / 12);
}
export function hzToMidi(hz) {
  return 69 + 12 * Math.log2(Math.max(1e-6, hz) / 440);
}

// MIDI note number → name (e.g. 60 → "C4"). General-purpose: `Math.round`
// tolerates a fractional MIDI value. Replaces VXN1's `keysNoteName` and the
// two VXN2 `noteName` copies.
const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
export function noteName(m) {
  const n = Math.round(m);
  return NOTE_NAMES[((n % 12) + 12) % 12] + (Math.floor(n / 12) - 1);
}

// norm [0,1] → MIDI 12..60 (semitone-snapped) → Hz.
export function cutoffTunedNormToHz(norm) {
  const span = CUTOFF_TUNED_MIDI_MAX - CUTOFF_TUNED_MIDI_MIN;
  const midi = Math.round(CUTOFF_TUNED_MIDI_MIN + Math.max(0, Math.min(1, norm)) * span);
  return midiToHz(midi);
}
export function cutoffTunedHzToNorm(hz) {
  const midi = Math.max(
    CUTOFF_TUNED_MIDI_MIN,
    Math.min(CUTOFF_TUNED_MIDI_MAX, Math.round(hzToMidi(hz))),
  );
  return (midi - CUTOFF_TUNED_MIDI_MIN) / (CUTOFF_TUNED_MIDI_MAX - CUTOFF_TUNED_MIDI_MIN);
}
export function cutoffTunedNoteName(hz) {
  const midi = Math.max(
    CUTOFF_TUNED_MIDI_MIN,
    Math.min(CUTOFF_TUNED_MIDI_MAX, Math.round(hzToMidi(hz))),
  );
  return noteName(midi);
}
