import { describe, it, expect } from "vitest";
// Direct coverage of the shared cutoff-tuned math (0140), imported from the
// sibling crate (allow-listed via server.fs in vitest.config.js). VXN2's
// main.js used to carry its own copy of these helpers; the shared owner is
// now the single source, so this suite guards it from the VXN2 side too.
import {
  midiToHz, hzToMidi, noteName,
  cutoffTunedNormToHz, cutoffTunedHzToNorm, cutoffTunedNoteName,
  CUTOFF_TUNED_MIDI_MIN, CUTOFF_TUNED_MIDI_MAX,
} from "../../../../../crates/vxn-core-ui-web/assets/cutoff-tuned.js";

describe("midiToHz / hzToMidi round-trip", () => {
  it("hz → midi → hz is identity at exact semitones", () => {
    for (const m of [12, 33, 48, 60, 69, 96]) {
      expect(hzToMidi(midiToHz(m))).toBeCloseTo(m, 9);
    }
  });

  it("concert A anchors the scale (69 ↔ 440 Hz)", () => {
    expect(midiToHz(69)).toBeCloseTo(440, 9);
    expect(hzToMidi(440)).toBeCloseTo(69, 9);
  });
});

describe("cutoffTunedNormToHz / cutoffTunedHzToNorm", () => {
  it("norm → Hz → norm is identity across the snap grid", () => {
    const span = CUTOFF_TUNED_MIDI_MAX - CUTOFF_TUNED_MIDI_MIN; // 48
    for (let step = 0; step <= span; step++) {
      const norm = step / span;
      expect(cutoffTunedHzToNorm(cutoffTunedNormToHz(norm))).toBeCloseTo(norm, 9);
    }
  });

  it("midpoint norm is C2 (MIDI 36); ends are C0 / C4", () => {
    expect(cutoffTunedNormToHz(0)).toBeCloseTo(midiToHz(12), 9);
    expect(cutoffTunedNormToHz(0.5)).toBeCloseTo(midiToHz(36), 9);
    expect(cutoffTunedNormToHz(1)).toBeCloseTo(midiToHz(60), 9);
  });

  it("clamps norm and Hz outside the window", () => {
    expect(cutoffTunedNormToHz(-3)).toBeCloseTo(midiToHz(12), 9);
    expect(cutoffTunedNormToHz(99)).toBeCloseTo(midiToHz(60), 9);
    expect(cutoffTunedHzToNorm(1)).toBe(0);
    expect(cutoffTunedHzToNorm(1e6)).toBe(1);
  });
});

describe("noteName / cutoffTunedNoteName", () => {
  it("names MIDI notes with the GM octave convention", () => {
    expect(noteName(60)).toBe("C4");
    expect(noteName(69)).toBe("A4");
    expect(noteName(12)).toBe("C0");
  });

  it("names the snapped semitone for a Hz value, clamped to the window", () => {
    expect(cutoffTunedNoteName(midiToHz(36))).toBe("C2");
    expect(cutoffTunedNoteName(1)).toBe("C0");
    expect(cutoffTunedNoteName(1e6)).toBe("C4");
  });
});
