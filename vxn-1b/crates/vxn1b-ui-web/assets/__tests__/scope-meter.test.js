// 0282: the scope panel's level meter. It is mounted like any other meter, but
// it shows whichever layer is being EDITED rather than a fixed channel, so it
// registers under a key no meter frame carries (`scope`) and the dispatcher
// feeds it the frame's `l1` / `l2` by hand. These tests pin that routing.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  wireScope, pushScopeMeter, _resetScopeView, model, SCOPE_METER_KEY,
} from '../dispatch.js';

let meter;

function mountMeter() {
  const el = document.createElement('div');
  el.dataset.meter = SCOPE_METER_KEY;
  document.body.appendChild(el);
}

beforeEach(() => {
  document.body.innerHTML = '';
  globalThis.window = globalThis;
  _resetScopeView();
  meter = { push: vi.fn(), reset: vi.fn() };
  // `meterRegistry` is a spliced free global in production; stub the one call
  // `wireScope` makes of it.
  globalThis.meterRegistry = { get: (k) => (k === SCOPE_METER_KEY ? meter : null) };
  model.currentLayer = 'upper';
});

describe('scope meter', () => {
  it('pushes the upper layer pair while Layer 1 is being edited', () => {
    mountMeter();
    wireScope();
    pushScopeMeter({ l1: [0.5, 0.25], l2: [0.9, 0.9] });
    expect(meter.push).toHaveBeenCalledWith([0.5, 0.25]);
  });

  it('follows the edit layer to the lower pair', () => {
    mountMeter();
    wireScope();
    model.currentLayer = 'lower';
    pushScopeMeter({ l1: [0.5, 0.25], l2: [0.9, 0.9] });
    expect(meter.push).toHaveBeenCalledWith([0.9, 0.9]);
  });

  it('wraps a scalar tap in an array, as the registry does', () => {
    mountMeter();
    wireScope();
    pushScopeMeter({ l1: 0.3 });
    expect(meter.push).toHaveBeenCalledWith([0.3]);
  });

  it('ignores a frame with no pair for the edit layer', () => {
    mountMeter();
    wireScope();
    pushScopeMeter({ l2: [0.9, 0.9] });
    expect(meter.push).not.toHaveBeenCalled();
  });

  it('is inert with no mount — the registry is never consulted', () => {
    // A faceplate (or suite) without the mount must not need `meterRegistry`
    // defined at all, which is why `wireScope` gates the lookup on the element.
    delete globalThis.meterRegistry;
    wireScope();
    expect(() => pushScopeMeter({ l1: [1, 1] })).not.toThrow();
  });
});
