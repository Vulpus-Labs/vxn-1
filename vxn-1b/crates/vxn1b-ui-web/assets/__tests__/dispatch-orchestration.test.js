// 0209: direct coverage for the vxn1b dispatch.js orchestration layer —
// sync-partner resolution, the rate display override, the cutoff-override
// no-ops (the Cutoff "Tuned" strip was dropped from the compact faceplate), the
// single-patch rebind, and the init() → applyViewEvents fan-out. This was
// forked from VXN1's suite; vxn1b is single-patch (`patchCount === 1`) so a
// layer flip is a no-op and there is no dual-layer id-shifting to test.
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
  paramIdByName,
  _resetParamIndex,
} from '../dispatch.js';
import { installFixture } from '../fixtures/params.js';

// Fixture ids the vxn1b dispatch suite drives (see fixtures/params.js). Resolved
// by name so a fixture reshuffle can't silently desync the assertions.
let LFO1_RATE, LFO1_SYNC, LFO2_RATE, LFO2_SYNC, CUTOFF, DRIVE;

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
  globalThis.makeDial = factory('dial');
  globalThis.makeBipolar = factory('bipolar');
  globalThis.makeSwitch = factory('switch');
  globalThis.makeButtonGroup = factory('buttongroup');
  globalThis.makeDropdown = factory('dropdown');
  globalThis.makeHeaderSwitch = factory('header-switch');
  // Display helper the rate override closure calls (deterministic stand-in so
  // the wiring is observable).
  globalThis.subdivisionLabel = (norm) => `sub:${norm}`;
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
  globalThis.matrixOverlay = { build: vi.fn(), refreshForLayer: vi.fn() };
  // bridge.js free globals init() reads (the early-event replay buffer and the
  // text-input callback registry).
  globalThis._earlyViewEvents = [];
  globalThis._textInputCallbacks = new Map();
}

beforeEach(() => {
  installFixture();
  // vxn1b is single-patch: patchCount = 1 makes every layer flip a no-op
  // (upper id === lower id for every param the suite touches, all id ≥ 2).
  window.vxn.patchCount = 1;
  _resetParamIndex();
  resetModel();
  stubGlobals();
  document.body.innerHTML = '';

  LFO1_RATE = paramIdByName('lfo1_rate');
  LFO1_SYNC = paramIdByName('lfo1_sync');
  LFO2_RATE = paramIdByName('lfo2_rate');
  LFO2_SYNC = paramIdByName('lfo2_sync');
  CUTOFF    = paramIdByName('cutoff');
  DRIVE     = paramIdByName('drive');
});

// Mount a [data-control] cell inside a [data-layered] wrapper so isLayeredEl
// reports true. In vxn1b the layer machinery is retained but inert (single
// patch), so a "layered" cell rebinds to the same ids on any flip.
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
  it('maps only lfo1 and lfo2 rate↔sync; cutoff-tuned pairing is empty', () => {
    locateSyncPartners('upper');
    // lfo1_rate ↔ lfo1_sync, lfo2_rate ↔ lfo2_sync.
    expect(model.syncOfRate.get(LFO1_RATE)).toBe(LFO1_SYNC);
    expect(model.syncOfRate.get(LFO2_RATE)).toBe(LFO2_SYNC);
    expect(model.rateOfSync.get(LFO1_SYNC)).toBe(LFO1_RATE);
    expect(model.rateOfSync.get(LFO2_SYNC)).toBe(LFO2_RATE);
    // The compact faceplate dropped the Cutoff "Tuned" strip and Delay Sync —
    // nothing pairs to cutoff, and the tuned maps stay empty.
    expect(model.tunedOfCutoff.size).toBe(0);
    expect(model.cutoffOfTuned.size).toBe(0);
    expect(model.tunedOfCutoff.get(CUTOFF)).toBeUndefined();
  });

  it('tolerates a missing param — the pair is skipped, the other still resolves', () => {
    delete window.vxn.params[LFO2_RATE]; // drop lfo2_rate
    delete window.vxn.params[LFO2_SYNC];
    _resetParamIndex();
    expect(() => locateSyncPartners('upper')).not.toThrow();
    expect(model.syncOfRate.has(LFO2_RATE)).toBe(false);
    // The lfo1 pair still resolves.
    expect(model.syncOfRate.get(LFO1_RATE)).toBe(LFO1_SYNC);
  });
});

