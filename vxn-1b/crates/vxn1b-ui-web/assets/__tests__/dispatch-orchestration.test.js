// 0209: direct coverage for the vxn1b dispatch.js orchestration layer —
// sync-partner resolution, the rate display override, the Cutoff "Tuned"
// overrides (0250), the single-patch rebind, and the init() → applyViewEvents
// fan-out. This was
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
// The tuned-mode math is a shared core primitive; the suite drives the real
// implementation rather than a stand-in so a change there fails here.
import {
  cutoffTunedNormToHz,
  cutoffTunedHzToNorm,
  cutoffTunedNoteName,
  midiToHz,
} from '../../../../../crates/vxn-core-ui-web/assets/cutoff-tuned.js';

// Fixture ids the vxn1b dispatch suite drives (see fixtures/params.js). Resolved
// by name so a fixture reshuffle can't silently desync the assertions.
let LFO1_RATE, LFO1_SYNC, LFO2_RATE, LFO2_SYNC, DELAY_TIME, DELAY_SYNC, CUTOFF, CUTOFF_TUNED, DRIVE;

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
  // Cutoff tuned-mode helpers: free identifiers in production (spliced into one
  // scope from the shared core module), so hand dispatch.js the real ones.
  globalThis.cutoffTunedNormToHz = cutoffTunedNormToHz;
  globalThis.cutoffTunedHzToNorm = cutoffTunedHzToNorm;
  globalThis.cutoffTunedNoteName = cutoffTunedNoteName;
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
  DELAY_TIME = paramIdByName('delay_time');
  DELAY_SYNC = paramIdByName('delay_sync');
  CUTOFF    = paramIdByName('cutoff');
  CUTOFF_TUNED = paramIdByName('cutoff_tuned');
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
  it('maps lfo1 / lfo2 / delay rate↔sync and the cutoff↔tuned pair', () => {
    locateSyncPartners('upper');
    // lfo1_rate ↔ lfo1_sync, lfo2_rate ↔ lfo2_sync, delay_time ↔ delay_sync.
    expect(model.syncOfRate.get(LFO1_RATE)).toBe(LFO1_SYNC);
    expect(model.syncOfRate.get(LFO2_RATE)).toBe(LFO2_SYNC);
    expect(model.syncOfRate.get(DELAY_TIME)).toBe(DELAY_SYNC);
    expect(model.rateOfSync.get(LFO1_SYNC)).toBe(LFO1_RATE);
    expect(model.rateOfSync.get(LFO2_SYNC)).toBe(LFO2_RATE);
    expect(model.rateOfSync.get(DELAY_SYNC)).toBe(DELAY_TIME);
    // Cutoff "Tuned" is back (0250) and pairs both ways so a toggle can
    // repaint the fader.
    expect(model.tunedOfCutoff.get(CUTOFF)).toBe(CUTOFF_TUNED);
    expect(model.cutoffOfTuned.get(CUTOFF_TUNED)).toBe(CUTOFF);
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

  // 0250: with Tuned OFF the overrides exist but defer — they hand back null so
  // the fader keeps its exp-Hz mapping and Hz readout.
  it('defer to the default fader while Tuned is off', () => {
    model.lastParam.set(CUTOFF_TUNED, { plain: 0, norm: 0, display: 'Off' });
    expect(cutoffDisplayOverride(CUTOFF)(1000, 0.5, '1000 Hz')).toBe(null);
    expect(cutoffNormOverride(CUTOFF)(1000)).toBe(null);
    expect(cutoffInteractionOverride(CUTOFF)(0.5)).toBe(null);
  });

  // Tuned ON: drag snaps to a semitone over MIDI C0..C4 and the readout is a
  // note name. This is the whole point of the mode — the cutoff can be set to
  // an exact pitch by eye.
  it('snap to semitones and read out note names while Tuned is on', () => {
    model.lastParam.set(CUTOFF_TUNED, { plain: 1, norm: 1, display: 'On' });
    // Fader ends + midpoint → C0 / C2 / C4 (MIDI 12 / 36 / 60).
    const interact = cutoffInteractionOverride(CUTOFF);
    expect(interact(0).plain).toBeCloseTo(midiToHz(12), 6);
    expect(interact(0.5).plain).toBeCloseTo(midiToHz(36), 6);
    expect(interact(1).plain).toBeCloseTo(midiToHz(60), 6);
    // An in-between drag snaps to the nearest semitone, not a free Hz value.
    const snapped = interact(0.26).plain;
    expect(snapped).toBeCloseTo(midiToHz(Math.round(12 + 0.26 * 48)), 6);
    // Thumb position derives from the snapped Hz, and the popup reads a note.
    expect(cutoffNormOverride(CUTOFF)(midiToHz(36))).toBeCloseTo(0.5, 6);
    expect(cutoffDisplayOverride(CUTOFF)(midiToHz(36), 0.5, '65 Hz')).toBe('C2');
    expect(cutoffDisplayOverride(CUTOFF)(midiToHz(57), 0.9375, '220 Hz')).toBe('A3');
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

  // 0247: topology is not a CLAP param, so a preset / state load reaches the
  // page only through this echo. It must replace the snapshot the combos read
  // and repaint them — and never post a `set_matrix` back (that would bounce
  // the load's own routing at the engine).
  it('swaps the matrix snapshot and repaints the overlay on a topology echo', () => {
    globalThis.window.__vxn = {};
    window.vxn.send = { ready: vi.fn(), setMatrix: vi.fn() };
    window.vxn.matrix = { sources: [], dests: [], curves: [], slots: [[], []] };

    const root = document.createElement('div');
    root.id = 'faceplate';
    document.body.appendChild(root);
    init();

    const loaded = [
      [{ source: 3, dest: 4, curve: 0, scale: 0 }],
      [{ source: 9, dest: 1, curve: 2, scale: 7 }],
    ];
    window.__vxn.applyViewEvents([{ kind: 'matrix', slots: loaded }]);

    expect(window.vxn.matrix.slots).toEqual(loaded);
    expect(matrixOverlay.refreshForLayer).toHaveBeenCalledWith(model.currentLayer);
    expect(window.vxn.send.setMatrix).not.toHaveBeenCalled();
  });

  // A malformed echo must not blank the snapshot the combos are reading.
  it('ignores a topology echo with no slot array', () => {
    globalThis.window.__vxn = {};
    window.vxn.send = { ready: vi.fn(), setMatrix: vi.fn() };
    const kept = [[{ source: 1, dest: 4, curve: 0, scale: 0 }], []];
    window.vxn.matrix = { sources: [], dests: [], curves: [], slots: kept };

    const root = document.createElement('div');
    root.id = 'faceplate';
    document.body.appendChild(root);
    init();
    matrixOverlay.refreshForLayer.mockClear();

    window.__vxn.applyViewEvents([{ kind: 'matrix' }]);
    expect(window.vxn.matrix.slots).toBe(kept);
    expect(matrixOverlay.refreshForLayer).not.toHaveBeenCalled();
  });

  // 0221: KeyState rides the state blob, not the param table, so a preset /
  // host-state load reaches the page only through this echo. The derived
  // 0/1/2 mode decomposes back into the Layer 2 and split toggles, and nothing
  // posts back — an echoed load must not bounce its own routing at the engine.
  it('decomposes a keys echo into the layer-2 / split / link reflectors', () => {
    globalThis.window.__vxn = {};
    window.vxn.send = {
      ready: vi.fn(),
      setKeyMode: vi.fn(),
      setSplitPoint: vi.fn(),
      setLfo2Link: vi.fn(),
    };

    const root = document.createElement('div');
    root.id = 'faceplate';
    document.body.appendChild(root);
    init();

    // The faceplate's own wiring installs these when its elements exist; the
    // suite mounts a bare root, so stand them in directly.
    model.setLayer2On = vi.fn();
    model.setSplitEnabled = vi.fn();
    model.setSplitPoint = vi.fn();
    model.setLfo2Link = vi.fn();

    window.__vxn.applyViewEvents([{ kind: 'keys', mode: 2, split: 48, link: true }]);

    expect(keysPanel.setMode).toHaveBeenCalledWith(2);
    expect(keysPanel.setSplit).toHaveBeenCalledWith(48);
    expect(model.setLayer2On).toHaveBeenCalledWith(true);
    expect(model.setSplitEnabled).toHaveBeenCalledWith(true);
    expect(model.setSplitPoint).toHaveBeenCalledWith(48);
    expect(model.setLfo2Link).toHaveBeenCalledWith(true);
    // Reflect-only.
    expect(window.vxn.send.setKeyMode).not.toHaveBeenCalled();
    expect(window.vxn.send.setSplitPoint).not.toHaveBeenCalled();
    expect(window.vxn.send.setLfo2Link).not.toHaveBeenCalled();
  });

  it('turns both toggles off on a keys echo back to Single', () => {
    globalThis.window.__vxn = {};
    window.vxn.send = { ready: vi.fn(), setKeyMode: vi.fn() };

    const root = document.createElement('div');
    root.id = 'faceplate';
    document.body.appendChild(root);
    init();

    model.setLayer2On = vi.fn();
    model.setSplitEnabled = vi.fn();
    model.setLfo2Link = vi.fn();

    window.__vxn.applyViewEvents([{ kind: 'keys', mode: 0, split: 60, link: false }]);

    expect(model.setLayer2On).toHaveBeenCalledWith(false);
    expect(model.setSplitEnabled).toHaveBeenCalledWith(false);
    expect(model.setLfo2Link).toHaveBeenCalledWith(false);
  });
});
