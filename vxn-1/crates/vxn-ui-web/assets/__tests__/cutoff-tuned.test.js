import { describe, it, expect } from 'vitest';
// Direct coverage of the shared cutoff-tuned math (0140). Imported from the
// shared crate via the literal cross-crate path (allow-listed in
// vitest.config.js's server.fs), the same way the browser-panel tests reach
// preset-browser.js.
import {
  midiToHz, hzToMidi, noteName,
  cutoffTunedNormToHz, cutoffTunedHzToNorm, cutoffTunedNoteName,
  CUTOFF_TUNED_MIDI_MIN, CUTOFF_TUNED_MIDI_MAX,
} from '../../../../../crates/vxn-core-ui-web/assets/cutoff-tuned.js';

describe('midiToHz / hzToMidi round-trip', () => {
  it('hz → midi → hz is identity at exact semitones', () => {
    for (const m of [12, 33, 48, 60, 69, 96]) {
      const hz = midiToHz(m);
      expect(hzToMidi(hz)).toBeCloseTo(m, 9);
    }
  });

  it('concert A anchors the scale (69 ↔ 440 Hz)', () => {
    expect(midiToHz(69)).toBeCloseTo(440, 9);
    expect(hzToMidi(440)).toBeCloseTo(69, 9);
  });

  it('hzToMidi floors a non-positive Hz to the 1e-6 guard rather than -Infinity', () => {
    expect(Number.isFinite(hzToMidi(0))).toBe(true);
  });
});

describe('cutoffTunedNormToHz / cutoffTunedHzToNorm', () => {
  it('norm → Hz → norm is identity at the snap grid', () => {
    // Each of the 49 semitones (12..60) sits at a norm of step/48.
    const span = CUTOFF_TUNED_MIDI_MAX - CUTOFF_TUNED_MIDI_MIN; // 48
    for (let step = 0; step <= span; step++) {
      const norm = step / span;
      const hz = cutoffTunedNormToHz(norm);
      expect(cutoffTunedHzToNorm(hz)).toBeCloseTo(norm, 9);
    }
  });

  it('snaps to the nearest semitone (norm between grid points lands on one)', () => {
    // Just below the C0 step boundary still rounds down to C0 (MIDI 12).
    expect(cutoffTunedNormToHz(0)).toBeCloseTo(midiToHz(12), 9);
    // Midpoint norm = C2 (MIDI 36).
    expect(cutoffTunedNormToHz(0.5)).toBeCloseTo(midiToHz(36), 9);
    // Top.
    expect(cutoffTunedNormToHz(1)).toBeCloseTo(midiToHz(60), 9);
  });

  it('clamps norm outside [0,1]', () => {
    expect(cutoffTunedNormToHz(-3)).toBeCloseTo(midiToHz(12), 9);
    expect(cutoffTunedNormToHz(99)).toBeCloseTo(midiToHz(60), 9);
  });

  it('clamps Hz outside the tuned window to the end norms', () => {
    expect(cutoffTunedHzToNorm(1)).toBe(0);          // sub-C0 → 0
    expect(cutoffTunedHzToNorm(1e6)).toBe(1);        // super-C4 → 1
  });
});

describe('cutoffTunedNoteName', () => {
  it('names the snapped semitone for a Hz value', () => {
    expect(cutoffTunedNoteName(midiToHz(36))).toBe('C2');
    expect(cutoffTunedNoteName(midiToHz(60))).toBe(noteName(60)); // C4
  });

  it('clamps out-of-window Hz to the end note names', () => {
    expect(cutoffTunedNoteName(1)).toBe('C0');
    expect(cutoffTunedNoteName(1e6)).toBe('C4');
  });
});
