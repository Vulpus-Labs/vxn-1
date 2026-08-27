// 0318: coverage for the `makeWave` waveform-selector primitive. It had none
// before its geometry / glyph / face / indicator seams were pulled out into
// helpers, so this pins the pose it draws and the two ways it posts: a glyph
// click (absolute pick) and a vertical drag (relative detent walk).
//
// Same shape as dial.test.js: real primitive from the `../panels.js` barrel,
// stubbed `window.vxn.send`, real pointer events through the shared `wireDrag`.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { makeWave } from '../panels.js';
import { pointerEvt, mountEl } from './_helpers.js';

// Four variants across the 270° arc ⇒ 90° per detent, starting at -135°.
const VARIANTS = ['Saw', 'Square', 'Triangle', 'Sine'];
const DESC = { name: 'osc1_wave', label: 'Wave', variants: VARIANTS };
const ID = 21;

function deg(i) {
  return `rotate(${(-135 + i * 90).toFixed(2)})`;
}

describe('makeWave', () => {
  let el, send, ctl;

  beforeEach(() => {
    document.body.innerHTML = '';
    el = mountEl();
    el.dataset.control = 'wave';
    el.dataset.param = 'osc1_wave';
    send = {
      beginGesture: vi.fn(),
      endGesture: vi.fn(),
      setParam: vi.fn(),
      discrete: vi.fn(),
    };
    window.vxn = { send };
    ctl = makeWave(el, ID, DESC);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    delete window.vxn;
  });

  const svg = () => el.querySelector('svg.ctl-wave');
  // Glyph groups are appended first, so they lead the SVG's children; the face
  // circles and the indicator group follow.
  const glyphGroups = () => Array.from(svg().querySelectorAll('g')).filter((g) => g.querySelector('path'));
  const indicatorG = () => Array.from(svg().querySelectorAll('g')).find((g) => g.querySelector('line'));

  it('draws one glyph group per variant, plus the face and the indicator', () => {
    expect(svg()).not.toBeNull();
    expect(svg().getAttribute('viewBox')).toBe('0 0 64 64');
    expect(glyphGroups()).toHaveLength(VARIANTS.length);
    // Rim + dimple.
    expect(svg().querySelectorAll('circle')).toHaveLength(2);
    expect(indicatorG()).toBeDefined();
    expect(el.querySelector('.ctl-label').textContent).toBe('WAVE');
  });

  it('seeds the indicator at the arc start and lights variant 0', () => {
    expect(indicatorG().getAttribute('transform')).toBe(deg(0));
    const strokes = glyphGroups().map((g) => g.querySelector('path').getAttribute('stroke'));
    expect(strokes).toEqual([
      'var(--glyph-active)', 'var(--glyph)', 'var(--glyph)', 'var(--glyph)',
    ]);
  });

  it('rotates one detent per variant on update, and moves the active glyph', () => {
    for (let i = 0; i < VARIANTS.length; i++) {
      ctl.update(i, i / 3, VARIANTS[i]);
      expect(indicatorG().getAttribute('transform')).toBe(deg(i));
      const strokes = glyphGroups().map((g) => g.querySelector('path').getAttribute('stroke'));
      expect(strokes.filter((s) => s === 'var(--glyph-active)')).toHaveLength(1);
      expect(strokes[i]).toBe('var(--glyph-active)');
    }
  });

  it('clamps an out-of-range echo to the variant list', () => {
    ctl.update(99, 1, 'Sine');
    expect(indicatorG().getAttribute('transform')).toBe(deg(VARIANTS.length - 1));
    ctl.update(-4, 0, 'Saw');
    expect(indicatorG().getAttribute('transform')).toBe(deg(0));
  });

  it('posts an absolute pick when a glyph is clicked, without a gesture', () => {
    glyphGroups()[2].dispatchEvent(pointerEvt('pointerdown'));
    expect(send.discrete).toHaveBeenCalledWith(ID, 2);
    // The glyph stops propagation, so the knob's drag never opens a gesture.
    expect(send.beginGesture).not.toHaveBeenCalled();
  });

  it('walks detents on a vertical drag, bracketed by a gesture', () => {
    const target = svg();
    target.setPointerCapture = vi.fn();
    target.releasePointerCapture = vi.fn();
    target.dispatchEvent(pointerEvt('pointerdown', { clientY: 200 }));
    expect(send.beginGesture).toHaveBeenCalledWith(ID);
    // Up is a negative clientY delta, so dragging up raises the variant index.
    // PIXELS_PER_DETENT is 30, so -60 px is +2 detents: variant 0 → 2.
    target.dispatchEvent(pointerEvt('pointermove', { clientY: 140 }));
    target.dispatchEvent(pointerEvt('pointerup', { clientY: 140 }));
    expect(send.endGesture).toHaveBeenCalledWith(ID);
    const posted = send.setParam.mock.calls.map((c) => c[1]);
    expect(posted.at(-1)).toBe(2);
    for (const v of posted) {
      expect(Number.isInteger(v)).toBe(true);
      expect(v).toBeGreaterThanOrEqual(0);
      expect(v).toBeLessThanOrEqual(VARIANTS.length - 1);
    }
  });
});
