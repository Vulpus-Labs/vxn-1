// 0208: coverage for the `makeDial` rotary primitive (Dynamics pane). Mirrors
// the style of paint-fader / wire-drag tests: pull the primitive from the real
// `../panels.js` barrel, stub `window.vxn.send`, and drive real pointer events
// through the shared `wireDrag`. jsdom has no layout, so the drag math is
// exercised via clientY deltas, not computed geometry.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { makeDial } from '../panels.js';
import { pointerEvt } from './_helpers.js';

// makeDial rotates `.dial-indicator-g` by an affine map of norm:
//   deg = -135 + norm * 270  (−135° at norm 0, +135° at norm 1).
const DIAL_CX = 18;
const DIAL_CY = 18;
function expectedDeg(norm) {
  return (-135 + norm * 270).toFixed(2);
}
function rotateAttr(deg) {
  return `rotate(${deg} ${DIAL_CX} ${DIAL_CY})`;
}

const DESC = { name: 'comp_amount', label: 'Amount', min: 0, max: 1, default: 0, kind: 'float' };

describe('makeDial', () => {
  let el, send, rafSpy;

  beforeEach(() => {
    document.body.innerHTML = '';
    el = document.createElement('div');
    el.dataset.control = 'dial';
    el.dataset.param = 'comp_amount';
    // wireDrag captures the pointer; jsdom lacks these on plain elements.
    el.setPointerCapture = vi.fn();
    el.releasePointerCapture = vi.fn();
    document.body.appendChild(el);

    send = {
      beginGesture: vi.fn(),
      endGesture: vi.fn(),
      setParamNorm: vi.fn(),
    };
    globalThis.window = globalThis;
    window.vxn = { send };

    // Run the dial's rAF-throttled moves synchronously so a drag is deterministic.
    rafSpy = vi
      .spyOn(window, 'requestAnimationFrame')
      .mockImplementation((cb) => { cb(); return 1; });
  });

  afterEach(() => {
    rafSpy.mockRestore();
  });

  it('renders the SVG dial + label', () => {
    makeDial(el, 7, DESC);
    const svg = el.querySelector('svg.ctl-dial');
    expect(svg).not.toBeNull();
    // Track + fill arcs, the face circle, and the rotating indicator group.
    expect(el.querySelector('.dial-track')).not.toBeNull();
    expect(el.querySelector('.dial-fill')).not.toBeNull();
    expect(el.querySelector('.dial-face')).not.toBeNull();
    expect(el.querySelector('.dial-indicator-g')).not.toBeNull();
    const lbl = el.querySelector('.ctl-label');
    expect(lbl).not.toBeNull();
    expect(lbl.textContent).toBe('AMOUNT');
  });

  it('omits the label when data-no-label is set', () => {
    el.setAttribute('data-no-label', '');
    makeDial(el, 7, DESC);
    expect(el.querySelector('.ctl-label')).toBeNull();
    expect(el.querySelector('svg.ctl-dial')).not.toBeNull();
  });

  it('seeds the indicator at norm 0 (−135°)', () => {
    makeDial(el, 7, DESC);
    const g = el.querySelector('.dial-indicator-g');
    expect(g.getAttribute('transform')).toBe(rotateAttr(expectedDeg(0)));
  });

  it('update(plain, norm, display) rotates the indicator with norm', () => {
    const ctl = makeDial(el, 7, DESC);
    const g = el.querySelector('.dial-indicator-g');

    ctl.update(0.5, 0.5, '50%');
    expect(g.getAttribute('transform')).toBe(rotateAttr(expectedDeg(0.5))); // 0°

    ctl.update(1, 1, '100%');
    expect(g.getAttribute('transform')).toBe(rotateAttr(expectedDeg(1))); // +135°

    // A different norm paints a different rotation — the indicator tracks norm,
    // not plain.
    ctl.update(0.9, 0.25, '25%');
    expect(g.getAttribute('transform')).toBe(rotateAttr(expectedDeg(0.25)));
  });

  it('update also grows the fill arc (different d per norm)', () => {
    const ctl = makeDial(el, 7, DESC);
    const fill = el.querySelector('.dial-fill');
    ctl.update(0.25, 0.25, '');
    const dLow = fill.getAttribute('d');
    ctl.update(0.75, 0.75, '');
    const dHigh = fill.getAttribute('d');
    expect(dHigh).not.toBe(dLow);
  });

  it('a drag brackets a gesture and writes norm', () => {
    makeDial(el, 7, DESC);
    const svg = el.querySelector('svg.ctl-dial');

    svg.dispatchEvent(pointerEvt('pointerdown', { clientY: 100 }));
    // onDown re-anchors: beginGesture + a setParamNorm at the grab norm (0).
    expect(send.beginGesture).toHaveBeenCalledWith(7);
    expect(send.setParamNorm).toHaveBeenCalledWith(7, 0);

    send.setParamNorm.mockClear();
    // Drag up 100 px → norm rises by 100/200 = 0.5 (up = negative clientY delta).
    svg.dispatchEvent(pointerEvt('pointermove', { clientY: 0 }));
    expect(send.setParamNorm).toHaveBeenCalledWith(7, 0.5);

    svg.dispatchEvent(pointerEvt('pointerup', { clientY: 0 }));
    expect(send.endGesture).toHaveBeenCalledWith(7);
  });

  it('clamps the drag norm into [0, 1]', () => {
    makeDial(el, 7, DESC);
    const svg = el.querySelector('svg.ctl-dial');

    svg.dispatchEvent(pointerEvt('pointerdown', { clientY: 100 }));
    send.setParamNorm.mockClear();
    // Drag far up (dy hugely negative) → norm clamps to 1.
    svg.dispatchEvent(pointerEvt('pointermove', { clientY: -100000 }));
    expect(send.setParamNorm).toHaveBeenLastCalledWith(7, 1);
    // Drag far down → norm clamps to 0.
    svg.dispatchEvent(pointerEvt('pointermove', { clientY: 100000 }));
    expect(send.setParamNorm).toHaveBeenLastCalledWith(7, 0);
    svg.dispatchEvent(pointerEvt('pointerup', { clientY: 0 }));
  });
});
