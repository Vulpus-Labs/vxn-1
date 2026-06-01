import { describe, it, expect, beforeEach } from 'vitest';
import { subdivisionLabel } from '../panels.js';

beforeEach(() => {
  globalThis.window = globalThis;
});

describe('subdivisionLabel', () => {
  it('returns the empty string when the subdivisions table is empty', () => {
    window.vxn = { subdivisions: [] };
    expect(subdivisionLabel(0.5)).toBe('');
  });

  it('returns the empty string when window.vxn.subdivisions is missing', () => {
    window.vxn = {};
    expect(subdivisionLabel(0.5)).toBe('');
  });

  it('clamps below zero to the first entry', () => {
    window.vxn = { subdivisions: ['1/4', '1/8', '1/16'] };
    expect(subdivisionLabel(-1)).toBe('1/4');
  });

  it('clamps above one to the last entry', () => {
    window.vxn = { subdivisions: ['1/4', '1/8', '1/16'] };
    expect(subdivisionLabel(2)).toBe('1/16');
  });

  it('rounds an in-range norm to the matching entry', () => {
    // 3 entries → last = 2 → round(0.5 * 2) = 1 → '1/8'.
    window.vxn = { subdivisions: ['1/4', '1/8', '1/16'] };
    expect(subdivisionLabel(0.5)).toBe('1/8');
  });
});
