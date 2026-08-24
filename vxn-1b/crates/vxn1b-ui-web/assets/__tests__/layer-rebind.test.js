// A rebind must leave NOTHING of the previous layer's binding behind.
//
// The bug this pins: `rebindAllForLayer` used to reset a cell by clearing its
// `innerHTML`, which disposes of listeners on the children a primitive built but
// not of any bound to the cell root. Those accumulated one closure per layer
// flip, each holding the id of the layer it was bound under — so after visiting
// Layer 2, one click on the Voice rocker wrote Poly/Solo to *both* layers, and
// one double-click reset both layers' values. The cell root carries at least
// two such listeners today (the rocker's click and the generic
// reset-to-default double-click), so the fix is structural: the node itself is
// replaced on rebind.
//
// Driven with the REAL `makeRocker` rather than a stub — a stub that never binds
// a listener cannot show the leak.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { pointerEvt } from './_helpers.js';
import { installFixture, PATCH_COUNT } from '../fixtures/params.js';
import { makeRocker, makeFader } from '../panels.js';
import {
  model, init, rebindAllForLayer, paramIdByName, _resetParamIndex,
} from '../dispatch.js';

let posts;

function stubGlobals() {
  // Real widgets for the two kinds this suite mounts; the rest are unused.
  globalThis.makeRocker = makeRocker;
  globalThis.makeFader = makeFader;
  globalThis.keysPanel = { wireLayerLevels: vi.fn(), setLayer: vi.fn(), setMode: vi.fn(), setSplit: vi.fn() };
  globalThis.statusPill = { flash: vi.fn() };
  globalThis.presetBar = { setName: vi.fn(), setSource: vi.fn() };
  globalThis.browserPanel = { setCurrentSource: vi.fn(), followPath: vi.fn() };
  globalThis.matrixOverlay = { build: vi.fn(), refreshForLayer: vi.fn() };
  globalThis.valuePop = { show: vi.fn(), hide: vi.fn(), update: vi.fn() };
  globalThis._earlyViewEvents = [];
  globalThis._textInputCallbacks = new Map();
  globalThis.window.__vxn = {};
}

function resetModel() {
  model.controls.clear();
  model.lastParam.clear();
  model.dimRules.length = 0;
  model.dimRuleSpecs.length = 0;
  model.cells.length = 0;
  model.currentLayer = 'upper';
}

beforeEach(() => {
  installFixture();
  posts = [];
  window.vxn.send = {
    setParam: (id, plain) => posts.push(['set_param', id, plain]),
    beginGesture: vi.fn(),
    endGesture: vi.fn(),
    discrete(id, plain) { posts.push(['set_param', id, plain]); },
    setEditLayer: vi.fn(),
    ready: vi.fn(),
  };
  _resetParamIndex();
  resetModel();
  stubGlobals();

  document.body.innerHTML = `
    <div id="faceplate">
      <div class="tab-pane active" data-tab-pane="layer" data-edit-layer="upper">
        <div class="panel" data-layered>
          <div class="ctl-strip voice-mode" data-control="rocker" data-param="voice_mode" data-no-label></div>
        </div>
      </div>
    </div>
  `;
  init();
});

const cell = () => document.querySelector('[data-param="voice_mode"]');
const UPPER = () => paramIdByName('voice_mode');

describe('layer rebind leaves no listeners behind', () => {
  it('writes only the current layer after a flip', () => {
    const upper = UPPER();
    const lower = upper + PATCH_COUNT;

    cell().dispatchEvent(pointerEvt('pointerdown'));
    expect(posts).toEqual([['set_param', upper, 1]]);

    posts.length = 0;
    rebindAllForLayer('lower');
    cell().dispatchEvent(pointerEvt('pointerdown'));
    // The whole bug in one assertion: exactly one write, to Layer 2 only.
    expect(posts).toEqual([['set_param', lower, 1]]);

    posts.length = 0;
    rebindAllForLayer('upper');
    cell().dispatchEvent(pointerEvt('pointerdown'));
    expect(posts).toEqual([['set_param', upper, 1]]);
  });

  it('does not accumulate across many flips', () => {
    for (let i = 0; i < 5; i++) {
      rebindAllForLayer(i % 2 ? 'upper' : 'lower');
    }
    posts.length = 0;
    cell().dispatchEvent(pointerEvt('pointerdown'));
    expect(posts).toHaveLength(1);
  });

  it('fires the reset-to-default double-click once, on one layer', () => {
    rebindAllForLayer('lower');
    posts.length = 0;
    cell().dispatchEvent(new MouseEvent('dblclick', { bubbles: true, cancelable: true }));
    expect(posts).toEqual([['set_param', UPPER() + PATCH_COUNT, 0]]);
  });

  it('restores the markup\'s own classes rather than the last primitive\'s', () => {
    // The rocker adds `.ctl-rocker` (and `.right` once on), so a rebind that
    // kept the clone's classes would compound them; one that stripped a
    // hand-listed set would drop whatever the next primitive adds.
    const before = cell().className;
    rebindAllForLayer('lower');
    expect(cell().className).toBe(before);
    expect(cell().classList.contains('voice-mode')).toBe(true);
  });

  it('keeps the cell in place in the DOM', () => {
    const parent = cell().parentElement;
    rebindAllForLayer('lower');
    expect(cell().parentElement).toBe(parent);
    expect(document.querySelectorAll('[data-param="voice_mode"]')).toHaveLength(1);
  });
});
