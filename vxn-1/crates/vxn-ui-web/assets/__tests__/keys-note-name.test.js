import { describe, it, expect } from 'vitest';
import { noteName } from '../panels.js';

// `noteName` (the shared `vxn-core-ui-web/assets/cutoff-tuned.js` primitive,
// re-exported from panels.js — 0140; formerly VXN1's `keysNoteName`) maps a
// MIDI note number to a "<NAME><OCTAVE>" string with the General-MIDI octave
// convention (n=60 → C4). The split-point slider readout is one consumer.
describe('noteName', () => {
  it('returns C0 for n=12', () => {
    expect(noteName(12)).toBe('C0');
  });

  it('returns A4 for n=69 (concert A)', () => {
    expect(noteName(69)).toBe('A4');
  });

  it('handles the bottom of the range with a negative octave', () => {
    // n=0 → octave = floor(0/12) - 1 = -1, name = 'C'.
    expect(noteName(0)).toBe('C-1');
  });

  it('wraps negative note numbers via the mod-12 fixup', () => {
    // n=-1 → ((−1 mod 12) + 12) mod 12 = 11 → 'B'; octave = floor(-1/12) - 1 = -2.
    expect(noteName(-1)).toBe('B-2');
  });

  it('returns C7 at the top of the MIDI range used by the keys panel', () => {
    expect(noteName(96)).toBe('C7');
  });

  it('rounds a fractional MIDI value before naming it', () => {
    // The shared `noteName` tolerates non-integer input (VXN2 fed it
    // `Math.round`-ed cutoff MIDI); 59.6 → 60 → C4.
    expect(noteName(59.6)).toBe('C4');
  });
});
