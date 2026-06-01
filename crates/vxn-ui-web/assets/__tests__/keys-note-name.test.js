import { describe, it, expect } from 'vitest';
import { keysNoteName } from '../panels.js';

// keysNoteName maps a MIDI note number to a "<NAME><OCTAVE>" string with
// the General-MIDI octave convention (n=60 → C4). The split-point slider's
// readout is the only consumer.
describe('keysNoteName', () => {
  it('returns C0 for n=12', () => {
    expect(keysNoteName(12)).toBe('C0');
  });

  it('returns A4 for n=69 (concert A)', () => {
    expect(keysNoteName(69)).toBe('A4');
  });

  it('handles the bottom of the range with a negative octave', () => {
    // n=0 → octave = floor(0/12) - 1 = -1, name = 'C'.
    expect(keysNoteName(0)).toBe('C-1');
  });

  it('wraps negative note numbers via the mod-12 fixup', () => {
    // n=-1 → ((−1 mod 12) + 12) mod 12 = 11 → 'B'; octave = floor(-1/12) - 1 = -2.
    expect(keysNoteName(-1)).toBe('B-2');
  });

  it('returns C7 at the top of the MIDI range used by the keys panel', () => {
    expect(keysNoteName(96)).toBe('C7');
  });
});
