import { describe, it, expect } from 'vitest';
import { clampVariant } from '../panels.js';

describe('clampVariant', () => {
  const variants = ['Sine', 'Tri', 'Saw', 'Pulse'];

  it('passes through an in-range integer', () => {
    expect(clampVariant(2, variants)).toBe(2);
  });

  it('clamps below zero to zero', () => {
    expect(clampVariant(-5, variants)).toBe(0);
  });

  it('clamps above len-1 to len-1', () => {
    expect(clampVariant(10, variants)).toBe(variants.length - 1);
  });

  it('rounds non-integer plain', () => {
    expect(clampVariant(1.6, variants)).toBe(2);
    expect(clampVariant(1.4, variants)).toBe(1);
  });

  it('collapses to 0 on a single-variant table', () => {
    expect(clampVariant(0, ['Only'])).toBe(0);
    expect(clampVariant(42, ['Only'])).toBe(0);
    expect(clampVariant(-1, ['Only'])).toBe(0);
  });
});
