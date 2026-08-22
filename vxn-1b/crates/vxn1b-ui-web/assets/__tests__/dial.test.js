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

  // Bipolar dials (mixer pan / detune) grow their fill from the CENTRE detent,
  // not from the anticlockwise end of the track. jsdom has no SVG geometry, so
  // the assertions read the arc's endpoints out of the `d` string. An SVG arc
  // is always written clockwise here, so the centre is the path's END below
  // centre and its START above — either way one end of the lit span sits at
  // 12 o'clock, x = cx, y = cy - arcR (18, 3).
  describe('centre-origin fill (bipolar descriptor)', () => {
    const BIPOLAR = { name: 'layer_pan', label: 'Pan', min: -1, max: 1, default: 0, kind: 'float' };
    const TOP = ['18.00', '3.00'];
    // 'M sx sy A r r 0 f 1 ex ey' → [[sx, sy], [ex, ey]].
    function ends(d) {
      const t = d.split(/[\s]+/);
      return [[t[1], t[2]], [t[t.length - 2], t[t.length - 1]]];
    }

    it('anchors one end of the arc at 12 o\'clock on both sides of centre', () => {
      const ctl = makeDial(el, 7, BIPOLAR);
      const fill = el.querySelector('.dial-fill');

      ctl.update(-0.5, 0.25, 'L50');
      const [leftStart, leftEnd] = ends(fill.getAttribute('d'));
      // Below centre the span runs value → centre, so the TOP is the endpoint.
      expect(leftEnd).toEqual(TOP);
      expect(leftStart).not.toEqual(TOP);

      ctl.update(0.5, 0.75, 'R50');
      const [rightStart, rightEnd] = ends(fill.getAttribute('d'));
      // Above centre it runs centre → value, so the TOP is the start point.
      expect(rightStart).toEqual(TOP);
      expect(rightEnd).not.toEqual(TOP);
    });

    it('collapses to a stub at the centre detent', () => {
      const ctl = makeDial(el, 7, BIPOLAR);
      const fill = el.querySelector('.dial-fill');
      ctl.update(0, 0.5, 'C');
      const [start, end] = ends(fill.getAttribute('d'));
      expect(start).toEqual(TOP);
      // A hair of sweep, not a zero-length (NaN-prone) path.
      expect(end).not.toEqual(TOP);
      expect(Number(end[1])).toBeCloseTo(3.0, 2);
    });

    it('leaves a unipolar descriptor growing from norm 0', () => {
      const ctl = makeDial(el, 7, DESC);
      const fill = el.querySelector('.dial-fill');
      ctl.update(0.5, 0.5, '50%');
      const [start, end] = ends(fill.getAttribute('d'));
      // −135°: down-left of the face. The arc ENDS at the top at norm 0.5.
      expect(start).not.toEqual(TOP);
      expect(end).toEqual(TOP);
    });

    it('treats a range that merely ends at zero as unipolar', () => {
      const THRESH = { name: 'dynamics_threshold', label: 'Thresh', min: -60, max: 0, default: -12, kind: 'float' };
      const ctl = makeDial(el, 7, THRESH);
      const fill = el.querySelector('.dial-fill');
      ctl.update(-30, 0.25, '-30 dB');
      const [start] = ends(fill.getAttribute('d'));
      expect(start).toEqual(['7.39', '28.61']);  // −135°, the track's start
    });
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
