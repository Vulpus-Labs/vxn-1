// 0219: the bipolar mod-matrix depth fader (ported from vxn-2). The fill grows
// signed from the 50% centre so "same source, opposite depths" reads at a
// glance. jsdom computes no layout, so we assert the inline styles makeBipolar
// writes (left / width as %), driven through its `update` echo.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { makeBipolar } from '../panels.js';

const DESC = { label: 'Depth', min: -1, max: 1, default: 0 };

let el, ctl;

beforeEach(() => {
  window.vxn = {
    send: { setParamNorm: vi.fn(), beginGesture: vi.fn(), endGesture: vi.fn() },
  };
  document.body.innerHTML = '';
  el = document.createElement('div');
  el.dataset.label = ''; // no label (compact row cell)
  document.body.appendChild(el);
  ctl = makeBipolar(el, 5, DESC);
});

const fill = () => el.querySelector('.fader-track-fill');
const thumb = () => el.querySelector('.fader-thumb');

describe('makeBipolar', () => {
  it('renders a centre tick + track + fill + thumb, no label', () => {
    expect(el.querySelector('.vxn-mm-depth-center')).toBeTruthy();
    expect(el.querySelector('.fader-track')).toBeTruthy();
    expect(el.querySelector('.ctl-label')).toBeNull();
  });

  it('centre (0 depth) has zero-width fill and a centred thumb', () => {
    ctl.update(0, 0.5, '0.00');
    expect(fill().style.left).toBe('50%');
    expect(fill().style.width).toBe('0%');
    expect(thumb().style.left).toBe('50%');
  });

  it('positive depth fills right from centre', () => {
    ctl.update(0.5, 0.75, '+0.50');
    expect(fill().style.left).toBe('50%');
    expect(fill().style.width).toBe('25%');
    expect(thumb().style.left).toBe('75%');
  });

  it('negative depth fills left from centre', () => {
    ctl.update(-0.5, 0.25, '-0.50');
    expect(fill().style.left).toBe('25%');
    expect(fill().style.width).toBe('25%');
    expect(thumb().style.left).toBe('25%');
  });
});
