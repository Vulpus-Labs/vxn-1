import { describe, it, expect } from 'vitest';
import { glyphPath, WAVE_GLYPHS } from '../panels.js';

describe('glyphPath', () => {
  it('emits an M+L chain for a known multi-segment glyph (Pulse, 5 pts)', () => {
    const d = glyphPath('Pulse', 100, 50);
    expect(d).not.toBeNull();
    // 1 M + (n-1) L commands.
    const ms = d.match(/M/g) || [];
    const ls = d.match(/L/g) || [];
    expect(ms.length).toBe(1);
    expect(ls.length).toBe(WAVE_GLYPHS['Pulse'].length - 1);
  });

  it('emits an M then 16 L for Sine (17 sample points)', () => {
    const d = glyphPath('Sine', 100, 50);
    const ls = d.match(/L/g) || [];
    expect(ls.length).toBe(16);
  });

  it('returns null for an unknown glyph label', () => {
    expect(glyphPath('NopeWave', 100, 50)).toBeNull();
  });

  it('scales coordinates by the w / h params', () => {
    // First Sine point is [0, 0.5 - 0.38*sin(0)] = [0, 0.5]. Scaled to
    // (w=200, h=100) it lands at "M0.00 50.00".
    const d = glyphPath('Sine', 200, 100);
    expect(d.startsWith('M0.00 50.00')).toBe(true);

    // Pulse's second point is [0, 0.15] — scaled to (w=80, h=40) it's
    // (0, 6). The path runs M then L immediately for Pulse's vertical
    // jump at the start.
    const dp = glyphPath('Pulse', 80, 40);
    expect(dp.includes('L0.00 6.00')).toBe(true);
  });
});
