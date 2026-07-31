// 0016: direct coverage for the dispatch.js orchestration layer — sync-partner
// resolution, the rate/cutoff display overrides, the full layer rebind, and the
// init() → applyViewEvents fan-out. Previously this logic had only Rust-side
// substring assertions on the spliced HTML (identifiers exist, not that the
// wiring works).
//
// dispatch.js imports nothing: at splice time it shares one scope with panels.js
// / bridge.js, so cross-module symbols (makeFader, subdivisionLabel, keysPanel,
// the cutoffTuned* helpers …) are free identifiers that resolve via the global
// scope. Under Node ESM we stub them on globalThis, exactly as the splice would
// define them.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  model,
  locateSyncPartners,
  rateDisplayOverride,
  cutoffDisplayOverride,
  cutoffNormOverride,
  cutoffInteractionOverride,
  rebindAllForLayer,
  init,
  _resetParamIndex,
} from '../dispatch.js';
import { installFixture, PATCH_COUNT } from '../fixtures/params.js';

// Captures the ctl object each primitive factory returns, keyed by bound id, so
// a test can assert `update()` was driven with the reseeded value.
let madeCtls;

function resetModel() {
  model.controls.clear();
  model.lastParam.clear();
  model.syncOfRate.clear();
  model.rateOfSync.clear();
  model.tunedOfCutoff.clear();
  model.cutoffOfTuned.clear();
  model.dimRules.length = 0;
  model.dimRuleSpecs.length = 0;
  model.cells.length = 0;
  model.currentLayer = 'upper';
}

function stubGlobals() {
  madeCtls = new Map();
  const factory = (kind) => (el, id) => {
    const ctl = { update: vi.fn(), id, kind, el };
    madeCtls.set(id, ctl);
    return ctl;
  };
  globalThis.makeFader = factory('fader');
  globalThis.makeWave = factory('wave');
  globalThis.makeSwitch = factory('switch');
  globalThis.makeButtonGroup = factory('buttongroup');
  globalThis.makeDropdown = factory('dropdown');
  globalThis.makeHeaderSwitch = factory('header-switch');
  // Display/interaction helpers the override closures call (deterministic
  // stand-ins so the wiring is observable).
  globalThis.subdivisionLabel = (norm) => `sub:${norm}`;
  globalThis.cutoffTunedNormToHz = (norm) => 100 + norm * 100;
  globalThis.cutoffTunedHzToNorm = (hz) => (hz - 100) / 100;
  globalThis.cutoffTunedNoteName = (hz) => `note:${hz}`;
  // Side panels touched by rebind / dispatch.
  globalThis.keysPanel = {
    wireLayerLevels: vi.fn(),
    setLayer: vi.fn(),
    setMode: vi.fn(),
    setSplit: vi.fn(),
  };
  globalThis.statusPill = { flash: vi.fn() };
  globalThis.presetBar = { setName: vi.fn(), setSource: vi.fn() };
  globalThis.browserPanel = { setCurrentSource: vi.fn(), followPath: vi.fn() };
  globalThis.wireFxTabs = vi.fn();
  // bridge.js free globals init() reads (the early-event replay buffer and the
  // text-input callback registry).
  globalThis._earlyViewEvents = [];
  globalThis._textInputCallbacks = new Map();
}

beforeEach(() => {
  installFixture();
  _resetParamIndex();
  resetModel();
  stubGlobals();
  document.body.innerHTML = '';
});

// Mount a [data-control] cell inside a [data-layered] wrapper so isLayeredEl
// reports true (per-patch cells rebuild on a layer flip).
function mountCell(kind, name) {
  const wrap = document.createElement('div');
  wrap.setAttribute('data-layered', '');
  const el = document.createElement('div');
  el.dataset.control = kind;
  el.dataset.param = name;
  wrap.appendChild(el);
  document.body.appendChild(wrap);
  return el;
}

describe('locateSyncPartners', () => {
  it('maps rate↔sync and cutoff↔tuned at the upper layer', () => {
    locateSyncPartners('upper');
    // lfo_rate(5)↔lfo_sync(6), lfo2_rate(22)↔lfo2_sync(23),
    // delay_time(24)↔delay_sync(25).
    expect(model.syncOfRate.get(5)).toBe(6);
    expect(model.syncOfRate.get(22)).toBe(23);
    expect(model.syncOfRate.get(24)).toBe(25);
    expect(model.rateOfSync.get(6)).toBe(5);
    // cutoff(7)↔cutoff_tuned(8).
    expect(model.tunedOfCutoff.get(7)).toBe(8);
    expect(model.cutoffOfTuned.get(8)).toBe(7);
  });

  it('translates per-patch pairs on the lower layer; globals stay put', () => {
    locateSyncPartners('lower');
    expect(model.syncOfRate.get(5 + PATCH_COUNT)).toBe(6 + PATCH_COUNT);
    expect(model.tunedOfCutoff.get(7 + PATCH_COUNT)).toBe(8 + PATCH_COUNT);
    // Globals are layer-independent.
    expect(model.syncOfRate.get(24)).toBe(25);
    // The upper-layer per-patch ids are no longer present.
    expect(model.syncOfRate.has(5)).toBe(false);
  });

  it('tolerates missing params — the pair is skipped, no throw', () => {
    delete window.vxn.params[24]; // drop delay_time
    delete window.vxn.params[25];
    _resetParamIndex();
    expect(() => locateSyncPartners('upper')).not.toThrow();
    expect(model.syncOfRate.has(24)).toBe(false);
    // Other pairs still resolve.
    expect(model.syncOfRate.get(5)).toBe(6);
  });
});