describe('rateDisplayOverride', () => {
  beforeEach(() => locateSyncPartners('upper'));

  it('returns null for a fader with no sync partner', () => {
    expect(rateDisplayOverride(DRIVE)).toBe(null);
    expect(rateDisplayOverride(999)).toBe(null);
  });

  it('shows the subdivision label when the partner sync is on, else null', () => {
    const fn = rateDisplayOverride(LFO1_RATE);
    expect(typeof fn).toBe('function');
    // lfo1_sync on → subdivision label.
    model.lastParam.set(LFO1_SYNC, { plain: 1, norm: 1, display: 'On' });
    expect(fn(0.25, 0.25, '2 Hz')).toBe('sub:0.25');
    // Sync off → null (default numeric display).
    model.lastParam.set(LFO1_SYNC, { plain: 0, norm: 0, display: 'Off' });
    expect(fn(0.25, 0.25, '2 Hz')).toBe(null);
  });
});

describe('cutoff overrides', () => {
  beforeEach(() => locateSyncPartners('upper'));

  it('return null for a non-cutoff fader', () => {
    expect(cutoffDisplayOverride(DRIVE)).toBe(null);
    expect(cutoffNormOverride(DRIVE)).toBe(null);
    expect(cutoffInteractionOverride(DRIVE)).toBe(null);
  });

  it('return null for the cutoff fader too — the Tuned strip was removed', () => {
    // vxn1b has no Cutoff "Tuned" toggle, so `tunedOfCutoff` never carries the
    // cutoff id and every override collapses to the default fader behaviour.
    expect(cutoffDisplayOverride(CUTOFF)).toBe(null);
    expect(cutoffNormOverride(CUTOFF)).toBe(null);
    expect(cutoffInteractionOverride(CUTOFF)).toBe(null);
  });
});

describe('rebindAllForLayer', () => {
  it('binds every layered cell and re-resolves sync partners (single patch)', () => {
    mountCell('fader', 'cutoff');
    mountCell('fader', 'lfo1_rate');
    model.cells.push(
      { el: document.querySelector('[data-param="cutoff"]'), kind: 'fader', name: 'cutoff', layered: true },
      { el: document.querySelector('[data-param="lfo1_rate"]'), kind: 'fader', name: 'lfo1_rate', layered: true },
    );

    rebindAllForLayer('upper');
    expect([...model.controls.keys()].sort((a, b) => a - b)).toEqual(
      [CUTOFF, LFO1_RATE].sort((a, b) => a - b),
    );
    // Sync partners re-resolved for the active layer as part of the rebind.
    expect(model.syncOfRate.get(LFO1_RATE)).toBe(LFO1_SYNC);

    // A flip is a no-op in single-patch vxn1b: the same ids rebind. Seed a
    // cached value and confirm the freshly-rebound cell is reseeded from it.
    model.lastParam.set(CUTOFF, { plain: 0.9, norm: 0.9, display: 'X' });
    rebindAllForLayer('lower');
    expect([...model.controls.keys()].sort((a, b) => a - b)).toEqual(
      [CUTOFF, LFO1_RATE].sort((a, b) => a - b),
    );
    expect(model.syncOfRate.get(LFO1_RATE)).toBe(LFO1_SYNC);
    expect(madeCtls.get(CUTOFF).update).toHaveBeenCalledWith(0.9, 0.9, 'X');
  });
});

describe('init() → applyViewEvents', () => {
  it('binds cells, applies param echoes, and refreshes a sync partner on toggle', () => {
    globalThis.window.__vxn = {};
    window.vxn.send = { ready: vi.fn() };

    mountCell('fader', 'lfo1_rate');
    mountCell('switch', 'lfo1_sync'); // the sync toggle
    // Mount the faceplate root so the shape matches production; the module-level
    // auto-boot already ran under vitest (no #faceplate then) so this is inert.
    const root = document.createElement('div');
    root.id = 'faceplate';
    document.body.appendChild(root);

    init();
    expect(window.vxn.send.ready).toHaveBeenCalled();
    expect(typeof window.__vxn.applyViewEvents).toBe('function');

    // A param echo on the rate fader drives its ctl.
    window.__vxn.applyViewEvents([
      { kind: 'param_changed', id: LFO1_RATE, plain: 0.4, norm: 0.4, display: '2 Hz' },
    ]);
    expect(madeCtls.get(LFO1_RATE).update).toHaveBeenCalledWith(0.4, 0.4, '2 Hz');

    // Toggling the sync partner must re-update the rate fader from its last-seen
    // value so its display flips Hz ↔ subdivision.
    madeCtls.get(LFO1_RATE).update.mockClear();
    window.__vxn.applyViewEvents([
      { kind: 'param_changed', id: LFO1_SYNC, plain: 1, norm: 1, display: 'On' },
    ]);
    expect(madeCtls.get(LFO1_SYNC).update).toHaveBeenCalledWith(1, 1, 'On');
    // Partner refresh fired with the rate's cached value.
    expect(madeCtls.get(LFO1_RATE).update).toHaveBeenCalledWith(0.4, 0.4, '2 Hz');
  });
});
