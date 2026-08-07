// 0248: the mixer's pan readout. A bipolar `[-1, 1]` position is a raw signed
// fraction everywhere the engine and the host see it — that is what automates —
// so the mixer-style `L50 / C / R50` label is purely the faceplate's, applied
// through the same `displayOverride` hook the cutoff/rate faders use.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panLabel, panDisplayOverride } from '../dispatch.js';
import { makeDial, valuePop } from '../panels.js';

describe('panLabel', () => {
  it('reads centre as C, with no sign or number', () => {
    expect(panLabel(0)).toBe('C');
    // Rounds to centre rather than reporting L0/R0, which would read as a
    // direction the layer is not actually in.
    expect(panLabel(0.001)).toBe('C');
    expect(panLabel(-0.001)).toBe('C');
  });

  it('reads negative as L and positive as R, in whole percent', () => {
    expect(panLabel(-1)).toBe('L100');
    expect(panLabel(-0.5)).toBe('L50');
    expect(panLabel(0.25)).toBe('R25');
    expect(panLabel(1)).toBe('R100');
  });
});

describe('panDisplayOverride', () => {
  it('applies to layer_pan only', () => {
    expect(panDisplayOverride('layer_pan')).toBeTypeOf('function');
    // Every other dial keeps the engine's own display string.
    expect(panDisplayOverride('dynamics_threshold')).toBeNull();
    expect(panDisplayOverride('layer_level')).toBeNull();
  });
});

describe('makeDial displayOverride', () => {
  let el;

  beforeEach(() => {
    valuePop.hide();
    // Deliberately NOT `document.body.innerHTML = ''`: the value popup is a
    // module-level singleton that appends itself to the body once and keeps the
    // node. Wiping the body detaches it, and it never re-attaches — the next
    // test would then query a popup that is no longer in the document.
    el?.remove();
    el = document.createElement('div');
    el.setPointerCapture = vi.fn();
    el.releasePointerCapture = vi.fn();
    document.body.appendChild(el);
    globalThis.window = globalThis;
    window.vxn = { send: { beginGesture: vi.fn(), endGesture: vi.fn(), setParamNorm: vi.fn() } };
  });

  const DESC = { name: 'layer_pan', label: 'Pan', min: -1, max: 1, default: 0, kind: 'float' };

  it('shows the overridden label instead of the engine display string', () => {
    const ctl = makeDial(el, 9, DESC, { displayOverride: panDisplayOverride('layer_pan') });
    // Engine sends the raw value ("-0.500"); the strip must read L50.
    ctl.update(-0.5, 0.25, '-0.500');
    const svg = el.querySelector('svg.ctl-dial');
    svg.dispatchEvent(new window.MouseEvent('pointerenter', { clientX: 10, clientY: 10 }));
    // The popup reads from the dial's last display, whichever path set it.
    expect(document.querySelector('.value-pop').textContent).toBe('L50');
  });

  it('falls back to the engine display when there is no override', () => {
    const ctl = makeDial(el, 9, DESC);
    ctl.update(-0.5, 0.25, '-0.500');
    const svg = el.querySelector('svg.ctl-dial');
    svg.dispatchEvent(new window.MouseEvent('pointerenter', { clientX: 10, clientY: 10 }));
    expect(document.querySelector('.value-pop').textContent).toBe('-0.500');
  });
});