describe('rateDisplayOverride', () => {
  beforeEach(() => locateSyncPartners('upper'));

  it('returns null for a fader with no sync partner', () => {
    expect(rateDisplayOverride(999)).toBe(null);
  });

  it('shows the subdivision label when the partner sync is on, else null', () => {
    const fn = rateDisplayOverride(5);
    expect(typeof fn).toBe('function');
    // Sync (id 6) on → subdivision label.
    model.lastParam.set(6, { plain: 1, norm: 1, display: 'On' });
    expect(fn(0.25, 0.25, '2 Hz')).toBe('sub:0.25');
    // Sync off → null (default numeric display).
    model.lastParam.set(6, { plain: 0, norm: 0, display: 'Off' });
    expect(fn(0.25, 0.25, '2 Hz')).toBe(null);
  });
});

describe('cutoff overrides', () => {
  beforeEach(() => locateSyncPartners('upper'));

  it('return null for a non-cutoff fader', () => {
    expect(cutoffDisplayOverride(999)).toBe(null);
    expect(cutoffNormOverride(999)).toBe(null);
    expect(cutoffInteractionOverride(999)).toBe(null);
  });

  it('map drag/display through the tuned helpers only while Tuned is on', () => {
    const disp = cutoffDisplayOverride(7);
    const norm = cutoffNormOverride(7);
    const interact = cutoffInteractionOverride(7);

    // Tuned (id 8) on.
    model.lastParam.set(8, { plain: 1, norm: 1, display: 'On' });
    expect(disp(440)).toBe('note:440');
    expect(norm(200)).toBe(cutoffTunedHzToNorm(200));
    expect(interact(0.5)).toEqual({ plain: 150, norm: cutoffTunedHzToNorm(150) });

    // Tuned off → all passthrough (null).
    model.lastParam.set(8, { plain: 0, norm: 0, display: 'Off' });
    expect(disp(440)).toBe(null);
    expect(norm(200)).toBe(null);
    expect(interact(0.5)).toBe(null);
  });
});

describe('rebindAllForLayer', () => {
  it('rebinds every layered cell to the new layer ids and reseeds from lastParam', () => {
    mountCell('fader', 'cutoff'); // upper id 7 / lower 17
    mountCell('fader', 'lfo_rate'); // upper id 5 / lower 15
    model.cells.push(
      { el: document.querySelector('[data-param="cutoff"]'), kind: 'fader', name: 'cutoff', layered: true },
      { el: document.querySelector('[data-param="lfo_rate"]'), kind: 'fader', name: 'lfo_rate', layered: true },
    );

    rebindAllForLayer('upper');
    expect([...model.controls.keys()].sort((a, b) => a - b)).toEqual([5, 7]);
    // Sync partners re-resolved for the active layer as part of the rebind.
    expect(model.syncOfRate.get(5)).toBe(6);

    // Flip to lower: ids shift by patchCount, partners re-resolve, and the
    // freshly-bound cell is reseeded from the cached lower-layer value.
    model.lastParam.set(7 + PATCH_COUNT, { plain: 0.9, norm: 0.9, display: 'X' });
    rebindAllForLayer('lower');
    expect([...model.controls.keys()].sort((a, b) => a - b)).toEqual([15, 17]);
    expect(model.syncOfRate.get(5 + PATCH_COUNT)).toBe(6 + PATCH_COUNT);
    expect(madeCtls.get(17).update).toHaveBeenCalledWith(0.9, 0.9, 'X');
  });
});

describe('init() → applyViewEvents', () => {
  it('binds cells, applies param echoes, and refreshes a sync partner on toggle', () => {
    globalThis.window.__vxn = {};
    window.vxn.send = { ready: vi.fn() };

    mountCell('fader', 'lfo_rate'); // id 5
    mountCell('switch', 'lfo_sync'); // id 6 (the sync toggle)
    // init() needs the faceplate root for nothing here, but mount it so the
    // shape matches production; the module-level auto-boot already ran (no-op).
    const root = document.createElement('div');
    root.id = 'faceplate';
    document.body.appendChild(root);

    init();
    expect(window.vxn.send.ready).toHaveBeenCalled();
    expect(typeof window.__vxn.applyViewEvents).toBe('function');

    // A param echo on the rate fader drives its ctl.
    window.__vxn.applyViewEvents([
      { kind: 'param_changed', id: 5, plain: 0.4, norm: 0.4, display: '2 Hz' },
    ]);
    expect(madeCtls.get(5).update).toHaveBeenCalledWith(0.4, 0.4, '2 Hz');

    // Toggling the sync partner (id 6) must re-update the rate fader (id 5)
    // from its last-seen value so its display flips Hz ↔ subdivision.
    madeCtls.get(5).update.mockClear();
    window.__vxn.applyViewEvents([
      { kind: 'param_changed', id: 6, plain: 1, norm: 1, display: 'On' },
    ]);
    expect(madeCtls.get(6).update).toHaveBeenCalledWith(1, 1, 'On');
    // Partner refresh fired with the rate's cached value.
    expect(madeCtls.get(5).update).toHaveBeenCalledWith(0.4, 0.4, '2 Hz');
  });
});
