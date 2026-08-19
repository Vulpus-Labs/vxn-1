// The layer oscilloscope: the trigger search that holds a steady waveform
// still, and the tap-follows-the-tab wiring that decides what the audio thread
// captures at all.
//
// jsdom has no canvas, so `getContext` returns null and `makeScope` draws
// nothing — deliberately survivable, since the widget must also cope with the
// real case of a canvas inside a hidden tab pane. The drawing itself is
// verified by eye; what is pinned here is the sample arithmetic and the wire.
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { findFirstRisingCross, scopeStart, makeScope } from '../panels.js';
import { wireScope, syncScopeSource, _resetScopeView, model } from '../dispatch.js';

describe('findFirstRisingCross', () => {
  it('finds the first negative→positive crossing', () => {
    // Zero-crossing between index 2 and 3.
    const s = [-0.3, -0.2, -0.1, 0.4, 0.9, 0.4, -0.2];
    expect(findFirstRisingCross(s, 6)).toBe(3);
  });

  it('ignores falling crossings — a trace anchored on either edge flips', () => {
    const s = [0.5, 0.2, -0.4, -0.9, -0.2, 0.3];
    expect(findFirstRisingCross(s, 5)).toBe(5);
  });

  it('treats an exact zero as still-below, so a rise off the line counts', () => {
    expect(findFirstRisingCross([0, 0, 0.2], 2)).toBe(2);
  });

  it('returns null when the search window has no rising crossing', () => {
    expect(findFirstRisingCross([1, 0.9, 0.8, 0.7], 3)).toBe(null);
    expect(findFirstRisingCross([0, 0, 0, 0], 3)).toBe(null);
  });

  it('respects the search limit rather than scanning the whole window', () => {
    const s = [-1, -1, -1, -1, 1, 1];
    expect(findFirstRisingCross(s, 2)).toBe(null);
    expect(findFirstRisingCross(s, 4)).toBe(4);
  });
});

describe('scopeStart', () => {
  it('starts at the trigger when one lands inside the search budget', () => {
    // 16 samples ⇒ a 4-sample search window; the rise is at index 2.
    const s = new Array(16).fill(0.5);
    s[0] = -0.5;
    s[1] = -0.5;
    expect(scopeStart(s)).toBe(2);
  });

  it('falls back to the search limit, so the drawn span never changes length', () => {
    const flat = new Array(16).fill(0);
    // No crossing: still starts at the limit, so an untriggered frame draws
    // the same number of samples as a triggered one and the time base holds.
    expect(scopeStart(flat)).toBe(4);
  });

  it('handles a window too short to search', () => {
    expect(scopeStart([0.1, 0.2])).toBe(0);
    expect(scopeStart([])).toBe(0);
  });
});

describe('makeScope', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('mounts a canvas and survives a context-less environment', () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const scope = makeScope(el);
    expect(el.querySelector('canvas.scope-canvas')).not.toBeNull();
    expect(el.classList.contains('scope')).toBe(true);
    // Frames and clears must not throw when there is nothing to draw on.
    expect(() => scope.push([0, 0.5, -0.5])).not.toThrow();
    expect(() => scope.push(null)).not.toThrow();
    expect(() => scope.clear()).not.toThrow();
  });
});

// The tap opcode is what makes the audio thread capture anything at all, so
// these assertions are about DSP work as much as about pixels.
describe('scope tap follows the visible pane', () => {
  let sent;

  beforeEach(() => {
    sent = [];
    globalThis.window.vxn = { send: { setScopeSource: (s) => sent.push(s) } };
    // dispatch.js resolves the `make*` factories from the concatenated splice
    // scope, so under Node they are free identifiers the suite supplies — with
    // the real widget, so a break in it fails here too.
    globalThis.makeScope = makeScope;
    _resetScopeView();
    document.body.innerHTML = `
      <div class="tab-pane active" data-tab-pane="layer">
        <div class="scope-mount" data-scope="layer"></div>
      </div>
      <div class="tab-pane" data-tab-pane="global"></div>
    `;
    model.currentLayer = 'upper';
    wireScope();
  });

  afterEach(() => {
    document.body.innerHTML = '';
    model.currentLayer = 'upper';
  });

  const layerPane = () => document.querySelector('[data-tab-pane="layer"]');

  it('selects the edit layer while the layer pane is showing', () => {
    syncScopeSource();
    expect(sent).toEqual(['upper']);
  });

  it('re-points on a layer flip and turns off on the global tab', () => {
    syncScopeSource();
    model.currentLayer = 'lower';
    syncScopeSource();
    layerPane().classList.remove('active');
    syncScopeSource();
    expect(sent).toEqual(['upper', 'lower', 'off']);
  });

  it('posts nothing when the tap has not changed', () => {
    syncScopeSource();
    syncScopeSource();
    syncScopeSource();
    expect(sent).toEqual(['upper']);
  });

  it('is inert with no scope mounted — a faceplate without the panel', () => {
    _resetScopeView();
    document.body.innerHTML = '<div class="tab-pane active" data-tab-pane="layer"></div>';
    wireScope();
    syncScopeSource();
    // No panel ⇒ no tap posted ⇒ the audio thread never starts capturing.
    expect(sent).toEqual([]);
  });
});
